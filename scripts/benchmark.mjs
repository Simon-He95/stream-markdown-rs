import { spawnSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { cpus } from 'node:os'
import { fileURLToPath } from 'node:url'
import { isDeepStrictEqual } from 'node:util'
import { brotliCompressSync, gzipSync } from 'node:zlib'
import { loadCandidate } from './load-candidate.mjs'

const cases = {
  'stable-blocks': '## Answer\n\n中文 **strong** and [link](https://example.com).\n\n',
  'long-paragraph': '中文 **strong** _emphasis_ and `code` 🙂 ',
  'open-fence': { prefix: '```ts\n', body: 'const example = { count: 42, label: "中文" }\n' },
  'long-table': { prefix: '| Name | Value |\n| --- | --- |\n', body: '| **text** | [link](/docs) |\n' },
  'long-list': '- top **text**\n  - nested [link](/docs)\n',
  'late-reference': {
    prefix: 'A [link][late].\n\n',
    body: '\nA **stable** paragraph with no dependency.\n',
    suffix: '\n[late]: https://example.com\n',
  },
}
const scales = [1, 2, 4]
const baseBytes = 8192
const chunkBytes = 32
const config = { id: 'streaming-benchmark' }

function workload(name, scale) {
  const item = cases[name]
  const { prefix = '', body, suffix = '' } = typeof item === 'string' ? { body: item } : item
  const source = prefix + body.repeat(Math.floor(scale * baseBytes / Buffer.byteLength(body))) + suffix
  const chunks = []
  let chunk = ''
  let bytes = 0
  for (const char of source) {
    const width = Buffer.byteLength(char)
    if (bytes + width > chunkBytes) {
      chunks.push(chunk)
      chunk = ''
      bytes = 0
    }
    chunk += char
    bytes += width
  }
  if (chunk)
    chunks.push(chunk)
  return { source, chunks }
}

function percentile(values, fraction) {
  const sorted = [...values].sort((a, b) => a - b)
  return sorted[Math.ceil(sorted.length * fraction) - 1]
}

function memory() {
  const { rss, heapUsed, external, arrayBuffers } = process.memoryUsage()
  return { rss, heapUsed, external, arrayBuffers }
}

function delta(current, before) {
  return Object.fromEntries(Object.keys(before).map(key => [key, current[key] - before[key]]))
}

function measureMemory(createSession, chunks) {
  global.gc()
  const before = memory()
  const peak = { ...before }
  const sample = () => {
    const current = memory()
    for (const key of Object.keys(peak))
      peak[key] = Math.max(peak[key], current[key])
  }
  const session = createSession(config)
  for (const chunk of chunks) {
    session.append(chunk)
    sample()
  }
  session.finish()
  sample()
  global.gc()
  const retainedDelta = delta(memory(), before)
  session.dispose?.()
  global.gc()
  return {
    sampledPeakDelta: delta(peak, before),
    retainedDelta,
    afterDisposeDelta: delta(memory(), before),
  }
}

function measureLatency(createSession, chunks) {
  const started = performance.now()
  const session = createSession(config)
  const initMs = performance.now() - started
  const commits = []
  for (const chunk of chunks) {
    const start = performance.now()
    session.append(chunk)
    commits.push(performance.now() - start)
  }
  const finishing = performance.now()
  session.finish()
  const finishMs = performance.now() - finishing
  session.dispose?.()
  return {
    initMs,
    firstCommitMs: commits[0],
    appendP95Ms: percentile(commits, 0.95),
    appendTotalMs: commits.reduce((sum, duration) => sum + duration, 0),
    finishMs,
  }
}

function verifyOutput(createSession, reference, { source, chunks }) {
  const session = createSession(config)
  for (const chunk of chunks)
    session.append(chunk)
  session.finish()
  const matches = isDeepStrictEqual(session.snapshot(), reference.parseCold(source, config, true))
  const work = session.metrics?.()
  session.dispose?.()
  return { matches, work }
}

async function runChild(engine, name, scale) {
  const reference = await import('./reference.mjs')
  const { createSession } = engine === 'reference' ? reference : await loadCandidate()
  const { source, chunks } = workload(name, scale)
  const memoryBytes = measureMemory(createSession, chunks)
  const processPeakRssBytes = process.resourceUsage().maxRSS * 1024

  const { matches, work } = verifyOutput(createSession, reference, { source, chunks })
  if (!matches)
    throw new Error(`Final output differs from the cold reference for ${engine}/${name}/${scale}x`)
  measureLatency(createSession, chunks)
  const runs = Array.from({ length: 5 }, () => measureLatency(createSession, chunks))
  const medianMs = Object.fromEntries(Object.keys(runs[0]).map(key => [
    key, percentile(runs.map(run => run[key]), 0.5),
  ]))
  return {
    engine, case: name, scale, inputBytes: Buffer.byteLength(source), commits: chunks.length,
    medianMs, memoryBytes, processPeakRssBytes, work,
  }
}

const [flag, engine, workerCase, workerScale] = process.argv.slice(2)
if (flag === '--worker') {
  console.log(JSON.stringify(await runChild(engine, workerCase, Number(workerScale))))
}
else {
  const selected = flag ? [flag] : Object.keys(cases)
  if (selected.some(name => !Object.hasOwn(cases, name)))
    throw new Error(`Unknown benchmark case: ${flag}`)
  const engines = process.env.PARSER_CANDIDATE ? ['reference', 'candidate'] : ['reference']
  const results = []
  for (const name of selected) {
    for (const scale of scales) {
      for (const engine of engines) {
        const child = spawnSync(process.execPath, [
          ...process.execArgv, fileURLToPath(import.meta.url), '--worker', engine, name, scale,
        ], { encoding: 'utf8' })
        if (child.error)
          throw child.error
        if (child.status !== 0)
          throw new Error(child.stderr || `Benchmark worker exited with ${child.status ?? child.signal}`)
        results.push(JSON.parse(child.stdout))
      }
    }
  }
  const artifacts = (process.env.PARSER_ARTIFACTS ?? '').split(',').filter(Boolean).map(path => {
    const content = readFileSync(path)
    return { path, rawBytes: content.length, gzipBytes: gzipSync(content).length, brotliBytes: brotliCompressSync(content).length }
  })
  const comparisons = results.filter(row => row.engine === 'candidate').map(row => {
    const baseline = results.find(result => result.engine === 'reference' && result.case === row.case && result.scale === row.scale)
    return {
      case: row.case,
      scale: row.scale,
      referenceOverCandidateLatency: Object.fromEntries(Object.keys(row.medianMs).map(key => [
        key, baseline.medianMs[key] / row.medianMs[key],
      ])),
      candidateMinusReferenceRetainedBytes: delta(row.memoryBytes.retainedDelta, baseline.memoryBytes.retainedDelta),
    }
  })
  console.log(JSON.stringify({
    environment: { platform: process.platform, arch: process.arch, node: process.version, cpu: cpus()[0]?.model },
    chunkBytes, artifacts, results, comparisons,
  }, null, 2))
}
