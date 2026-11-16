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

test('native candidate delivers Rust custom tag metadata only after a complete opener', () => {
  const session = createSession({ factoryOptions: { customHtmlTags: ['tool-call'] } })
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
