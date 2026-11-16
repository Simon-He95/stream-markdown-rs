import assert from 'node:assert/strict'
import test from 'node:test'
import { createSession } from '../scripts/native-candidate.mjs'

test('native candidate keeps source and block IDs independent across sessions', () => {
  const first = createSession({ parseOptions: { includeSourceMap: true } })
  const second = createSession()
  try {
    first.append('first \uD83D')
    second.append('other session')
    first.append('\uDE00\n\n')
    first.finish()
    second.finish()
    assert.deepEqual(first.snapshot(), [{
      type: 'paragraph',
      children: [{ type: 'text', content: 'first 😀', raw: 'first 😀', center: false }],
      raw: 'first 😀',
      sourceMap: { startLine: 0, endLine: 1 },
    }])
    assert.equal(second.snapshot()[0].raw, 'other session')
    assert.equal(first.metrics().scannedBytes, Buffer.byteLength('first 😀\n\n'))
  }
  finally {
    first.dispose()
    second.dispose()
  }
})

test('native candidate applies reference backpatch and nested fence deltas', () => {
  const session = createSession()
  try {
    session.append('[jump][dest]\n\n')
    const before = session.snapshot()
    assert.equal(before[0].children[0].type, 'text')
    session.append('[dest]: /docs\n\n> ```rs\n> first\n')
    assert.equal(session.snapshot()[0].children[0].href, '/docs')
    assert.equal(session.snapshot()[1].children[0].loading, true)
    session.append('> next\n> ```\n')
    session.finish()
    assert.equal(session.snapshot()[1].children[0].code, 'first\nnext\n')
    assert.equal(session.snapshot()[1].children[0].loading, false)
    assert.equal(before[0].children[0].type, 'text')
    assert.equal(session.metrics().invalidatedBlocks, 1)
  }
  finally {
    session.dispose()
  }
})

test('nested paragraph raw keeps line fragments after close and reference backpatch', () => {
  const session = createSession({ parseOptions: { includeSourceMap: true } })
  try {
    session.append('> 中文 [jump][id]\r\n> second line\r\n\r\n')
    const raw = '中文 [jump][id]\nsecond line'
    assert.equal(session.snapshot()[0].children[0].raw, raw)
    session.append('[id]: /docs\n\n')
    session.finish()
    const quote = session.snapshot()[0]
    assert.equal(quote.raw, raw)
    assert.equal(quote.children[0].raw, raw)
    assert.deepEqual(quote.children[0].sourceMap, { startLine: 0, endLine: 2 })
    assert.equal(quote.children[0].children.find(node => node.type === 'link').href, '/docs')
  }
  finally {
    session.dispose()
  }
})

test('native candidate delivers Rust custom tag metadata only after a complete opener', () => {
  const session = createSession({ factoryOptions: { customHtmlTags: ['tool-call'], markstream: false } })
  try {
    session.append('<tool-call title="a >')
    assert.equal(session.snapshot().some(node => node.type === 'tool-call'), false)
    session.append(' b">lookup</tool-call>\n')
    session.finish()
    assert.equal(session.snapshot()[0].type, 'tool-call')
    assert.deepEqual(session.snapshot()[0].attrs, [['title', 'a > b']])
    assert.equal(session.snapshot()[0].content, 'lookup')
  }
  finally {
    session.dispose()
  }
})

test('native candidate keeps a long raw HTML line live while its closer is arriving', () => {
  const opening = '<DIV class="outer">'
  const body = '长文本'.repeat(500)
  for (const chunks of [[opening, body + '</di'], [opening + body + '</di']]) {
    const session = createSession()
    try {
      for (const chunk of chunks)
        session.append(chunk)
      const pending = session.snapshot()[0]
      assert.equal(pending.raw, opening + body + '</di')
      assert.equal(pending.content, opening + body + '\n</div>')
      assert.equal(pending.loading, true)
      session.append('v>')
      const closed = session.snapshot()[0]
      assert.equal(closed.raw, opening + body + '</div>')
      assert.equal(closed.loading, false)
      session.finish()
      assert.deepEqual(session.snapshot(), [closed])
    }
    finally {
      session.dispose()
    }
  }
})

test('native candidate ignores apparent HTML closers inside a split comment', () => {
  const session = createSession()
  try {
    const source = '<div>before<!-- </div> -->after</div>'
    for (const chunk of ['<div>before<!', '-- </div> -', '-', '>after</di']) {
      session.append(chunk)
      assert.equal(session.snapshot()[0].loading, true)
    }
    session.append('v>')
    assert.equal(session.snapshot()[0].loading, false)
    assert.equal(session.snapshot()[0].raw, source)
    session.finish()
    assert.equal(session.snapshot()[0].raw, source)
  }
  finally {
    session.dispose()
  }
})

test('native candidate keeps HTML open across literal closers in raw text elements', () => {
  for (const tag of ['script', 'style', 'textarea']) {
    const session = createSession()
    try {
      const opening = `<DIV><${tag}>`
      const rawBody = 'x = "</div>"'
      const rawCloser = `</${tag.toUpperCase()}>`
      for (const chunk of [opening, rawBody, rawCloser.slice(0, -2), rawCloser.slice(-2), 'after</di']) {
        session.append(chunk)
        assert.equal(session.snapshot()[0].loading, true, tag)
      }
      session.append('v>')
      assert.equal(session.snapshot()[0].loading, false, tag)
      session.finish()
      assert.equal(session.snapshot()[0].raw, `${opening}${rawBody}${rawCloser}after</div>`)
    }
    finally {
      session.dispose()
    }
  }
})

