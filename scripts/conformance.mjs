import { createHash } from 'node:crypto'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
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
    return Array.from(text.matchAll(examples), ([, , extensions, markdown, html], index) => ({
      example: index + 1,
      extensions: extensions.trim() ? extensions.trim().split(/\s+/) : [],
      markdown: markdown.replaceAll('→', '\t'),
      html: html.replaceAll('→', '\t'),
    }))
  },
}

const directory = new URL('../.reference/spec/', import.meta.url)

async function loadSpec(spec) {
  const filename = new URL(spec.file, directory)
  let content
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
  }

  const hash = createHash('sha256').update(content).digest('hex')
  if (hash !== spec.sha256)
    throw new Error(`Unexpected SHA-256 for ${spec.file}: ${hash}`)

  const examples = spec.parse(content.toString('utf8'))
  if (examples.length !== spec.count)
    throw new Error(`Expected ${spec.count} examples in ${spec.file}, found ${examples.length}`)
  writeFileSync(filename, content)
  return examples
}

const list = process.argv.includes('--list')
const candidate = list ? null : await loadCandidate()
if (candidate && typeof candidate.renderSpec !== 'function')
  throw new TypeError('The candidate adapter must export renderSpec(markdown, { dialect, extensions }).')

mkdirSync(directory, { recursive: true })
const suites = await Promise.all([commonmark, gfm].map(loadSpec))
let failures = 0

for (const [index, spec] of [commonmark, gfm].entries()) {
  const examples = suites[index]
  if (list) {
    console.log(`${spec.name}: ${examples.length} examples`)
    continue
  }
  let count = 0
  for (const example of examples) {
    const actual = candidate.renderSpec(example.markdown, { dialect: spec.name, extensions: example.extensions ?? [] })
    if (actual === example.html)
      continue
    if (count < 10)
      console.error(`${spec.name} #${example.example}\n  input: ${JSON.stringify(example.markdown)}\n  expected: ${JSON.stringify(example.html)}\n  actual: ${JSON.stringify(actual)}`)
    count++
  }
  console.log(`${spec.name}: ${examples.length - count}/${examples.length} passed`)
  failures += count
}

if (failures)
  process.exitCode = 1
