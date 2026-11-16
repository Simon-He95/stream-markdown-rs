import { isDeepStrictEqual } from 'node:util'
import type { GetMarkdownOptions, ParseOptions } from '../../.reference/oracle/packages/markdown-parser/src'
import * as reference from '../../.reference/oracle/packages/markdown-parser/src'
import { loadCandidate } from '../../scripts/load-candidate.mjs'

const candidate = process.env.TEST_PARSER_CANDIDATE ? await loadCandidate() : undefined

function createHandle(id = 'editor-default', factoryOptions?: GetMarkdownOptions) {
  return {
    id,
    factoryOptions,
    session: undefined as ReturnType<NonNullable<typeof candidate>['createSession']> | undefined,
    parseOptions: undefined as ParseOptions | undefined,
    source: '',
    final: false,
  }
}

function parseCandidate(source: string, handle: ReturnType<typeof createHandle>, options: ParseOptions = {}) {
  const { final = false, streamParse, reuseStableTopLevelNodes: _reuse, ...parseOptions } = options
  const canAppend = handle.session && streamParse !== false && !handle.final
    && source.startsWith(handle.source) && isDeepStrictEqual(parseOptions, handle.parseOptions)

  if (!canAppend) {
    handle.session?.dispose?.()
    handle.session = candidate!.createSession({ id: handle.id, factoryOptions: handle.factoryOptions, parseOptions })
    handle.source = ''
  }

  const session = handle.session!
  const chunk = source.slice(handle.source.length)
  if (chunk || !canAppend)
    session.append(chunk)
  if (final)
    session.finish()
  handle.source = source
  handle.final = final
  handle.parseOptions = parseOptions
  return session.snapshot()
}

// The original signatures keep the migrated assertions usable with the reference.
// The test handle exposes only behavior the candidate contract supports.
export const getMarkdown = (candidate ? createHandle : reference.getMarkdown) as typeof reference.getMarkdown
export const parseMarkdownToStructure = (candidate ? parseCandidate : reference.parseMarkdownToStructure) as typeof reference.parseMarkdownToStructure