test('native candidate exposes complete attributes from a multiline registered opening', () => {
  const session = createSession({ factoryOptions: { customHtmlTags: ['tool-call'] } })
  try {
    session.append('<tool-call\n  name="search"\n  title="甲 >\n')
    assert.equal(session.snapshot().length, 0)
    session.append('乙"')
    assert.equal(session.snapshot().length, 0)
    session.append('>payload')
    assert.deepEqual(session.snapshot()[0].attrs, [['name', 'search'], ['title', '甲 >\n乙']])
    assert.equal(session.snapshot()[0].loading, true)
    session.append('</tool-call>')
    session.finish()
    const [paragraph] = session.snapshot()
    assert.equal(paragraph.type, 'paragraph')
    const [tag] = paragraph.children
    assert.deepEqual(tag.attrs, [['name', 'search'], ['title', '甲 >\n乙']])
    assert.equal(tag.content, 'payload')
    assert.equal(tag.loading, false)
  }
  finally {
    session.dispose()
  }
})

test('native candidate addresses a backpatched table cell by its flat wire index', () => {
  const session = createSession()
  try {
    session.append('| title | value |\n|---|---|\n| [jump][dest] | here |\n\n')
    assert.equal(session.snapshot()[0].rows[0].cells[0].children[0].type, 'text')
    session.append('[dest]: /next\n\n')
    session.finish()
    const table = session.snapshot()[0]
    assert.equal(table.header.cells[0].raw, 'title')
    assert.equal(table.rows[0].cells[0].children[0].href, '/next')
    assert.equal(table.rows[0].cells[1].raw, 'here')
  }
  finally {
    session.dispose()
  }
})

test('native candidate projects pending and confirmed table code at the same row', () => {
  const session = createSession()
  try {
    session.append('| Key | Value |\n| --- | --- |\n| ')
    assert.equal(session.snapshot()[0].rows[0].raw, '|')
    session.append('x | Use `')
    assert.deepEqual(session.snapshot()[0].rows[0].cells[1], {
      type: 'table_cell', header: false, align: 'left', raw: 'Use `', children: [
        { type: 'text', content: 'Use ', raw: 'Use ' },
        { type: 'inline_code', code: '', raw: '' },
      ],
    })
    session.append('cmd')
    const pending = session.snapshot()[0].rows[0].cells[1].children
    assert.deepEqual(pending, [
      { type: 'text', content: 'Use ', raw: 'Use ' },
      { type: 'inline_code', code: 'cmd', raw: 'cmd' },
    ])
    session.append('` |\n')
    session.finish()
    const rows = session.snapshot()[0].rows
    assert.equal(rows.length, 1)
    assert.deepEqual(rows[0].cells[1].children, [
      { type: 'text', content: 'Use ', raw: 'Use ', center: false },
      pending[1],
    ])
  }
  finally {
    session.dispose()
  }
})

test('legacy table projection keeps a trailing single backtick distinct from a longer run', () => {
  const session = createSession()
  try {
    session.append('| A | B |\n| - | - |\n| ` | text `` |\n')
    const [single, multiple] = session.snapshot()[0].rows[0].cells
    assert.equal(single.raw, '`')
    assert.deepEqual(single.children, [])
    assert.deepEqual(multiple.children, [{ type: 'text', content: 'text ``', raw: 'text ``' }])
    session.finish()
    assert.deepEqual(session.snapshot()[0].rows[0].cells[0].children, [
      { type: 'inline_code', code: '', raw: '' },
    ])
  }
  finally {
    session.dispose()
  }
})

test('native candidate projects resolved link and image metadata from typed spans', () => {
  const session = createSession()
  try {
    const source = '[**label**](<https://a.test/中文> \'title\') ![*caption*](/图.png "")'
    session.append(source + '\n\nprefix <a CLASS="label" HREF="https://example.com" TITLE="tip">link</a>')
    session.finish()
    const [link, , image] = session.snapshot()[0].children
    assert.equal(link.markup, '')
    assert.equal(link.text, '**label**')
    assert.equal(link.raw, '[**label**](https://a.test/%E4%B8%AD%E6%96%87 "title")')
    assert.deepEqual(link.attrs, [['href', 'https://a.test/%E4%B8%AD%E6%96%87'], ['title', 'title']])
    assert.deepEqual(image, {
      type: 'image',
      src: '/%E5%9B%BE.png',
      title: null,
      alt: 'caption',
      raw: '*caption*',
      loading: false,
    })
    const htmlLink = session.snapshot()[1].children[1]
    assert.equal(htmlLink.markup, 'html')
    assert.equal(htmlLink.title, 'tip')
    assert.equal(htmlLink.raw, '<a CLASS="label" HREF="https://example.com" TITLE="tip">link</a>')
    assert.deepEqual(htmlLink.attrs, [['CLASS', 'label'], ['HREF', 'https://example.com'], ['TITLE', 'tip']])
  }
  finally {
    session.dispose()
  }
})

test('native candidate keeps full link metadata on a line containing inline code', () => {
  const session = createSession()
  try {
    session.append('- Update `code` ([#811](https://example.com/811))\n')
    const link = session.snapshot()[0].items[0].children[0].children.find(node => node.type === 'link')
    assert.equal(link.title, null)
    assert.deepEqual(link.attrs, [['href', 'https://example.com/811']])
  }
  finally {
    session.dispose()
  }
})

test('native candidate exposes inline and block math and accepts both feature settings', () => {
  const enabled = createSession()
  const disabled = createSession({ factoryOptions: { enableMath: false } })
  try {
    const source = 'Equation $x + 1$.\n\n\\[\ny + 2\n\\]\n'
    for (const session of [enabled, disabled]) {
      session.append(source)
      session.finish()
    }

    assert.deepEqual(enabled.snapshot()[0].children[1], {
      type: 'math_inline', content: 'x + 1', raw: '$x + 1$', markup: '$', loading: false,
    })
    assert.deepEqual(enabled.snapshot()[1], {
      type: 'math_block', content: 'y + 2', raw: '\\[y + 2\\]', markup: '\\[\\]', loading: false,
    })
    assert.equal(disabled.snapshot()[0].children[0].content, 'Equation $x + 1$.')
    assert.equal(disabled.snapshot().some(node => node.type === 'math_block'), false)
  }
  finally {
    enabled.dispose()
    disabled.dispose()
  }
})

