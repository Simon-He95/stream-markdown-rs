// Converts the native delta view to the legacy ParsedNode test contract.
// This module never tokenizes Markdown or calls the reference parser.
const VOID = new Set('area base br col embed hr img input link meta param source track wbr'.split(' '))
const HTML_START = /^<([A-Z][\w:.-]*)(\s(?:[^"'<>]|"[^"]*"|'[^']*')*)?\s*\/?>/i
const HTML_ATTRIBUTE = /([^\s=/>]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g

export function projectBlocks(nodes, roots, context) {
  const blocks = projectChildren(nodes, roots, context)
  const definitions = new Map()
  for (const node of blocks) {
    if (node.type === 'footnote' && !definitions.has(node.id))
      definitions.set(node.id, node)
  }
  if (!definitions.size)
    return blocks

  const visible = blocks.filter(node => node.type !== 'footnote')
  const notes = []
  const seen = new Set()
  function collectReferences(children) {
    for (const child of children) {
      if (child.type === 'footnote_reference' && definitions.has(child.id) && !seen.has(child.id)) {
        seen.add(child.id)
        notes.push(definitions.get(child.id))
      }
      collectReferences(child.children ?? child.items ?? child.cells ?? [])
      if (child.type === 'table')
        collectReferences([child.header, ...child.rows])
    }
  }
  collectReferences(visible)
  for (const note of notes)
    collectReferences(note.children)
  return [...visible, ...notes]
}

function projectChildren(nodes, ids, context) {
  return ids.map(id => projectBlock(nodes.get(id), nodes, context))
}

function lineAt(starts, offset) {
  let left = 0
  let right = starts.length
  while (left + 1 < right) {
    const mid = (left + right) >>> 1
    if (starts[mid] <= offset)
      left = mid
    else
      right = mid
  }
  return left
}

function sourceMap(span, { lineStarts, parseOptions }) {
  if (!parseOptions.includeSourceMap)
    return {}
  const [start, end] = span
  return { sourceMap: {
    startLine: lineAt(lineStarts, start),
    endLine: lineAt(lineStarts, Math.max(start, end - 1)) + 1,
  } }
}

function normalize(value) {
  return value.replace(/\r\n?/g, '\n').replaceAll('\0', '\uFFFD')
}

function textNode(content, center = true) {
  return { type: 'text', content, raw: content, ...(center && { center: false }) }
}

function readText(value, context) {
  return normalize(typeof value === 'string' ? value : context.read(value))
}

function rawLines(lines, context) {
  return normalize(lines.map(line => ' '.repeat(line.leading) + context.read(line.span)).join(''))
}

function inlineRaw(content, range, context) {
  if (content.plain)
    return readText(content.plain, context).trim()
  if (content.fragments)
    return normalize(content.fragments.map(span => context.read(span)).join('')).trim()
  const first = content.parsed.at(0)
  const last = content.parsed.at(-1)
  return first && last ? normalize(context.read([first.span[0], last.span[1]])).trim() : range ?? ''
}

function htmlHeader(raw) {
  const header = raw.match(HTML_START)
  if (!header)
    return
  const { 0: markup, 1: tag, 2: fields = '' } = header
  const attrs = Array.from(fields.matchAll(HTML_ATTRIBUTE), ([, name, quote, single, bare]) => [
    name, quote ?? single ?? bare ?? '',
  ])
  return { tag: tag.toLowerCase(), attrs, closed: markup.endsWith('/>') || VOID.has(tag.toLowerCase()) }
}

function displayText(nodes) {
  return nodes.map(node => (node.type === 'math_inline' ? node.raw : node.content) ?? node.code ?? node.alt ?? (
    node.children ? displayText(node.children) : node.raw
  )).join('')
}

function mergeText(nodes, node) {
  if (node.type !== 'text') {
    nodes.push(node)
    return
  }
  const previous = nodes.at(-1)
  if (previous?.type !== 'text') {
    if (node.content)
      nodes.push(node)
    return
  }
  previous.content += node.content
  previous.raw = previous.content
}

function allowsLink(href, context) {
  return context.validateLink?.(href)
    ?? !/^\s*(?:javascript|vbscript|file):/i.test(href)
}

function legacyLinkText(children) {
  return children.map(child => child.content ?? child.raw).join('')
}

function markdownHref(href) {
  return href.replace(/%(?![0-9a-fA-F]{2})|[^!-~]|[<>"`{}|\\^]/gu, encodeURIComponent)
}

function linkNode(href, title, children, markup, raw, attrs) {
  const text = legacyLinkText(children)
  return {
    type: 'link', href, title, text, children, loading: false,
    raw: raw ?? `[${text}](${href}${title ? ` "${title}"` : ''})`,
    attrs: attrs ?? [['href', href], ...(title === null ? [] : [['title', title]])],
    markup,
  }
}

function attrValue(attrs, name) {
  for (let i = attrs.length - 1; i >= 0; i--) {
    if (attrs[i][0].toLowerCase() === name)
      return attrs[i][1]
  }
  return null
}

function mathNode(type, body, raw, loading, context) {
  const delimiters = [
    ['\\[', '\\]'], ['\\(', '\\)'], ['$$', '$$'], ['[', ']'], ['(', ')'], ['$', '$'],
  ]
  const [open, close] = delimiters.find(([left]) => raw.startsWith(left)) ?? delimiters.at(-1)
  const markup = open === close ? open : open + close
  const content = readText(body, context).trim()
  return { type, content, raw: open + content + close, loading, markup }
}

function projectInlines(content, context) {
  if (content.plain || content.fragments) {
    const text = inlineRaw(content, '', context)
    return text ? [textNode(text)] : []
  }
  return projectInlineNodes(content.parsed, context)
}

function projectInlineNodes(inlines, context) {
  const nodes = []
  for (let index = 0; index < inlines.length; index++) {
    const { kind, span, data } = inlines[index]
    const raw = normalize(context.read(span))
    if (kind === 'text' || kind === 'softbreak') {
      mergeText(nodes, textNode(kind === 'softbreak' ? '\n' : readText(data, context), kind !== 'softbreak'))
    }
    else if (kind === 'hardbreak') {
      nodes.push({ type: 'hardbreak', raw: '\\\n' })
    }
    else if (kind === 'code') {
      const code = readText(data, context)
      nodes.push({ type: 'inline_code', code, raw: code })
    }
    else if (kind === 'emphasis' || kind === 'strong' || kind === 'strikethrough') {
      nodes.push({ type: kind, children: projectInlineNodes(data, context), raw })
    }
    else if (kind === 'math_inline') {
      nodes.push(mathNode(kind, data.body, raw, data.loading, context))
    }
    else if (kind === 'footnote_reference') {
      const id = context.read(data.label)
      nodes.push({ type: kind, id, raw: `[^${id}]` })
    }
    else if (kind === 'link' || kind === 'image') {
      const href = markdownHref(readText(data.destination, context))
      const title = data.title === null ? null : readText(data.title, context) || null
      const children = projectInlineNodes(data.children, context)
      if (!allowsLink(href, context)) {
        mergeText(nodes, textNode(displayText(children), false))
      }
      else if (kind === 'image') {
        const first = data.children.at(0)
        const last = data.children.at(-1)
        const raw = first && last ? normalize(context.read([first.span[0], last.span[1]])) : ''
        nodes.push({ type: 'image', src: href, alt: displayText(children), title, raw, loading: data.loading })
      }
      else {
        const markup = raw.startsWith('<') ? 'autolink' : raw.startsWith('[') ? '' : 'linkify'
        if (markup && /^<?https?:\/\//i.test(raw)) {
          const text = legacyLinkText(children)
          nodes.push({
            type: 'link', href, title: '', text, children: [textNode(text, false)],
            raw: `[${text}](${href})`, loading: false, markup,
          })
        }
        else {
          nodes.push(linkNode(href, title, children, markup))
        }
      }
    }
    else if (kind === 'html') {
      const header = htmlHeader(raw)
      if (header?.tag === 'br') {
        nodes.push({ type: 'hardbreak', raw })
        continue
      }
      let end = index
      if (header && !header.closed) {
        let depth = 1
        for (let lookahead = index + 1; lookahead < inlines.length; lookahead++) {
          if (inlines[lookahead].kind !== 'html')
            continue
          const candidate = context.read(inlines[lookahead].span)
          const close = /^<\/([\w:.-]+)\s*>/.exec(candidate)
          const opening = htmlHeader(candidate)
          if (close?.[1].toLowerCase() === header.tag && !--depth) {
            end = lookahead
            break
          }
          if (opening?.tag === header.tag && !opening.closed)
            depth++
        }
      }
      const children = projectInlineNodes(inlines.slice(index + 1, end), context)
      const combined = end > index ? normalize(context.read([span[0], inlines[end].span[1]])) : raw
      const tag = header?.tag
      const href = header && attrValue(header.attrs, 'href')
      if (tag === 'a' && href && end > index) {
        if (allowsLink(href, context)) {
          const title = attrValue(header.attrs, 'title')
          const attrs = title === '' ? [...header.attrs, ['title', title]] : header.attrs
          nodes.push(linkNode(href, title, children, 'html', combined, attrs))
        }
        else {
          mergeText(nodes, textNode(displayText(children), false))
        }
      }
      else {
        const custom = tag && context.customTags.includes(tag)
        nodes.push({
          type: custom ? tag : 'html_inline',
          ...(tag && { tag, attrs: header.attrs }),
          content: custom ? combined.slice(raw.length, combined.lastIndexOf('</')) : combined,
          children,
          raw: combined,
          loading: false,
          autoClosed: false,
        })
      }
      index = end
    }
    else {
      nodes.push({ type: kind, raw, ...data })
    }
  }
  return nodes
}

function codeBlock(info, raw, loading) {
  if (!info.startsWith('diff'))
    return { type: 'code_block', language: info, code: raw, raw, diff: false, loading }

  const language = info.includes(' ') ? info.slice(info.indexOf(' ') + 1) : ''
  const lines = raw.split('\n')
  const endsWithNewline = lines.at(-1) === ''
  if (endsWithNewline)
    lines.pop()
  const unified = lines.some(line => /^(?:diff |--- |\+\+\+ |@@ )/.test(line))
  const original = []
  const updated = []
  for (const line of lines) {
    if (/^(?:diff |index |--- |\+\+\+ |@@ )/.test(line))
      continue
    if (line.startsWith('-'))
      original.push(line.slice(1))
    else if (line.startsWith('+'))
      updated.push(line.slice(1))
    else {
      const context = unified && line.startsWith(' ') ? line.slice(1) : line
      original.push(context)
      updated.push(context)
    }
  }
  const originalCode = original.join('\n') + (!loading && endsWithNewline && original.length ? '\n' : '')
  const updatedCode = updated.join('\n') + (!loading && endsWithNewline && updated.length ? '\n' : '')
  return { type: 'code_block', language, code: updatedCode, raw, diff: true, loading, originalCode, updatedCode }
}

function projectTableRow(values, alignments, header, context) {
  const cells = values.map((cell, column) => ({
    type: 'table_cell', header, children: projectInlines(cell, context),
    raw: inlineRaw(cell, '', context), align: alignments[column],
  }))
  return { type: 'table_row', cells, raw: cells.map(cell => cell.raw).join('|') }
}

function projectBlock(block, nodes, context) {
  const { kind, span, data, open } = block
  const raw = normalize(context.read(span))
  const map = sourceMap(span, context)

  switch (kind) {
    case 'paragraph':
      return { type: kind, children: projectInlines(block.previewInlines ?? data, context), raw: inlineRaw(data, raw.trim(), context), ...map }
    case 'heading': {
      const raw = inlineRaw(data.content, '', context)
      return { type: kind, level: data.level, text: raw, children: projectInlines(block.previewInlines ?? data.content, context), raw, ...map }
    }
    case 'code_block': {
      const info = data.info === null ? '' : context.read(data.info)
      const content = data.lines ? rawLines(data.lines, context) : readText(data.body, context)
      return { ...codeBlock(info, content, open && !context.final), ...map, ...map.sourceMap }
    }
    case 'indented_code': {
      const text = rawLines(data.lines, context).trim()
      return { type: 'paragraph', children: text ? [textNode(text)] : [], raw: text }
    }
    case 'thematic_break':
      return { type: kind, raw: '---', ...map }
    case 'math_block':
      return { ...mathNode(kind, data.body, raw.trimStart(), open && !context.final, context), ...map }
    case 'footnote': {
      const id = context.read(data.label)
      const text = inlineRaw(data.content, '', context)
      const children = text ? [{
        type: 'paragraph', raw: text, children: [
          ...projectInlines(data.content, context),
          { type: 'footnote_anchor', id, raw: '' },
        ],
      }] : []
      return { type: kind, id, children, raw: `[^${id}]: ${text}` }
    }
    case 'list': {
      const items = projectChildren(nodes, block.children, context)
      return { type: kind, ordered: data.ordered, start: data.ordered ? data.start : undefined, items, raw: items.map(item => item.raw).join('\n'), ...map }
    }
    case 'list_item':
    case 'blockquote': {
      const children = projectChildren(nodes, block.children, context)
      if (data?.task !== undefined && data.task !== null && children[0]?.type === 'paragraph') {
        const marker = data.task ? '[x]' : '[ ]'
        children[0].children.unshift({ type: 'checkbox_input', checked: data.task, raw: marker })
        children[0].raw = `${marker} ${children[0].raw}`
      }
      return { type: kind, children, raw: children.map(child => child.raw).join(kind === 'blockquote' ? '\n' : ''), ...map }
    }
    case 'table': {
      const header = projectTableRow(data.header, data.alignments, true, context)
      const rows = data.rows.map(row => projectTableRow(row, data.alignments, false, context))
      return { type: kind, header, rows, raw: [header, ...rows].map(row => row.raw).join('\n'), loading: false, ...map }
    }
    case 'custom':
      return { type: data.name, tag: data.name, attrs: data.attrs.length ? data.attrs : undefined, content: readText(data.body, context), raw, loading: open && !context.final, ...map }
    case 'html_block': {
      const content = data.lines ? rawLines(data.lines, context) : raw
      const header = htmlHeader(content.trimStart())
      return {
        type: kind, content, raw: content, tag: header?.tag ?? '',
        ...(header?.attrs.length && { attrs: header.attrs }),
        loading: open && !context.final,
        ...map,
      }
    }
    default:
      return { type: kind, raw, ...data, ...map }
  }
}
