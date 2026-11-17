import { describe, expect, it } from 'vitest'
import { getMarkdown, parseMarkdownToStructure } from '../support/parser'

const markdown = [
  '# Title',
  '',
  'Paragraph text',
  '',
  '- one',
  '- two',
  '',
  '```ts',
  'const x = 1',
  '```',
  '',
  '---',
].join('\n')

describe('node source map metadata', () => {
  it('keeps node shapes unchanged by default', () => {
    const nodes = parseMarkdownToStructure(markdown, getMarkdown('source-map-default'), {
      final: true,
      streamParse: false,
    }) as any[]

    expect(nodes.map(node => node.sourceMap)).toEqual([
      undefined,
      undefined,
      undefined,
      undefined,
      undefined,
    ])
  })

  it('attaches opt-in source line ranges to top-level block nodes', () => {
    const nodes = parseMarkdownToStructure(markdown, getMarkdown('source-map-enabled'), {
      final: true,
      includeSourceMap: true,
      streamParse: false,
    }) as any[]

    expect(nodes.map(node => [node.type, node.sourceMap])).toEqual([
      ['heading', { startLine: 0, endLine: 1 }],
      ['paragraph', { startLine: 2, endLine: 3 }],
      ['list', { startLine: 4, endLine: 7 }],
      ['code_block', { startLine: 7, endLine: 10 }],
      ['thematic_break', { startLine: 11, endLine: 12 }],
    ])
    expect(nodes[3].startLine).toBe(7)
    expect(nodes[3].endLine).toBe(10)
  })

  it('attaches source maps to additional block types', () => {
    const nodes = parseMarkdownToStructure([
      '> Quote',
      '',
      '| A | B |',
      '| - | - |',
      '| 1 | 2 |',
      '',
      '$$',
      'x + y',
      '$$',
    ].join('\n'), getMarkdown('source-map-extra-blocks'), {
      final: true,
      includeSourceMap: true,
      streamParse: false,
    }) as any[]

    expect(nodes.map(node => [node.type, node.sourceMap])).toEqual([
      ['blockquote', { startLine: 0, endLine: 1 }],
      ['table', { startLine: 2, endLine: 5 }],
      ['math_block', { startLine: 6, endLine: 9 }],
    ])
  })

  it('attaches source maps to list and blockquote child blocks', () => {
    const nodes = parseMarkdownToStructure([
      '- item',
      '  - nested',
      '',
      '> quote',
      '> - quoted item',
    ].join('\n'), getMarkdown('source-map-nested-list-blockquote'), {
      final: true,
      includeSourceMap: true,
      streamParse: false,
    }) as any[]

    const list = nodes[0]
    const nestedList = list.items[0].children[1]
    const blockquote = nodes[1]
    const quotedList = blockquote.children[1]

    expect(list.sourceMap).toEqual({ startLine: 0, endLine: 3 })
    expect(list.items[0].sourceMap).toEqual({ startLine: 0, endLine: 3 })
    expect(list.items[0].children[0].sourceMap).toEqual({ startLine: 0, endLine: 1 })
    expect(nestedList.sourceMap).toEqual({ startLine: 1, endLine: 3 })
    expect(nestedList.items[0].sourceMap).toEqual({ startLine: 1, endLine: 3 })
    expect(nestedList.items[0].children[0].sourceMap).toEqual({ startLine: 1, endLine: 2 })

    expect(blockquote.sourceMap).toEqual({ startLine: 3, endLine: 5 })
    expect(blockquote.children[0].sourceMap).toEqual({ startLine: 3, endLine: 4 })
    expect(quotedList.sourceMap).toEqual({ startLine: 4, endLine: 5 })
    expect(quotedList.items[0].sourceMap).toEqual({ startLine: 4, endLine: 5 })
    expect(quotedList.items[0].children[0].sourceMap).toEqual({ startLine: 4, endLine: 5 })
  })
})
