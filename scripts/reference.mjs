import { getMarkdown, parseMarkdownToStructure } from '../.reference/oracle/packages/markdown-parser/src/index.ts'

export function createColdReader({ id, factoryOptions, parseOptions }) {
  const markdown = getMarkdown(id, factoryOptions)
  return (source, final) => parseMarkdownToStructure(source, markdown, {
    ...parseOptions,
    streamParse: false,
    final,
  })
}

export function parseCold(source, config, final) {
  return createColdReader(config)(source, final)
}

export function createSession({ id, factoryOptions, parseOptions }) {
  const markdown = getMarkdown(id, factoryOptions)
  let source = ''
  let nodes = []

  function update(final) {
    nodes = parseMarkdownToStructure(source, markdown, {
      streamParse: true,
      reuseStableTopLevelNodes: true,
      ...parseOptions,
      final,
    })
  }

  return {
    append(chunk) {
      source += chunk
      update(false)
    },
    finish() {
      update(true)
    },
    snapshot: () => nodes,
    dispose() {
      markdown.stream?.reset()
      source = ''
      nodes = []
    },
  }
}
