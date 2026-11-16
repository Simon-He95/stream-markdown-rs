import { expect, it } from 'vitest'
import { getMarkdown } from '../../.reference/oracle/packages/markdown-parser/src'

it('retains an unknown footnote label in the legacy HTML renderer', () => {
  const markdown = getMarkdown('footnote-pending-boundaries')
  expect(markdown.render('[^unknown]')).toContain('[^unknown]')
})