test('native candidate projects resolved footnotes after body blocks', () => {
  const session = createSession()
  const disabled = createSession({ parseOptions: { enableFootnotes: false } })
  try {
    const prefix = 'Use [^first] twice [^first].\n\n'
    session.append(prefix)
    assert.equal(session.snapshot()[0].children[0].type, 'text')
    const suffix = '[^unused]: skip\n\n[^first]: A **note**.\n\nAfter definitions.'
    session.append(suffix)
    session.finish()
    const [body, after, note] = session.snapshot()
    assert.deepEqual(body.children.filter(node => node.type === 'footnote_reference'), [
      { type: 'footnote_reference', id: 'first', raw: '[^first]' },
      { type: 'footnote_reference', id: 'first', raw: '[^first]' },
    ])
    assert.equal(after.raw, 'After definitions.')
    assert.equal(note.raw, '[^first]: A **note**.')
    assert.deepEqual(note.children[0].children.at(-1), { type: 'footnote_anchor', id: 'first', raw: '' })
    assert.equal(session.snapshot().length, 3)

    disabled.append(prefix + suffix)
    disabled.finish()
    assert.equal(disabled.snapshot()[0].children[0].type, 'text')
    assert.equal(disabled.snapshot().some(node => node.type === 'footnote'), false)
  }
  finally {
    session.dispose()
    disabled.dispose()
  }
})

test('native candidate removes provisional math when the same source finishes as plain text', () => {
  const session = createSession()
  try {
    session.append('Equation $x +')
    const preview = session.snapshot()[0]
    assert.equal(preview.raw, 'Equation $x +')
    assert.equal(preview.children[1].type, 'math_inline')
    assert.equal(preview.children[1].loading, true)
    session.finish()
    assert.deepEqual(session.snapshot()[0].children, [
      { type: 'text', content: 'Equation $x +', raw: 'Equation $x +', center: false },
    ])
    assert.equal(preview.children[1].loading, true)
  }
  finally {
    session.dispose()
  }
})

test('native candidate tracks image preview loading while its destination arrives', () => {
  const session = createSession()
  try {
    session.append('![caption $X[v,d]$](')
    assert.deepEqual(session.snapshot()[0].children[0], {
      type: 'image', src: '', alt: 'caption $X[v,d]$', title: null,
      raw: 'caption $X[v,d]$', loading: true,
    })
    session.append('https://example.com/image.png')
    assert.equal(session.snapshot()[0].children[0].src, 'https://example.com/image.png')
    assert.equal(session.snapshot()[0].children[0].loading, true)
    session.append(')')
    session.finish()
    assert.equal(session.snapshot()[0].children[0].loading, false)
  }
  finally {
    session.dispose()
  }
})

test('native candidate exposes Markstream containers and supports explicit opt-out', () => {
  const enabled = createSession()
  const disabled = createSession({ parseOptions: { markstream: false } })
  try {
    const head = '::: viewcode:live args {value:hello}\n'
    enabled.append(head + 'body')
    const vmr = enabled.snapshot()[0]
    assert.equal(vmr.type, 'vmr_container')
    assert.equal(vmr.name, 'viewcode:live')
    assert.deepEqual(vmr.attrs, { args: 'args', value: 'hello' })
    assert.equal(vmr.loading, true)
    assert.equal(vmr.children[0].raw, 'body')
    enabled.append('\n:::')
    assert.equal(enabled.snapshot()[0].loading, false)

    disabled.append(head + 'body\n:::')
    disabled.finish()
    assert.equal(disabled.snapshot()[0].type, 'paragraph')
  }
  finally {
    enabled.dispose()
    disabled.dispose()
  }
})

test('native candidate shows held fence delimiters through legacy loading and source metadata', () => {
  const session = createSession({ parseOptions: { includeSourceMap: true } })
  try {
    session.append('~~~js\r\ncode\r\n~~')
    const pending = session.snapshot()[0]
    assert.equal(pending.code, 'code')
    assert.equal(pending.loading, true)
    assert.deepEqual(pending.sourceMap, { startLine: 0, endLine: 3 })
    session.append('~')
    const closed = session.snapshot()[0]
    assert.equal(closed.code, 'code\n')
    assert.equal(closed.loading, false)
    assert.deepEqual(closed.sourceMap, { startLine: 0, endLine: 3 })
    session.finish()
    assert.equal(session.snapshot()[0].code, 'code\n')
  }
  finally {
    session.dispose()
  }
})

test('native candidate materializes legacy table preview from typed native fields', () => {
  const session = createSession({ parseOptions: { includeSourceMap: true } })
  try {
    session.append('| A | B |\n| --- | ')
    assert.equal(session.snapshot()[0].type, 'table')
    assert.equal(session.snapshot()[0].loading, true)
    assert.deepEqual(session.snapshot()[0].sourceMap, { startLine: 0, endLine: 2 })
    session.append('-')
    assert.equal(session.snapshot()[0].loading, false)
    session.append(' not a separator')
    assert.equal(session.snapshot()[0].type, 'paragraph')
  }
  finally {
    session.dispose()
  }
})

test('legacy link projection keeps local file hrefs and demotes hosted file hrefs', () => {
  const session = createSession()
  try {
    session.append('[local](file:///tmp/example) [hosted](file://host/tmp/example)')
    session.finish()
    const children = session.snapshot()[0].children
    const links = children.filter(child => child.type === 'link')
    assert.deepEqual(links.map(link => link.href), ['file:///tmp/example'])
    assert.ok(children.some(child => child.content?.includes('hosted')))
  }
  finally {
    session.dispose()
  }
})

test('native candidate projects Markstream inline extension payloads into legacy nodes', () => {
  const session = createSession()
  try {
    session.append(String.raw`H~foo\~bar~O x^3^ ==mark *em*== ++insert *em*++`)
    session.finish()
    const children = session.snapshot()[0].children
    const subscript = children.find(node => node.type === 'subscript')
    assert.equal(subscript.children[0].content, 'foo~bar')
    assert.equal(subscript.raw, '~foo~bar~')
    assert.equal(children.find(node => node.type === 'superscript').raw, '^3^')
    assert.equal(children.find(node => node.type === 'highlight').raw, '==mark em==')
    assert.equal(children.find(node => node.type === 'insert').raw, '++insert em++')
  }
  finally {
    session.dispose()
  }
})

