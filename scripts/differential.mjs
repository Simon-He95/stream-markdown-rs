import { isDeepStrictEqual, inspect } from 'node:util'
import { readdirSync, readFileSync } from 'node:fs'
import * as reference from './reference.mjs'
import { loadCandidate } from './load-candidate.mjs'

const fixtures = [
  {
    id: 'math-crlf-unicode',
    seed: 62601,
    source: [
      '# 数学与 Unicode 🙂',
      '',
      '行内公式 $x + 1$，以及未完成公式 $y +',
      '',
      '$$',
      '\\sum_{i=0}^{n} i',
      '$$',
      '',
      '未完成括号公式 \\[z + 1',
    ].join('\r\n'),
    restartSource: '重启后的内容 👋\r\n\r\n完整公式 $a+b$。\r\n',
    options: { includeSourceMap: true },
  },
  {
    id: 'html-custom-links',
    seed: 62602,
    source: [
      '<thinking>分析 <em>中</em></thinking>',
      '',
      '<div data-kind="box">HTML 内容</div>',
      '',
      '[安全链接][safe] 与 [拒绝链接](javascript:alert(1))',
      '',
      '[safe]: https://example.com/docs',
      '',
      '<thinking>未完成',
    ].join('\n'),
    restartSource: '<thinking>新的会话</thinking>\n\n[链接](https://example.com)\n',
    options: {
      customHtmlTags: ['thinking'],
      validateLink: url => !/^\s*javascript:/i.test(url),
    },
  },
  {
    id: 'lists-tables-fences',
    seed: 62603,
    source: [
      '- 第一项',
      '  - nested 🙂',
      '- 第二项',
      '',
      '| 列 A | 列 B |',
      '| --- | --- |',
      '| 1 | 二 |',
      '',
      '```ts',
      'const value = `中🙂`',
      '```',
      '',
      '~~~diff',
      '+ added',
    ].join('\n'),
    restartSource: '1. restarted\n2. stable\n\n```js\nconsole.log(1)\n```\n',
    options: { requireClosingStrong: true },
  },
  {
    id: 'inline-dispatcher-heavy',
    seed: 63401,
    source: [
      '## Inline dispatcher 中🙂',
      '',
      '前缀 **strong [link `code` 与 $x_1$](https://example.com/a_(b)) 后缀**，再接 *强调*、~~删除~~、==高亮==、++插入++、H~2~O 与 x^2^。',
      '',
      '转义：\\*literal\\* \\[not link\\] \\$not math\\$ \\`not code\\`。',
      '',
      '[![alt *字*](https://images.example/complete.png "图")](https://example.com/outer)',
      '',
      '[![流式图片](https://images.example/loading.png)](https://example.com/loading',
      '',
      '未完成：**strong [label](https://example.com/path 与 `code',
    ].join('\n'),
    restartSource: '重启：**[新链接 \\*字\\* 与 `code`](https://example.com/restart)**，以及 $a+b$。\n',
    options: { requireClosingStrong: true },
  },
  {
    // An unclosed wrapper swallows a <details> block, the nested list raw has no
    // literal counterpart at its own position, and a later paragraph repeats
    // that text verbatim (plus a second <details> block with the same summary).
    // Before the merge cursor was anchored on node source ranges, this shape made
    // the HTML merge pass scan the rest of the document for every list node.
    id: 'html-wrapper-details-duplicate-raw',
    seed: 63402,
    source: [
      '<details>',
      '<summary>Summary 2</summary>',
      '',
      '- x1',
      '- y1',
      '',
      '</details>',
      '',
      '<div class="same">',
      'BETA',
      '',
      '',
      '<details>',
      '<summary>Summary 2</summary>',
      '',
      '- p2',
      '- q2',
      '',
      '</details>',
      '',
      '<div class="same">',
      'DELTA',
      '</div>',
      '',
      'trailing copy:',
      'x1',
      'y1',
    ].join('\n'),
    restartSource: '<div class="same">\nRESTART\n\n<details>\n<summary>S</summary>\n\n- r1\n- r2\n\n</details>\n',
    options: {},
  },
]

function createRandom(seed) {
  let state = seed >>> 0
  return () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0
    return state / 0x100000000
  }
}

function randomInt(random, minimum, maximum) {
  return minimum + Math.floor(random() * (maximum - minimum + 1))
}

// Keep JS input chunks valid Unicode. The native suite will exercise raw byte splits.
function boundary(source, offset) {
  const code = source.charCodeAt(offset)
  return code >= 0xDC00 && code <= 0xDFFF ? offset + 1 : offset
}

function appendInChunks(actions, source, random) {
  let offset = 0
  while (offset < source.length) {
    const end = boundary(source, offset + randomInt(random, 1, Math.min(19, source.length - offset)))
    actions.push({ kind: 'append', text: source.slice(offset, end), final: false })
    offset = end
  }
}

