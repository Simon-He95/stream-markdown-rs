import { createHash } from 'node:crypto'
import { spawnSync } from 'node:child_process'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { loadCandidate } from './load-candidate.mjs'

const commonmark = {
  name: 'commonmark',
  file: 'commonmark-0.31.2.json',
  count: 652,
  url: 'https://spec.commonmark.org/0.31.2/spec.json',
  sha256: 'd431b29d97b6f73e69d547109cf5081578fac931e72afe95639ebe766c1b2a20',
  parse: JSON.parse,
}

const gfm = {
  name: 'gfm',
  file: 'gfm-0.29.0.gfm.13.txt',
  count: 672,
  url: 'https://raw.githubusercontent.com/github/cmark-gfm/0.29.0.gfm.13/test/spec.txt',
  sha256: '7d8e5814befec287ac116786d81ff14e0adc9b13295b4494649e995408fd871c',
  parse(text) {
    const examples = /^(`{32}) example([^\n]*)\n([\s\S]*?)^\.\n([\s\S]*?)^\1$/gm
    let section = ''
    let previousEnd = 0
    return Array.from(text.matchAll(examples), (match, index) => {
      for (const heading of text.slice(previousEnd, match.index).matchAll(/^#{1,6} (.+)$/gm))
        section = heading[1]
      const [example, , extensions, markdown, html] = match
      previousEnd = match.index + example.length
      return {
        example: index + 1,
        section,
        extensions: extensions.trim() ? extensions.trim().split(/\s+/) : [],
        markdown: markdown.replaceAll('→', '\t'),
        html: html.replaceAll('→', '\t'),
      }
    })
  },
}

const directory = new URL('../.reference/spec/', import.meta.url)

async function loadSpec(spec) {
  const filename = new URL(spec.file, directory)
  let content
  let downloaded = false
  try {
    content = readFileSync(filename)
  }
  catch (error) {
    if (error.code !== 'ENOENT')
      throw error
    const response = await fetch(spec.url, { signal: AbortSignal.timeout(30_000) })
    if (!response.ok)
      throw new Error(`Could not download ${spec.file}: HTTP ${response.status}`)
    content = Buffer.from(await response.arrayBuffer())
    downloaded = true
  }

  const hash = createHash('sha256').update(content).digest('hex')
  if (hash !== spec.sha256)
    throw new Error(`Unexpected SHA-256 for ${spec.file}: ${hash}`)

  const examples = spec.parse(content.toString('utf8'))
  if (examples.length !== spec.count)
    throw new Error(`Expected ${spec.count} examples in ${spec.file}, found ${examples.length}`)
  if (downloaded)
    writeFileSync(filename, content)
  return examples
}

function renderNative(suites, streaming) {
  const cwd = fileURLToPath(new URL('../', import.meta.url))
  let executable = process.env.NATIVE_SPEC_RENDERER
  if (!executable) {
    const build = spawnSync('cargo', ['build', '--example', 'spec_renderer', '--message-format=json'], {
      cwd,
      encoding: 'utf8',
      maxBuffer: 16 * 1024 * 1024,
    })
    if (build.error)
      throw build.error
    const messages = build.stdout.trim().split('\n').filter(Boolean).map(line => JSON.parse(line))
    if (build.status !== 0)
      throw new Error(messages.map(({ message }) => message?.rendered ?? '').join('') || build.stderr)
    executable = messages.find(message => message.reason === 'compiler-artifact' && message.target.name === 'spec_renderer')?.executable
    if (!executable)
      throw new Error('Cargo did not report the native spec renderer executable.')
  }

  const input = Buffer.concat(suites.flatMap((examples, index) => examples.flatMap(example => {
    const markdown = Buffer.from(example.markdown, 'utf8')
    const dialect = example.extensions?.length ? 'gfm' : 'commonmark'
    const renderer = index === 0 ? 'standard' : 'gfm-legacy'
    const tagfilter = example.extensions?.includes('tagfilter') ? 'tagfilter' : 'no-filter'
    const mode = streaming ? 'stream' : 'batch'
    return [Buffer.from(`${markdown.length} ${dialect} ${renderer} ${tagfilter} ${mode}\n`), markdown]
  })))
  const run = spawnSync(executable, [], { cwd, input, maxBuffer: 16 * 1024 * 1024 })
  if (run.error)
    throw run.error
  if (run.status !== 0)
    throw new Error(`Native spec renderer exited with ${run.status ?? run.signal}: ${run.stderr}`)

  let offset = 0
  const result = suites.map(examples => examples.map(() => {
    const headerEnd = run.stdout.indexOf(10, offset)
    if (headerEnd < 0)
      throw new Error('Missing native renderer frame header.')
    const header = run.stdout.toString('ascii', offset, headerEnd)
    const size = Number(header)
    offset = headerEnd + 1
    if (!/^\d+$/.test(header) || !Number.isSafeInteger(size) || offset + size > run.stdout.length)
      throw new Error('Invalid native renderer frame size.')
    const html = run.stdout.toString('utf8', offset, offset + size)
    offset += size
    return html
  }))
  if (offset !== run.stdout.length)
    throw new Error('Unexpected extra output from the native renderer.')
  return result
}

const list = process.argv.includes('--list')
const nativeStream = process.argv.includes('--native-stream')
const native = (process.argv.includes('--native') || nativeStream) && !list
const sectionFilter = process.argv.find(argument => argument.startsWith('--section='))?.slice(10).toLowerCase()
const candidate = list || native ? null : await loadCandidate()
if (candidate && typeof candidate.renderSpec !== 'function')
  throw new TypeError('The candidate adapter must export renderSpec(markdown, { dialect, extensions }).')

mkdirSync(directory, { recursive: true })
const loaded = await Promise.all([commonmark, gfm].map(loadSpec))
const suites = sectionFilter
  ? loaded.map(examples => examples.filter(example => example.section.toLowerCase().includes(sectionFilter)))
  : loaded
if (!suites.some(examples => examples.length))
  throw new Error(`No examples in a section matching ${sectionFilter}`)
const nativeHtml = native ? renderNative(suites, nativeStream) : null
let failures = 0

for (const [index, spec] of [commonmark, gfm].entries()) {
  const examples = suites[index]
  if (!examples.length)
    continue
  if (list) {
    console.log(`${spec.name}: ${examples.length} examples`)
    continue
  }
  let count = 0
  const sections = new Map()
  for (const [exampleIndex, example] of examples.entries()) {
    const actual = nativeHtml === null
      ? candidate.renderSpec(example.markdown, { dialect: spec.name, extensions: example.extensions ?? [] })
      : nativeHtml[index][exampleIndex]
    if (actual === example.html)
      continue
    if (count < 10)
      console.error(`${spec.name} #${example.example}\n  input: ${JSON.stringify(example.markdown)}\n  expected: ${JSON.stringify(example.html)}\n  actual: ${JSON.stringify(actual)}`)
    if (native)
      sections.set(example.section, (sections.get(example.section) ?? 0) + 1)
    count++
  }
  console.log(`${spec.name}: ${examples.length - count}/${examples.length} passed`)
  for (const [section, mismatches] of [...sections].sort((a, b) => b[1] - a[1]))
    console.log(`  ${mismatches} mismatches in ${section}`)
  failures += count
}

if (failures)
  process.exitCode = 1