test('legacy delimiter raw uses typed content while the paragraph keeps the source spelling', () => {
  const session = createSession()
  try {
    const input = '[**用 `pnpm` 安装**](https://a.test) [***x***](https://b.test)'
    session.append(input)
    session.finish()
    const paragraph = session.snapshot()[0]
    const links = paragraph.children.filter(node => node.type === 'link')
    assert.equal(paragraph.raw, input)
    assert.equal(links[0].text, '**用 pnpm 安装**')
    assert.equal(links[1].text, '*x*')
    assert.equal(links[1].children[0].children[0].type, 'strong')
  }
  finally {
    session.dispose()
  }
})

test('native candidate keeps a provisional table row out of the committed row list', () => {
  const session = createSession()
  try {
    session.append('| A | B |\n| --- | --- |\n| one | ')
    const preview = session.snapshot()[0]
    assert.equal(preview.rows.length, 1)
    assert.equal(preview.rows[0].raw, 'one|')
    session.append('two |\n')
    assert.equal(session.snapshot()[0].rows.length, 1)
    assert.equal(session.snapshot()[0].rows[0].raw, 'one|two')
    assert.equal(preview.rows[0].raw, 'one|')
  }
  finally {
    session.dispose()
  }
})

test('native candidate projects typed details and summary hierarchy separately from raw source', () => {
  const session = createSession()
  try {
    const first = '<details open>\n<summary title="a > b">**sum**</summary>\n\n'
    session.append(first)
    session.append('- one\n- two\n\n```js\nconst value = 1\n```\n</details>')
    session.append('\n')
    session.finish()
    const details = session.snapshot()[0]
    assert.equal(details.tag, 'details')
    assert.deepEqual(details.attrs, [['open', '']])
    assert.ok(details.raw.startsWith(first))
    assert.equal(details.children[0].tag, 'summary')
    assert.deepEqual(details.children[0].attrs, [['title', 'a > b']])
    assert.equal(details.children[0].children[0].type, 'strong')
    assert.ok(details.content.includes('<ul>'))
    assert.ok(details.content.includes('data-lang="js"'))
    assert.ok(details.content.endsWith('</details>\n'))
  }
  finally {
    session.dispose()
  }
})

test('open HTML wrapper includes the current child source and keeps a held summary closer in raw', () => {
  const session = createSession()
  const opening = '<div class="same">\nBETA\n\n<details>\n<summary>S'
  try {
    session.append(opening)
    session.append('</summ')
    const [held] = session.snapshot()
    assert.equal(held.raw, opening + '</summ')
    assert.equal(held.content, opening + '\n</div>')
    assert.equal(held.children.at(-1).children[0].content, '<summary>S</summ')

    const rest = 'ary>\n\nbody\n\n</details>\n\ntrailing copy:\nx1\n'
    session.append(rest)
    session.append('未结束')
    const [wrapper] = session.snapshot()
    assert.equal(wrapper.raw, opening + '</summ' + rest + '未结束')
    assert.equal(wrapper.content, wrapper.raw + '\n</div>')
    assert.equal(wrapper.children.at(-1).raw, 'trailing copy:\nx1\n未结束')
    assert.equal(wrapper.loading, true)
  }
  finally {
    session.dispose()
  }
})

test('native candidate retains typed custom tag children alongside the source payload', () => {
  const session = createSession({ parseOptions: { customHtmlTags: ['widget'] } })
  try {
    session.append('prefix <widget title="a > b">**value**</widget> tail\n\n<widget>\n**second**\n</widget>\n')
    session.finish()

    const [paragraph, block] = session.snapshot()
    const inline = paragraph.children.find(child => child.type === 'widget')
    assert.deepEqual(inline.attrs, [['title', 'a > b']])
    assert.equal(inline.content, '**value**')
    assert.equal(inline.children[0].type, 'strong')
    assert.equal(block.content, '**second**')
    assert.equal(block.raw, '<widget>\n**second**\n</widget>')
    assert.equal(block.children[0].type, 'strong')
  }
  finally {
    session.dispose()
  }
})

test('legacy custom block projection renders a same-line open payload on its own line', () => {
  const session = createSession({ parseOptions: { customHtmlTags: ['thinking'] } })
  try {
    session.append('<thinking>  text')
    assert.equal(session.snapshot()[0].content, 'text')
    assert.equal(session.snapshot()[0].raw, '<thinking>\ntext')
    assert.equal(session.snapshot()[0].loading, true)
  }
  finally {
    session.dispose()
  }
})

test('a split colon closer keeps the next HTML block outside its nested admonition', () => {
  const session = createSession()
  try {
    for (const character of '<details>\n::: note\n:::\n<!-- outside note -->\n</details>\n')
      session.append(character)
    session.finish()
    const [details] = session.snapshot()
    assert.deepEqual(details.children.map(node => node.type), ['admonition', 'html_block'])
    assert.deepEqual(details.children[0].children, [])
    assert.match(details.children[1].raw, /<!-- outside note -->/)
  }
  finally {
    session.dispose()
  }
})

test('native candidate projects the admonition title and actual child blocks', () => {
  const session = createSession()
  try {
    session.append('::: note Headline\n')
    session.append('**bold**\n:::\n')
    session.finish()
    const [note] = session.snapshot()
    assert.equal(note.type, 'admonition')
    assert.equal(note.kind, 'note')
    assert.equal(note.title, 'Headline')
    assert.equal(note.raw, ':::note Headline\n**bold**\n:::')
    assert.equal(note.children[0].children[0].type, 'strong')
  }
  finally {
    session.dispose()
  }
})

test('legacy paragraph raw follows a typed preview that holds an unfinished HTML tag', () => {
  const session = createSession()
  try {
    session.append('see <fo')
    const paragraph = session.snapshot()[0]
    assert.equal(paragraph.raw, 'see')
    assert.equal(paragraph.children[0].content, 'see')
  }
  finally {
    session.dispose()
  }
})