function createActionPlan(fixture, seed) {
  const random = createRandom(seed)
  const actions = [{ kind: 'reset', final: false }]
  appendInChunks(actions, fixture.source, random)
  actions.push({ kind: 'duplicate', final: false })
  actions.push({ kind: 'final-transition', final: true })
  actions.push({ kind: 'final-transition', final: false })

  const truncateCount = randomInt(random, 1, Math.min(17, fixture.source.length))
  const truncatedLength = boundary(fixture.source, fixture.source.length - truncateCount)
  actions.push({ kind: 'truncate', length: truncatedLength, final: false })
  actions.push({ kind: 'duplicate', final: false })
  actions.push({ kind: 'append', text: fixture.source.slice(truncatedLength), final: false })

  const replaceStart = boundary(fixture.source, randomInt(random, 1, Math.max(1, fixture.source.length - 3)))
  const replaceEnd = boundary(fixture.source, Math.min(fixture.source.length, replaceStart + randomInt(random, 1, 5)))
  actions.push({
    kind: 'replace-range',
    start: replaceStart,
    end: replaceEnd,
    text: `替换-${seed.toString(36)}-🙂`,
    final: false,
  })
  actions.push({ kind: 'duplicate', final: false })
  actions.push({ kind: 'final-transition', final: true })
  actions.push({ kind: 'reset', final: false })
  appendInChunks(actions, fixture.restartSource, random)
  actions.push({ kind: 'duplicate', final: false })
  actions.push({ kind: 'final-transition', final: true })
  return actions
}

function applyAction(source, action) {
  switch (action.kind) {
    case 'append':
      return source + action.text
    case 'duplicate':
    case 'final-transition':
      return source
    case 'truncate':
      return source.slice(0, action.length)
    case 'replace-range':
      return source.slice(0, action.start) + action.text + source.slice(action.end)
    case 'reset':
      return ''
    default:
      throw new Error(`Unknown differential action: ${action.kind}`)
  }
}

const checkingCandidate = Boolean(process.env.PARSER_CANDIDATE)
const { createSession } = checkingCandidate ? await loadCandidate() : reference
const requested = process.argv[2]

function verify(id, options, actions, corpus = false) {
  const config = {
    id,
    factoryOptions: options.customHtmlTags ? { customHtmlTags: options.customHtmlTags } : undefined,
    parseOptions: options,
  }
  let session
  let source = ''
  let wasFinal = false
  let warmColdDifferences = 0
  // The upstream corpus validator reused its sync parser instance. Keep that
  // baseline visible and use a fresh instance for every candidate comparison.
  const legacyCorpusReader = corpus && !checkingCandidate ? reference.createColdReader(config) : null

  try {
    for (const [index, action] of actions.entries()) {
      source = applyAction(source, action)
      const rebuild = !session || wasFinal || ['reset', 'truncate', 'replace-range'].includes(action.kind)
      if (rebuild) {
        session?.dispose?.()
        session = createSession(config)
        session.append(source)
      }
      else if (action.kind === 'append') {
        session.append(action.text)
      }
      if (action.final)
        session.finish()
      wasFinal = action.final

      const fresh = reference.parseCold(source, config, action.final)
      const expected = legacyCorpusReader ? legacyCorpusReader(source, action.final) : fresh
      if (legacyCorpusReader && !isDeepStrictEqual(expected, fresh))
        warmColdDifferences++
      const actual = session.snapshot()
      if (!isDeepStrictEqual(actual, expected)) {
        let node = 0
        while (node < Math.max(expected.length, actual.length) && isDeepStrictEqual(actual[node], expected[node]))
          node++
        const settings = { depth: 4, maxArrayLength: 8, maxStringLength: 320 }
        throw new Error([
          `${id}, commit ${index}, ${action.kind}, final=${action.final}`,
          `input length: ${source.length}, last 200 chars: ${JSON.stringify(source.slice(-200))}`,
          `expected node[${node}]: ${inspect(expected[node], settings)}`,
          `actual node[${node}]: ${inspect(actual[node], settings)}`,
        ].join('\n'))
      }
    }
  }
  finally {
    session?.dispose?.()
  }

  const diagnostics = warmColdDifferences ? `; legacy reused sync differs from fresh on ${warmColdDifferences} commit(s)` : ''
  console.log(`PASS ${id} (${actions.length} commits${diagnostics})`)
}

const corpora = new URL('../fixtures/corpus/', import.meta.url)
let checked = 0
for (const fixture of fixtures) {
  if (requested && requested !== fixture.id)
    continue
  verify(fixture.id, fixture.options, createActionPlan(fixture, fixture.seed))
  checked++
}

for (const name of readdirSync(corpora).sort()) {
  if (requested && requested !== name)
    continue
  const text = readFileSync(new URL(name, corpora), 'utf8')
  const chunkSize = Math.max(1, Math.ceil(text.length / 120))
  const actions = []
  for (let offset = 0; offset < text.length;) {
    const end = boundary(text, Math.min(offset + chunkSize, text.length))
    actions.push({ kind: 'append', text: text.slice(offset, end), final: false })
    offset = end
  }
  actions.push({ kind: 'final-transition', final: true })
  verify(name, {}, actions, true)
  checked++
}
if (!checked)
  throw new Error(`Unknown differential fixture: ${requested}`)
