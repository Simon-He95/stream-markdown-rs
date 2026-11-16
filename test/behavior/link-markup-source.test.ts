import { describe, expect, it } from 'vitest'
import { links } from '../support/midstate-utils'
import { getMarkdown, parseMarkdownToStructure } from '../support/parser'

describe('link markup source', () => {
  it.each([
    ['bare https://example.com here', 'linkify'],
    ['www.example.com', 'linkify'],
    ['write to me@example.com today', 'linkify'],
    ['<https://example.com>', 'autolink'],
    ['<a href="https://example.com/x">html link</a>', 'html'],
    ['| a |\n|---|\n| [x](http://s) |', ''],
    ['- item [x](http://s)\n- two', ''],
  ])('marks the link origin for %s', (source, markup) => {
    const md = getMarkdown('behavior-link-markup-source')
    expect(links(parseMarkdownToStructure(source, md, { final: true })).map(node => node.markup)).toEqual([markup])
  })

  it('keeps the full attributes of terminal autolinks', () => {
    const md = getMarkdown('behavior-terminal-autolink-attrs')
    const source = '- Docs: https://example.com/docs\n- <https://example.com/>\n- https://example.org/ next\n'
    const nodes = links(parseMarkdownToStructure(source, md, { final: true }))
    expect(nodes.map(node => [node.markup, node.title, node.attrs])).toEqual([
      ['linkify', null, [['href', 'https://example.com/docs']]],
      ['autolink', null, [['href', 'https://example.com/']]],
      ['linkify', '', undefined],
    ])
  })
})