test('legacy closed same-line custom content keeps its inline view after streaming', () => {
  const session = createSession({ parseOptions: { customHtmlTags: ['widget'] } })
  try {
    session.append('<widget title="quote > works">')
    session.append('**value**</widget>')
    session.finish()
    const paragraph = session.snapshot()[0]
    assert.equal(paragraph.type, 'paragraph')
    assert.equal(paragraph.raw, '<widget title="quote > works">**value**</widget>')
    assert.deepEqual(paragraph.children[0].attrs, [['title', 'quote > works']])
    assert.equal(paragraph.children[0].children[0].type, 'strong')
  }
  finally {
    session.dispose()
  }
})

test('legacy custom closure keeps the preserved same-line suffix next to the typed inline node', () => {
  const session = createSession({ parseOptions: { customHtmlTags: ['widget'] } })
  try {
    const source = '<widget>{"text":"literal </widget> in json"}</widget>'
    session.append(source)
    session.finish()
    const [paragraph] = session.snapshot()
    assert.equal(paragraph.raw, source)
    assert.equal(paragraph.children[0].content, '{"text":"literal ')
    assert.ok(paragraph.children[1].content.startsWith(' in json'))
  }
  finally {
    session.dispose()
  }
})

test('legacy strict strong projection omits center on a typed literal fallback', () => {
  const session = createSession({ parseOptions: { requireClosingStrong: true } })
  try {
    session.append('prefix **soft')
    assert.deepEqual(session.snapshot()[0].children, [{
      type: 'text', content: 'prefix **soft', raw: 'prefix **soft',
    }])
  }
  finally {
    session.dispose()
  }
})

test('native HTML content preview retains the held closing prefix only in raw', () => {
  const session = createSession()
  try {
    session.append('<div data-kind="box">HTML 内容</d')
    const [block] = session.snapshot()
    assert.equal(block.raw, '<div data-kind="box">HTML 内容</d')
    assert.equal(block.content, '<div data-kind="box">HTML 内容\n</div>')
    assert.equal(block.loading, true)
  }
  finally {
    session.dispose()
  }
})

test('native external closer updates the inline raw without extending its paragraph source', () => {
  const session = createSession({ parseOptions: { customHtmlTags: ['widget'] } })
  try {
    session.append('- prefix <widget>{"a":"b"}\n\n')
    session.append('</widget>')
    session.finish()
    const [list] = session.snapshot()
    const [paragraph] = list.items[0].children
    const widget = paragraph.children.find(node => node.type === 'widget')
    assert.equal(widget.content, '{"a":"b"}')
    assert.ok(widget.raw.endsWith('</widget>'))
    assert.equal(paragraph.raw, 'prefix <widget>{"a":"b"}')
  }
  finally {
    session.dispose()
  }
})

test('locally closed paragraph keeps explicit math preview until the session ends', () => {
  const session = createSession()
  try {
    session.append('Equation $x +')
    session.append('\n\nNext')
    const [paragraph, following] = session.snapshot()
    assert.equal(paragraph.raw, 'Equation $x +')
    assert.equal(paragraph.children[1].type, 'math_inline')
    assert.equal(paragraph.children[1].loading, true)
    assert.equal(following.raw, 'Next')
    session.finish()
    assert.equal(session.snapshot()[0].raw, 'Equation $x +')
    assert.deepEqual(session.snapshot()[0].children, [
      { type: 'text', content: 'Equation $x +', raw: 'Equation $x +', center: false },
    ])
  }
  finally {
    session.dispose()
  }
})

test('native provisional fence uses the existing code block output', () => {
  const session = createSession()
  try {
    session.append('~~~dif')
    assert.deepEqual(session.snapshot(), [{
      type: 'code_block', language: 'dif', code: '', raw: '', diff: false, loading: true,
    }])
    session.append('f\n+line')
    const [block] = session.snapshot()
    assert.equal(block.diff, true)
    assert.equal(block.updatedCode, 'line')
  }
  finally {
    session.dispose()
  }
})

test('open fenced code displays held indentation from the current line', () => {
  const session = createSession({ parseOptions: { includeSourceMap: true } })
  const indented = createSession()
  try {
    session.append('```ts\nfirst\n')
    session.append('   ')
    const [draft] = session.snapshot()
    assert.equal(draft.code, 'first\n   ')
    assert.equal(draft.raw, 'first\n   ')
    assert.equal(draft.loading, true)
    assert.deepEqual(draft.sourceMap, { startLine: 0, endLine: 3 })

    session.append('`')
    assert.equal(session.snapshot()[0].loading, true)
    session.append('``')
    assert.equal(session.snapshot()[0].loading, false)
    assert.equal(session.snapshot()[0].code, 'first\n')

    indented.append('  ```ts\r\n    first\r\n')
    indented.append('\t')
    assert.equal(indented.snapshot()[0].code, '  first\n  ')
    indented.append('body')
    assert.equal(indented.snapshot()[0].code, '  first\n  body')
  }
  finally {
    session.dispose()
    indented.dispose()
  }
})

test('literal unmatched multi-backticks keep their text and legacy metadata', () => {
  const body = ' pauses at unclosed ``` fences and each fence stays atomic.'
  const legacy = createSession()
  const standard = createSession({ parseOptions: { markstream: false } })
  try {
    for (const session of [legacy, standard]) {
      session.append('The **streaming code-fence atomicity**' + body)
      session.finish()
    }
    assert.deepEqual(legacy.snapshot()[0].children[2], { type: 'text', content: body, raw: body })
    assert.deepEqual(standard.snapshot()[0].children[2], {
      type: 'text', content: body, raw: body, center: false,
    })
  }
  finally {
    legacy.dispose()
    standard.dispose()
  }

  const matched = createSession()
  try {
    matched.append('Show ``` the marker ``` here')
    matched.finish()
    assert.deepEqual(matched.snapshot()[0].children, [
      { type: 'text', content: 'Show ', raw: 'Show ', center: false },
      { type: 'inline_code', code: 'the marker', raw: 'the marker' },
      { type: 'text', content: ' here', raw: ' here', center: false },
    ])
  }
  finally {
    matched.dispose()
  }
})

