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
