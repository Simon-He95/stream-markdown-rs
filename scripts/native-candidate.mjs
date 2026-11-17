// Synchronous test adapter for the candidate API in test/support/parser.ts.
import { Buffer } from 'node:buffer'
import { existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { MessageChannel, receiveMessageOnPort, Worker } from 'node:worker_threads'
import { projectBlocks } from './native-candidate-projection.mjs'

let transport
let nextSession = 0
const disposeCollectedSession = new FinalizationRegistry((session) => {
  transport?.worker.postMessage({ op: 'dispose', session, noReply: true })
})

function request(op, session, data) {
  if (!transport) {
    const executable = process.platform === 'win32' ? 'node_candidate.exe' : 'node_candidate'
    const binary = process.env.NATIVE_CANDIDATE_BIN
      ? resolve(process.env.NATIVE_CANDIDATE_BIN)
      : fileURLToPath(new URL(`../target/release/examples/${executable}`, import.meta.url))
    if (!existsSync(binary))
      throw new Error(`Missing ${binary}. Run cargo build --release --example node_candidate.`)
    const { port1, port2 } = new MessageChannel()
    const wakeup = new SharedArrayBuffer(Int32Array.BYTES_PER_ELEMENT)
    const signal = new Int32Array(wakeup)
    const worker = new Worker(new URL('./native-candidate-worker.mjs', import.meta.url), {
      workerData: { binary, port: port2, wakeup },
      transferList: [port2],
      execArgv: [],
    })
    worker.unref()
    port1.unref()
    transport = { worker, port: port1, signal }
  }
  const { worker, port, signal } = transport
  const previous = Atomics.load(signal, 0)
  worker.postMessage({ op, session, ...data })
  if (Atomics.wait(signal, 0, previous, 30_000) === 'timed-out') {
    transport = undefined
    worker.terminate().catch(() => {})
    throw new Error(`Timed out calling native Markdown candidate (${op})`)
  }
  const reply = receiveMessageOnPort(port)?.message
  if (!reply)
    throw new Error(`Missing reply from native Markdown candidate (${op})`)
  if (reply.error)
    throw new Error(reply.error)
  return reply.value
}

export function createSession({ id, factoryOptions, parseOptions = {} } = {}) {
  const session = ++nextSession
  const customTags = parseOptions.customHtmlTags ?? factoryOptions?.customHtmlTags ?? []
  const math = (parseOptions.enableMath ?? factoryOptions?.enableMath) !== false
  const footnotes = (parseOptions.enableFootnotes ?? factoryOptions?.enableFootnotes) !== false
  const factoryValidateLink = factoryOptions?.markdownItOptions?.validateLink
  const validateLink = parseOptions.validateLink ?? factoryValidateLink
  const markstream = parseOptions.markstream ?? factoryOptions?.markstream ?? true
  const detailMessageId = typeof id === 'string' && !factoryOptions?.i18n
    && !factoryOptions?.plugin?.length && !factoryOptions?.apply?.length
    && !parseOptions.preTransformTokens && !parseOptions.postTransformTokens ? id : undefined
  const fixIndentedCodeBlock = parseOptions.enableFixIndentedCodeBlock ?? factoryOptions?.enableFixIndentedCodeBlock ?? true
  request('create', session, { customTags, math, footnotes, markstream, requireClosingStrong: parseOptions.requireClosingStrong })
  const nodes = new Map()
  const roots = []
  const lineStarts = [0]
  let source = Buffer.alloc(0)
  let length = 0
  let pendingHighSurrogate = ''
  let cached
  let final = false
  let metrics = {}
  let disposed = false

  function appendSource(bytes) {
    const end = length + bytes.length
    if (source.length < end) {
      const grown = Buffer.allocUnsafe(Math.max(256, end, source.length * 2))
      source.copy(grown, 0, 0, length)
      source = grown
    }
    bytes.copy(source, length)
    for (let i = length; i < end; i++) {
      if (source[i] === 13)
        lineStarts.push(i + 1)
      else if (source[i] === 10) {
        if (source[i - 1] === 13)
          lineStarts[lineStarts.length - 1] = i + 1
        else
          lineStarts.push(i + 1)
      }
    }
    length = end
  }

  function update(op, data) {
    const response = request(op, session, data)
    metrics = response.metrics
    for (const change of response.changes)
      applyChange(nodes, roots, change)
    if (response.changes.length || op === 'finish' || markstream)
      cached = undefined
  }

  const adapter = {
    append(chunk) {
      if (final || disposed)
        throw new Error('Append requires an active native Markdown session')
      chunk = pendingHighSurrogate + chunk
      const last = chunk.charCodeAt(chunk.length - 1)
      pendingHighSurrogate = last >= 0xD800 && last <= 0xDBFF ? chunk.slice(-1) : ''
      if (pendingHighSurrogate)
        chunk = chunk.slice(0, -1)
      if (chunk) {
        const bytes = Buffer.from(chunk)
        appendSource(bytes)
        update('append', { chunk: bytes.toString() })
      }
    },
    finish() {
      if (final || disposed)
        return
      if (pendingHighSurrogate) {
        pendingHighSurrogate = ''
        adapter.append('\uFFFD')
      }
      update('finish')
      final = true
    },
    snapshot() {
      if (!cached) {
        const projected = projectBlocks(nodes, roots, {
          read: ([start, end]) => source.toString('utf8', start, end),
          lineStarts,
          length,
          final,
          markstream,
          detailMessageId,
          fixIndentedCodeBlock,
          customTags,
          parseOptions,
          validateLink,
          factoryValidateLink,
        })
        cached = parseOptions.postTransformNodes?.(projected) ?? projected
      }
      return cached
    },
    metrics: () => metrics,
    dispose() {
      if (disposed)
        return
      disposed = true
      disposeCollectedSession.unregister(adapter)
      request('dispose', session)
      nodes.clear()
      roots.length = 0
      source = Buffer.alloc(0)
      cached = []
    },
  }
  disposeCollectedSession.register(adapter, session, adapter)
  return adapter
}

function applyChange(nodes, roots, change) {
  if (change.op === 'insert') {
    const { block, parent } = change
    nodes.set(block.id, { ...block, parent, children: [] })
    const siblings = parent === null ? roots : nodes.get(parent).children
    siblings.push(block.id)
    return
  }

  const block = nodes.get(change.block?.id ?? change.id)
  switch (change.op) {
    case 'replace':
      if (change.block.kind === 'paragraph' && change.block.data.parsed) {
        if (block.data.fragments)
          block.rawFragments = block.data.fragments
      }
      else {
        delete block.rawFragments
      }
      Object.assign(block, change.block)
      delete block.previewInlines
      delete block.preview
      break
    case 'append_text': {
      const range = block.data.body
        ?? block.data.plain
        ?? block.data.content?.plain
        ?? block.data.content?.fragments?.at(-1)
        ?? block.data.fragments?.at(-1)
        ?? block.span
      range[1] = change.span[1]
      block.span[1] = change.span[1]
      break
    }
    case 'add_text_fragment': {
      const content = block.data.content ?? block.data
      content.fragments ??= [content.plain]
      delete content.plain
      content.fragments.push(change.span)
      block.span[1] = change.span[1]
      break
    }
    case 'add_raw_line':
      block.data.lines.push(change.line)
      block.span[1] = change.end
      break
    case 'add_row':
      block.data.rows.push(change.cells)
      block.span[1] = change.end
      break
    case 'replace_cell': {
      const width = block.data.header.length
      const column = change.index % width
      const row = Math.floor(change.index / width) - 1
      const cells = row < 0 ? block.data.header : block.data.rows[row]
      cells[column] = change.cell
      break
    }
    case 'status':
      block.status = change.status
      if (change.status !== 'open') {
        delete block.previewInlines
        delete block.preview
      }
      block.span[1] = change.end
      break
    case 'close_inline_element': {
      const base = block.data.content ?? block.data
      const closed = closeInlineElement(base.parsed, change.start, change.end)
      closeInlineElement(block.previewInlines?.parsed, change.start, change.end)
      if (!closed)
        throw new Error(`Native inline close references a missing opener at ${change.start}`)
      break
    }
    case 'remove': {
      const siblings = block.parent === null ? roots : nodes.get(block.parent).children
      siblings.splice(siblings.indexOf(block.id), 1)
      nodes.delete(block.id)
      break
    }
    case 'preview':
      block.preview = change.hint
      break
    case 'preview_inlines':
      block.previewInlines = change.nodes.length ? { parsed: change.nodes } : undefined
      break
    default:
      throw new Error(`Unsupported native Markdown update ${change.op}`)
  }
}


function closeInlineElement(nodes, start, end) {
  if (!nodes)
    return false
  for (let index = nodes.length - 1; index >= 0; index--) {
    const node = nodes[index]
    if (node.kind === 'custom_inline' && node.span[0] === start) {
      node.span[1] = end
      node.data.status = 'closed'
      return true
    }
    const children = node.data?.children ?? (
      Array.isArray(node.data) && typeof node.data[0] === 'object' ? node.data : null
    )
    if (closeInlineElement(children, start, end))
      return true
  }
  return false
}