test('complete unregistered self-closing components keep their literal text boundary', () => {
  const tag = '<PrereqChips :items="[\n  { text: \'Vue Quick Start\', link: \'/guide/quick-start\' },\n]" />'
  const legacy = createSession()
  const standard = createSession({ parseOptions: { markstream: false } })
  const escaped = createSession()
  try {
    for (const session of [legacy, standard]) {
      session.append('See ' + tag.slice(0, -2))
      session.append('/> here')
      session.finish()
    }
    assert.deepEqual(legacy.snapshot(), [{
      type: 'paragraph',
      raw: 'See ' + tag + ' here',
      children: [
        { type: 'text', content: 'See ', raw: 'See ', center: false },
        { type: 'text', content: tag + ' here', raw: tag + ' here' },
      ],
    }])
    assert.equal(standard.snapshot()[0].children[1].type, 'html_inline')

    escaped.append('\\<PrereqChips />\n\nvalue < tag / >')
    escaped.finish()
    assert.deepEqual(escaped.snapshot().map(node => node.children[0]), [
      { type: 'text', content: '<PrereqChips />', raw: '<PrereqChips />', center: false },
      { type: 'text', content: 'value < tag / >', raw: 'value < tag / >', center: false },
    ])
  }
  finally {
    legacy.dispose()
    standard.dispose()
    escaped.dispose()
  }
})

test('final literal asterisks in prose retain their spelling and legacy metadata', () => {
  const source = '- use rounded-* ([change](https://example.org/c))\n\n'
    + '**bold***\n\nplain \\* and &#42;'
  const legacy = createSession()
  const standard = createSession({ parseOptions: { markstream: false } })
  try {
    for (const session of [legacy, standard]) {
      session.append(source)
      session.finish()
    }
    const [list, bold, escaped] = legacy.snapshot()
    assert.equal(list.items[0].raw, 'use rounded-* ([change](https://example.org/c))')
    assert.deepEqual(list.items[0].children[0].children[0], {
      type: 'text', content: 'use rounded-* (', raw: 'use rounded-* (',
    })
    assert.equal(bold.children[0].type, 'strong')
    assert.deepEqual(bold.children[1], { type: 'text', content: '*', raw: '*', center: false })
    assert.deepEqual(escaped.children[0], {
      type: 'text', content: 'plain * and *', raw: 'plain * and *', center: false,
    })
    assert.equal(standard.snapshot()[0].items[0].children[0].children[0].center, false)
  }
  finally {
    legacy.dispose()
    standard.dispose()
  }
})

test('final legacy inline pre and code use typed tail children and preserve paragraph source', () => {
  const body = ' elements; ref ([change](https://example.org/c))'
  const source = 'plain <pre><code>' + body
  const legacy = createSession()
  const standard = createSession({ parseOptions: { markstream: false } })
  try {
    for (const session of [legacy, standard]) {
      session.append('plain <pre><co')
      session.append('de>' + body + '\n\nkeep <pre><code>x</code></pre> tail')
      session.finish()
    }
    const [unclosed, closed] = legacy.snapshot()
    assert.equal(unclosed.raw, source)
    assert.equal(unclosed.children.length, 2)
    const pre = unclosed.children[1]
    assert.equal(pre.raw, '<pre><code>' + body + '</pre>')
    assert.equal(pre.loading, true)
    assert.equal(pre.autoClosed, true)
    const [code] = pre.children
    assert.equal(code.raw, '<code>' + body + '</code>')
    assert.equal(code.loading, true)
    assert.equal(code.autoClosed, true)
    assert.equal(code.children[1].href, 'https://example.org/c')
    assert.equal(closed.children[1].loading, false)
    assert.equal(closed.children[2].content, ' tail')
    assert.equal(standard.snapshot()[0].children[1].raw, '<pre>')
  }
  finally {
    legacy.dispose()
    standard.dispose()
  }
})

test('legacy simple link metadata remains compact only with same-line inline content after it', () => {
  const session = createSession()
  try {
    session.append('## [label](https://a.test "title") tail\n\n[label](https://b.test)\nnext')
    session.finish()
    const [heading, paragraph] = session.snapshot()
    assert.deepEqual(heading.children[0], {
      type: 'link', href: 'https://a.test', title: 'title', text: 'label',
      children: [{ type: 'text', content: 'label', raw: 'label' }],
      raw: '[label](https://a.test)', loading: false, markup: '',
    })
    assert.deepEqual(paragraph.children[0].attrs, [['href', 'https://b.test']])
  }
  finally {
    session.dispose()
  }
})

test('resolved references use the same compact metadata as direct links on the same line', () => {
  const session = createSession()
  try {
    session.append('[label][id] tail\n\n[id]: https://a.test "title"')
    session.finish()
    assert.deepEqual(session.snapshot()[0].children, [
      {
        type: 'link', href: 'https://a.test', title: 'title', text: 'label',
        children: [{ type: 'text', content: 'label', raw: 'label' }],
        raw: '[label](https://a.test)', loading: false, markup: '',
      },
      { type: 'text', content: ' tail', raw: ' tail', center: false },
    ])
  }
  finally {
    session.dispose()
  }
})

test('the rejected unsafe link fallback keeps the source suffix after the first close', () => {
  const session = createSession({ parseOptions: { validateLink: href => !/^javascript:/i.test(href) } })
  const custom = createSession({
    factoryOptions: { markdownItOptions: { validateLink: () => true } },
    parseOptions: { validateLink: () => false },
  })
  try {
    session.append('[安全链接][safe] 与 [拒绝链接](javascript:alert((1))) tail (z)\n\n[safe]: https://a.test')
    session.finish()
    assert.deepEqual(session.snapshot()[0].children, [
      {
        type: 'link', href: 'https://a.test', title: '', text: '安全链接',
        children: [{ type: 'text', content: '安全链接', raw: '安全链接' }],
        raw: '[安全链接](https://a.test)', loading: false, markup: '',
      },
      { type: 'text', content: ' 与 拒绝链接)) tail (z)', raw: ' 与 拒绝链接)) tail (z)' },
    ])
    custom.append('[label](javascript:alert(1)) tail')
    custom.finish()
    assert.equal(custom.snapshot()[0].children[0].content, 'label tail')
  }
  finally {
    session.dispose()
    custom.dispose()
  }
})

test('legacy math map extends only for typed open Markstream blocks', () => {
  const session = createSession({ parseOptions: { includeSourceMap: true } })
  try {
    session.append('$$\nx +\n')
    let [block] = session.snapshot()
    assert.equal(block.loading, true)
    assert.deepEqual(block.sourceMap, { startLine: 0, endLine: 3 })
    session.append('$$\n')
    session.finish()
    ;[block] = session.snapshot()
    assert.equal(block.loading, false)
    assert.deepEqual(block.sourceMap, { startLine: 0, endLine: 3 })
  }
  finally {
    session.dispose()
  }
})

test('standalone link preview waits for a known destination while image links retain the typed prefix', () => {
  const standalone = createSession()
  const image = createSession()
  try {
    standalone.append('[链接](http')
    assert.deepEqual(standalone.snapshot()[0].children[0], {
      type: 'link', href: '', title: null, text: '链接',
      children: [{ type: 'text', content: '链接', raw: '链接' }],
      loading: true, markup: '',
    })
    standalone.append('s://example.com)')
    assert.equal(standalone.snapshot()[0].children[0].href, 'https://example.com')
    assert.equal(standalone.snapshot()[0].children[0].loading, false)

    image.append('[![alt](https://images.example/icon.png)](https:')
    assert.equal(image.snapshot()[0].children[0].href, 'https:')
    assert.equal(image.snapshot()[0].children[0].loading, true)
  }
  finally {
    standalone.dispose()
    image.dispose()
  }
})

test('link projection normalizes Unicode hostnames without changing userinfo, port or path', () => {
  const checked = new Set()
  const session = createSession({ parseOptions: { validateLink: href => {
    checked.add(href)
    return true
  } } })
  try {
    session.append('[链接](https://user:pass@例子.测试:8443/路径?x=1)')
    session.finish()
    const href = 'https://user:pass@xn--fsqu00a.xn--0zwm56d:8443/%E8%B7%AF%E5%BE%84?x=1'
    assert.equal(session.snapshot()[0].children[0].href, href)
    assert.ok(checked.has(href))
  }
  finally {
    session.dispose()
  }
})

test('an explicit draft link keeps the title that has arrived', () => {
  const session = createSession()
  try {
    const prefix = '[label](https://example.org/p "'
    session.append(prefix)
    assert.equal(session.snapshot()[0].children[0].title, '')
    session.append('标题')
    const draft = session.snapshot()[0]
    assert.equal(draft.raw, prefix + '标题')
    assert.deepEqual(draft.children[0], {
      type: 'link', href: 'https://example.org/p', title: '标题', text: 'label',
      children: [{ type: 'text', content: 'label', raw: 'label' }],
      loading: true, raw: '[label](https://example.org/p)', markup: '',
    })
    session.append('")')
    const closed = session.snapshot()[0].children[0]
    assert.equal(closed.loading, false)
    assert.equal(closed.title, '标题')
    assert.equal(closed.markup, '')
  }
  finally {
    session.dispose()
  }
})

test('a draft Markdown link waiting for its href preserves the legacy preceding text shape', () => {
  const session = createSession()
  const multiline = createSession()
  try {
    session.append('See [Guide](/gu')
    assert.deepEqual(session.snapshot()[0].children, [
      { type: 'text', content: 'See ', raw: 'See ' },
      {
        type: 'link', href: '', title: null, text: 'Guide',
        children: [{ type: 'text', content: 'Guide', raw: 'Guide' }],
        loading: true, markup: '',
      },
    ])
    session.append('ide)')
    assert.deepEqual(session.snapshot()[0].children[0], {
      type: 'text', content: 'See ', raw: 'See ', center: false,
    })
    assert.equal(session.snapshot()[0].children[1].href, '/guide')

    multiline.append('Overview\nSee [Guide](/gu')
    assert.deepEqual(multiline.snapshot()[0].children[0], {
      type: 'text', content: 'Overview\nSee ', raw: 'Overview\nSee ', center: false,
    })
  }
  finally {
    session.dispose()
    multiline.dispose()
  }
})

test('a loading image link after a completed link uses the image alt as its label', () => {
  const session = createSession()
  try {
    session.append('[first](https://example.com)\n[![Second](http')
    const early = session.snapshot()[0].children[2]
    assert.equal(early.text, 'Second')
    assert.deepEqual(early.children, [{
      type: 'image', src: '', alt: 'Second', title: null,
      raw: '![Second](http', loading: true,
    }])

    session.append('s://images.example/part')
    const [previous, newline, preview] = session.snapshot()[0].children
    assert.equal(previous.href, 'https://example.com')
    assert.equal(previous.loading, false)
    assert.equal(newline.content, '\n')
    assert.equal(preview.text, 'Second')
    assert.equal(preview.raw, '[Second]()')
    assert.equal(preview.loading, true)
    assert.deepEqual(preview.children, [{
      type: 'image', src: 'https://images.example/part', alt: 'Second', title: null,
      raw: '![Second](https://images.example/part)', loading: true,
    }])
    session.append(')](https://example.com/second)')
    const confirmed = session.snapshot()[0].children[2]
    assert.equal(confirmed.href, 'https://example.com/second')
    assert.equal(confirmed.loading, false)
    assert.equal(confirmed.children[0].loading, false)
  }
  finally {
    session.dispose()
  }
})

test('legacy final view can display a complete image under its unclosed outer URL', () => {
  const source = '[![流式图片](https://images.example/loading.png)](https://example.com/loading'
  const legacy = createSession()
  const standard = createSession({ parseOptions: { markstream: false } })
  const rejected = createSession({ parseOptions: { validateLink: href => !href.startsWith('https://example.com/') } })
  const closed = createSession()
  try {
    for (const session of [legacy, standard, rejected, closed]) {
      session.append(source.slice(0, -7))
      session.append(source.slice(-7) + (session === closed ? ')' : ''))
      session.finish()
    }
    const [paragraph] = legacy.snapshot()
    assert.equal(paragraph.raw, source)
    assert.deepEqual(paragraph.children, [{
      type: 'link', href: 'https://example.com/loading', title: null, text: '流式图片',
      children: [{
        type: 'image', src: 'https://images.example/loading.png', alt: '流式图片',
        title: null, raw: '流式图片', loading: false,
      }],
      raw: '[流式图片](https://example.com/loading)', loading: true, markup: '',
    }])
    assert.equal(standard.snapshot()[0].children[0].content, '[')
    assert.equal(rejected.snapshot()[0].children.some(node => node.type === 'link'), false)
    assert.equal(closed.snapshot()[0].children[0].loading, false)
  }
  finally {
    legacy.dispose()
    standard.dispose()
    rejected.dispose()
    closed.dispose()
  }
})

test('an image-link preview recognizes only the unescaped bracket after typed text', () => {
  for (const [prefix, visible] of [
    ['say "hello" ', 'say “hello” '],
    ['preceding \\* ', 'preceding * '],
  ]) {
    const session = createSession()
    try {
      session.append(prefix + '[![Second](https://images.example/part')
      const [before, link] = session.snapshot()[0].children
      assert.equal(before.content, visible)
      assert.equal(link.type, 'link')
      assert.equal(link.loading, true)
      assert.equal(link.text, 'Second')
    }
    finally {
      session.dispose()
    }
  }

  const escaped = createSession()
  const unconfirmed = createSession()
  try {
    escaped.append('literal \\[![Second](https://images.example/part')
    const [before, image] = escaped.snapshot()[0].children
    assert.equal(before.content, 'literal [')
    assert.equal(image.type, 'image')
    assert.equal(image.loading, true)
    unconfirmed.append('[![')
    assert.ok(unconfirmed.snapshot().every(node => node.children?.every(child => child.type !== 'link' && child.type !== 'image') ?? true))
  }
  finally {
    escaped.dispose()
    unconfirmed.dispose()
  }
})

test('confirmed HTML image under an anchor preserves raw markup and the parent link attributes', () => {
  const image = '<img src="/badge.svg" alt="badge" width="250" />'
  const anchor = `<a href="/docs" target="_blank">${image}</a>`
  const legacy = createSession()
  const ordinary = createSession({ factoryOptions: { markstream: false } })
  try {
    for (const session of [legacy, ordinary]) {
      session.append('badge ' + anchor)
      session.finish()
    }
    const link = legacy.snapshot()[0].children[1]
    assert.equal(link.type, 'link')
    assert.equal(link.text, image)
    assert.equal(link.raw, anchor)
    assert.deepEqual(link.attrs, [['href', '/docs'], ['target', '_blank']])
    assert.deepEqual(link.children, [{
      type: 'html_inline', tag: 'img', content: image, children: [], raw: image, loading: false,
    }])
    assert.deepEqual(ordinary.snapshot()[0].children[1].children[0].attrs, [
      ['src', '/badge.svg'], ['alt', 'badge'], ['width', '250'],
    ])
  }
  finally {
    legacy.dispose()
    ordinary.dispose()
  }
})

test('a line-leading HTML anchor waits for its withheld image child in the legacy view', () => {
  const opening = '<a href="/docs" target="_blank">'
  const session = createSession()
  const inline = createSession()
  try {
    session.append(opening + '<img src="/badge')
    assert.deepEqual(session.snapshot(), [{ type: 'paragraph', children: [], raw: opening }])
    const suffix = '.svg" /></a>'
    session.append(suffix)
    session.finish()
    const [paragraph] = session.snapshot()
    assert.equal(paragraph.raw, opening + '<img src="/badge' + suffix)
    assert.equal(paragraph.children[0].href, '/docs')
    assert.equal(paragraph.children[0].loading, false)
    assert.equal(paragraph.children[0].children[0].raw, '<img src="/badge.svg" />')

    inline.append('Badge ' + opening + '<img src="/badge')
    assert.equal(inline.snapshot()[0].children[0].content, 'Badge ')
    assert.equal(inline.snapshot()[0].children[1].tag, 'a')
  }
  finally {
    session.dispose()
    inline.dispose()
  }
})

test('an inline HTML anchor with a confirmed image stays loading until its real closer', () => {
  const prefix = 'Badge '
  const opening = '<a href="/guide" title="read" target="_blank">'
  const image = '<img src="/logo.svg" alt="Example">'
  const session = createSession()
  const rejected = createSession({ parseOptions: { validateLink: () => false } })
  try {
    session.append(prefix + opening + image.slice(0, -1))
    assert.equal(session.snapshot()[0].children.some(node => node.type === 'link'), false)
    session.append('>')
    const [paragraph] = session.snapshot()
    assert.equal(paragraph.raw, prefix + opening + image)
    const expected = {
      type: 'link', href: '/guide', title: 'read', text: image, raw: opening + image,
      attrs: [['href', '/guide'], ['title', 'read'], ['target', '_blank']],
      children: [{ type: 'html_inline', tag: 'img', content: image, children: [], raw: image, loading: false }],
      loading: true, markup: 'html',
    }
    assert.deepEqual(paragraph.children[1], expected)
    session.append('</a>')
    assert.deepEqual(session.snapshot()[0].children[1], {
      ...expected, raw: opening + image + '</a>', loading: false,
    })

    rejected.append(prefix + opening + image)
    assert.deepEqual(rejected.snapshot()[0].children.map(node => node.type), ['text', 'html_inline', 'html_inline'])
  }
  finally {
    session.dispose()
    rejected.dispose()
  }
})
