// Converts the native delta view to the legacy ParsedNode test contract.
// This module never tokenizes Markdown or calls the reference parser.
import { domainToASCII } from 'node:url'

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
  const projected = []
  for (let index = 0; index < ids.length; index++) {
    const block = nodes.get(ids[index])
    const view = projectBlock(block, nodes, context)
    const next = nodes.get(ids[index + 1])
    const end = block.span[1]
    if (context.markstream && block.kind === 'custom' && view.type === 'paragraph'
      && next?.kind === 'paragraph' && next.span[0] >= end
      && lineAt(context.lineStarts, end - 1) === lineAt(context.lineStarts, next.span[0])) {
      // A native custom block can split a physical line into two blocks.
      // The legacy view keeps the typed inline element and its text suffix together.
      const gap = context.read([end, next.span[0]])
      if (/^[ \t]*$/.test(gap)) {
        const tail = projectBlock(next, nodes, context)
        if (tail.type === 'paragraph') {
          const first = tail.children[0]
          if (gap && first?.type === 'text')
            tail.children[0] = { ...first, content: gap + first.content, raw: gap + first.raw }
          else if (gap)
            tail.children.unshift(textNode(gap))
          view.children.push(...tail.children)
          view.raw += gap + tail.raw
          Object.assign(view, sourceMap([block.span[0], next.span[1]], context))
          index++
        }
      }
    }
    projected.push(view)
  }
  return projected
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

function inlineRaw(content, range, context, end = Infinity) {
  if (content.plain)
    return readText(content.plain, context).trim()
  if (content.fragments)
    return normalize(content.fragments.map(span => context.read(span)).join('')).trim()
  const first = content.parsed.at(0)
  const last = content.parsed.at(-1)
  return first && last ? normalize(context.read([first.span[0], Math.min(last.span[1], end)])).trim() : range ?? ''
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

function allowsLink(href, context, kind = 'link') {
  const validation = context.validateLink?.(href)
  if (validation !== undefined)
    return validation
  const url = href.trimStart()
  if (/^file:/i.test(url))
    return kind === 'link' && /^file:\/\/\/(?![/\\])/i.test(url)
  return !/^(?:javascript|vbscript):/i.test(url)
}

function legacyLinkText(children, useImageAlt = false) {
  return children.map(child => useImageAlt && child.type === 'image' ? child.alt : child.content ?? child.raw).join('')
}

function markdownHref(href) {
  const url = /^((?:https?|ftp):\/\/)([^/?#]*)/iu.exec(href)
  if (url && /[^\x00-\x7f]/u.test(url[2])) {
    const [, scheme, authority] = url
    const hostStart = authority.lastIndexOf('@') + 1
    const hostAndPort = /^([^:]+)(:\d+)?$/u.exec(authority.slice(hostStart))
    if (hostAndPort) {
      const [, hostname, port = ''] = hostAndPort
      const ascii = domainToASCII(hostname)
      if (ascii)
        href = scheme + authority.slice(0, hostStart) + ascii + port + href.slice(url[0].length)
    }
  }
  return href.replace(/%(?![0-9a-fA-F]{2})|[^!-~]|[<>"`{}|\\^]/gu, encodeURIComponent)
}

function pendingLinkDestination(destination) {
  if (/^(?:https?|ftp):\/\/(?:\[[0-9a-f:.]+\]|[\p{L}\p{N}])/iu.test(destination)
    || /^(?:mailto|xmpp):[^@\s]+@[^@\s]/iu.test(destination))
    return destination
  if (/^www\.[^./?#\s]+\.[^./?#\s]+/iu.test(destination))
    return 'http://' + destination
  return ''
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

function needsLegacyCenter(text, context) {
  // The strict legacy strong fallback omits center on text already left literal by the parser.
  return !(context.markstream && !context.final
    && context.parseOptions.requireClosingStrong && text.includes('**'))
}

function projectInlines(content, context) {
  if (content.plain || content.fragments) {
    const text = inlineRaw(content, '', context)
    return text ? [textNode(text, needsLegacyCenter(text, context))] : []
  }
  return projectInlineNodes(content.parsed, context)
}

function legacyDelimitedText(children) {
  return children.map((child) => {
    if (child.type === 'link')
      return ''
    if (child.type === 'image')
      return child.raw
    return child.content ?? child.code ?? (
      child.children ? legacyDelimitedText(child.children) : child.raw
    )
  }).join('')
}

function customInlineNode(data, status, raw, context) {
  const open = status === 'open'
  return {
    type: data.name, tag: data.name, attrs: data.attrs,
    content: readText(data.body, context),
    children: projectInlineNodes(data.children, context),
    raw, loading: open && !context.final, autoClosed: open,
  }
}

function sameLineInlineSuffix(inlines, index, context) {
  const line = lineAt(context.lineStarts, inlines[index].span[1])
  if (inlines.some(node => (node.kind === 'code' || node.kind === 'code_preview')
    && lineAt(context.lineStarts, node.span[0]) === line))
    return false
  for (let i = index + 1; i < inlines.length; i++) {
    const node = inlines[i]
    if (node.kind === 'softbreak' || node.kind === 'hardbreak'
      || lineAt(context.lineStarts, node.span[0]) !== line)
      return false
    if (node.kind !== 'text' || readText(node.data, context).trim())
      return true
  }
  return false
}

function projectInlineNodes(inlines, context, insideLoadingLink = false) {
  const nodes = []
  for (let index = 0; index < inlines.length; index++) {
    const { kind, span, data } = inlines[index]
    const raw = normalize(context.read(span))
    if (kind === 'text' || kind === 'softbreak') {
      if (kind === 'text' && context.markstream && Array.isArray(data)
        && data[0] === span[0] && data[1] === span[1]
        && raw.startsWith('<') && raw.endsWith('/>') && HTML_START.exec(raw)?.[0] === raw) {
        // The complete demoted tag starts its own legacy text node.
        nodes.push(textNode(raw, false))
        continue
      }
      const literalCodeMarker = kind === 'text' && context.markstream && raw.includes('``')
        && (/(?<!\\)(?:\\\\)*(`+)/u.exec(raw)?.[1].length ?? 0) > 1
      const literalAsterisk = kind === 'text' && context.markstream && context.final
        && raw.includes('*') && /[^*]/u.test(raw) && /(?<!\\)(?:\\\\)*\*/u.test(raw)
      mergeText(nodes, textNode(kind === 'softbreak' ? '\n' : readText(data, context),
        kind !== 'softbreak' && !literalCodeMarker && !literalAsterisk))
      if (literalCodeMarker && nodes.at(-1)?.type === 'text')
        delete nodes.at(-1).center
    }
    else if (kind === 'hardbreak') {
      nodes.push({ type: 'hardbreak', raw: '\\\n' })
    }
    else if (kind === 'code' || kind === 'code_preview') {
      const previous = nodes.at(-1)
      if (kind === 'code_preview' && previous?.type === 'text')
        delete previous.center
      const code = readText(data, context)
      nodes.push({ type: 'inline_code', code, raw: code })
    }
    else if (kind === 'emphasis' || kind === 'strong' || kind === 'strikethrough') {
      const children = projectInlineNodes(data, context)
      const marker = kind === 'emphasis' ? '*' : kind === 'strong' ? '**' : '~~'
      nodes.push({ type: kind, children, raw: context.markstream ? marker + legacyDelimitedText(children) + marker : raw })
    }
    else if (kind === 'subscript' || kind === 'superscript') {
      const marker = kind === 'subscript' ? '~' : '^'
      const body = readText(data, context)
      nodes.push({ type: kind, children: [textNode(body)], raw: marker + body + marker })
    }
    else if (kind === 'highlight' || kind === 'insert') {
      const marker = kind === 'highlight' ? '==' : '++'
      const children = projectInlineNodes(data, context)
      nodes.push({ type: kind, children, raw: marker + displayText(children) + marker })
    }
    else if (kind === 'math_inline') {
      nodes.push(mathNode(kind, data.body, raw, data.loading, context))
    }
    else if (kind === 'footnote_reference') {
      const id = context.read(data.label)
      nodes.push({ type: kind, id, raw: `[^${id}]` })
    }
    else if (kind === 'reference') {
      nodes.push({ type: kind, id: context.read(data.label), raw })
    }
    else if (kind === 'link' || kind === 'image') {
      const destination = readText(data.destination, context)
      const href = markdownHref(destination)
      const title = data.title === null ? null : readText(data.title, context) || null
      const children = projectInlineNodes(data.children, context, kind === 'link' && data.loading)
      const allowed = allowsLink(href, context, kind)
      const simpleMarkdownLink = context.markstream && kind === 'link' && raw.startsWith('[')
        && children.length === 1 && children[0].type === 'text'
      if (kind === 'link' && data.loading) {
        const text = legacyLinkText(children, context.markstream)
        const hasImage = children.some(child => child.type === 'image')
        const ready = context.markstream && !hasImage ? pendingLinkDestination(destination) : destination
        const preparedHref = ready === destination ? href : markdownHref(ready)
        const visibleHref = allowed && (preparedHref === href || !preparedHref || allowsLink(preparedHref, context, kind))
          ? preparedHref
          : ''
        const linkified = Boolean(visibleHref && context.markstream && ready !== destination)
        const visibleChildren = children.map(child => child.type === 'text' ? textNode(child.content, false) : child)
        const waitingForHref = context.markstream && !hasImage && !ready
        const previous = nodes.at(-1)
        if (waitingForHref && previous?.type === 'text' && !previous.content.includes('\n'))
          delete previous.center
        nodes.push({
          type: kind,
          href: visibleHref,
          title: title ?? (waitingForHref || linkified || hasImage ? null : ''),
          text,
          children: visibleChildren,
          ...(linkified ? { attrs: [['href', visibleHref]] } : {}),
          ...(waitingForHref ? {} : { raw: `[${text}](${visibleHref})` }),
          loading: true,
          markup: linkified ? 'linkify' : '',
        })
      }
      else if (!allowed) {
        const text = displayText(children)
        if (simpleMarkdownLink && !context.factoryValidateLink && /^(?:javascript|vbscript):/i.test(href.trimStart())) {
          const afterLabel = normalize(context.read([data.children.at(-1).span[1], span[1]]))
          const closing = afterLabel.indexOf(')', 2)
          if (afterLabel.startsWith('](') && closing !== -1) {
            // Legacy plain-text recovery starts after the first literal close
            // when the Markdown URL is denied before it becomes a link.
            mergeText(nodes, textNode(text + afterLabel.slice(closing + 1), false))
            if (nodes.at(-1)?.type === 'text')
              delete nodes.at(-1).center
            continue
          }
        }
        mergeText(nodes, textNode(text, false))
      }
      else if (kind === 'image') {
        const first = data.children.at(0)
        const last = data.children.at(-1)
        const labelRaw = first && last ? normalize(context.read([first.span[0], last.span[1]])) : ''
        const nestedPreview = context.markstream && insideLoadingLink && data.loading
        const ready = nestedPreview ? pendingLinkDestination(destination) : destination
        const preparedSrc = ready === destination ? href : markdownHref(ready)
        const src = preparedSrc === href || !preparedSrc || allowsLink(preparedSrc, context, kind)
          ? preparedSrc
          : ''
        const imageRaw = nestedPreview ? raw + (src ? ')' : '') : labelRaw
        nodes.push({ type: 'image', src, alt: displayText(children), title, raw: imageRaw, loading: data.loading })
      }
      else {
        const markup = raw.startsWith('<') ? 'autolink' : raw.startsWith('[') ? '' : 'linkify'
        const compact =
          ((markup && /^<?https?:\/\//i.test(raw)) || simpleMarkdownLink)
          && sameLineInlineSuffix(inlines, index, context)
        if (compact) {
          const text = legacyLinkText(children)
          nodes.push({
            type: 'link', href, title: title ?? '', text, children: [textNode(text, false)],
            raw: `[${text}](${href})`, loading: false, markup,
          })
        }
        else {
          nodes.push(linkNode(href, title, children, markup))
        }
      }
    }
    else if (kind === 'custom_inline') {
      nodes.push(customInlineNode(data, data.status, raw, context))
    }
    else if (kind === 'html') {
      const header = htmlHeader(raw)
      if (header?.tag === 'br') {
        nodes.push({ type: 'hardbreak', raw })
        continue
      }
      if (context.markstream && header?.tag === 'img' && !context.customTags.includes('img')) {
        nodes.push({ type: 'html_inline', tag: 'img', content: raw, children: [], raw, loading: false })
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
      const tag = header?.tag
      const autoClosed = context.markstream && context.final && end === index && !header?.closed
        && (tag === 'pre' || tag === 'code')
      const href = header && attrValue(header.attrs, 'href')
      const previous = inlines[index - 1]
      const image = inlines[index + 1]
      const loadingAnchor = context.markstream && !context.final && tag === 'a' && href
        && end === index && !header.closed && previous?.kind === 'text'
        && lineAt(context.lineStarts, previous.span[0]) === lineAt(context.lineStarts, span[0])
        && readText(previous.data, context).trim()
        && image?.kind === 'html' && image.span[0] === span[1] && index + 2 === inlines.length
        && htmlHeader(context.read(image.span))?.tag === 'img' && allowsLink(href, context)
      const includeTail = autoClosed || loadingAnchor
      const last = includeTail ? inlines.length - 1 : end
      let children = projectInlineNodes(inlines.slice(index + 1, includeTail ? inlines.length : end), context)
      const source = last === index ? raw : normalize(context.read([span[0], inlines[last].span[1]]))
      const combined = autoClosed ? source + `</${tag}>` : source
      if (tag === 'a' && href && (end > index || loadingAnchor)) {
        if (loadingAnchor || allowsLink(href, context)) {
          const title = attrValue(header.attrs, 'title')
          const attrs = title === '' ? [...header.attrs, ['title', title]] : header.attrs
          const link = linkNode(href, title, children, 'html', combined, attrs)
          link.loading = Boolean(loadingAnchor)
          nodes.push(link)
        }
        else {
          mergeText(nodes, textNode(displayText(children), false))
        }
      }
      else {
        const custom = tag && context.customTags.includes(tag)
        if (context.markstream && !custom) {
          children = children.map(child => (
            (child.type === 'text' || child.type === 'reference') && /^\[\d+\]$/.test(child.raw)
              ? textNode(child.raw, false)
              : child
          ))
        }
        nodes.push({
          type: custom ? tag : 'html_inline',
          ...(tag && { tag, attrs: header.attrs }),
          content: custom ? combined.slice(raw.length, combined.lastIndexOf('</')) : combined,
          children,
          raw: combined,
          loading: autoClosed,
          autoClosed,
        })
      }
      index = last
    }
    else {
      nodes.push({ type: kind, raw, ...data })
    }
  }
  for (const node of nodes) {
    if (node.type === 'text' && !needsLegacyCenter(node.content, context))
      delete node.center
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
  const headers = ['diff ', 'index ', '--- ', '+++ ', '@@ ']
  const changedLine = body => !unified && body.startsWith(' ') && !body.startsWith('  ') ? ` ${body}` : body
  for (const [index, line] of lines.entries()) {
    if (loading && !endsWithNewline && index === lines.length - 1 && headers.some(prefix => prefix.startsWith(line)))
      continue
    if (/^(?:diff |index |--- |\+\+\+ |@@ )/.test(line))
      continue
    if (line.startsWith('-'))
      original.push(changedLine(line.slice(1)))
    else if (line.startsWith('+'))
      updated.push(changedLine(line.slice(1)))
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

function displayFence(block, context) {
  const { span, data } = block
  const info = data.info === null ? '' : context.read(data.info)
  let content = data.lines ? rawLines(data.lines, context) : readText(data.body, context)
  let loading = block.status === 'open' && !context.final
  let sourceSpan = span
  if (context.markstream && loading && block.parent === null && span[1] < context.length) {
    const lineEnd = context.lineStarts[lineAt(context.lineStarts, span[0]) + 1] ?? span[1]
    const opening = /^( {0,3})(`{3,}|~{3,})/.exec(context.read([span[0], lineEnd]))
    const tail = context.read([span[1], context.length])
    const pending = /^ {0,3}(`+|~+)[ \t]*$/.exec(tail)
    if (opening && pending && opening[2][0] === pending[1][0]) {
      loading = pending[1].length < opening[2].length
      if (loading)
        content = content.replace(/\n$/, '')
      sourceSpan = [span[0], context.length]
    }
    else if (opening && span[1] === context.lineStarts.at(-1)) {
      const indent = opening[1].length
      let columns = 0
      let offset = 0
      while (columns < indent && (tail[offset] === ' ' || tail[offset] === '\t')) {
        columns += tail[offset] === '\t' ? 4 - columns % 4 : 1
        offset++
      }
      content += ' '.repeat(Math.max(0, columns - indent)) + normalize(tail.slice(offset))
      sourceSpan = [span[0], context.length]
    }
  }
  const map = sourceMap(sourceSpan, context)
  return { ...codeBlock(info, content, loading), ...map, ...map.sourceMap }
}

function projectTableRow(values, alignments, header, context) {
  const cells = values.map((cell, column) => ({
    type: 'table_cell', header, children: projectInlines(cell, context),
    raw: inlineRaw(cell, '', context).replace(/(\\+)\|/g, (source, escapes) => (
      escapes.length % 2 ? `${escapes.slice(1)}|` : source
    )),
    align: alignments[column],
  }))
  return { type: 'table_row', cells, raw: cells.map(cell => cell.raw).join('|') }
}

function projectTable(data, loading, map, context, provisionalRow) {
  const header = projectTableRow(data.header, data.alignments, true, context)
  const rows = data.rows.map(row => projectTableRow(row, data.alignments, false, context))
  if (provisionalRow)
    rows.push(projectTableRow(provisionalRow, data.alignments, false, context))
  return { type: 'table', header, rows, raw: [header, ...rows].map(row => row.raw).join('\n'), loading, ...map }
}

function looseVmrFields(text) {
  const inner = text.slice(1, -1).trim()
  const pair = /\s*([_$A-Z][\w$-]*|"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')\s*:\s*("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|[^,{}[\]]*)\s*(,|$)/giy
  const fields = []
  let offset = 0
  while (offset < inner.length) {
    pair.lastIndex = offset
    const match = pair.exec(inner)
    if (!match)
      return
    const [, rawKey, rawValue] = match
    const key = vmrValue(rawKey)
    fields.push([String(key), vmrValue(rawValue.trim())])
    offset = pair.lastIndex
  }
  return Object.fromEntries(fields)
}

function vmrValue(text) {
  try {
    return JSON.parse(text)
  }
  catch {
    return text.startsWith("'") && text.endsWith("'") ? text.slice(1, -1) : text
  }
}

function vmrNode(block, nodes, context) {
  const { data, span, status } = block
  const children = projectChildren(nodes, block.children, context)
  const name = context.read(data.name)
  const args = data.args ? context.read(data.args) : undefined
  const payload = data.attributes ? context.read(data.attributes) : undefined
  let fields
  if (payload) {
    try {
      fields = JSON.parse(payload)
    }
    catch {
      fields = looseVmrFields(payload) ?? { attrs: payload }
    }
  }
  const attrs = args || fields && Object.keys(fields).length
    ? { ...(args && { args }), ...fields }
    : undefined
  let raw = '::: ' + name + (attrs ? ' ' + JSON.stringify(attrs) : '') + '\n'
  let bodyEnd = data.header[1]

  if (children.length) {
    const bodyStart = context.lineStarts[lineAt(context.lineStarts, data.header[0]) + 1] ?? data.header[1]
    for (let node = nodes.get(block.children.at(-1)); node; node = nodes.get(node.children.at(-1)))
      bodyEnd = Math.max(bodyEnd, node.span[1])
    let body = normalize(context.read([bodyStart, bodyEnd]))
    if (!body.endsWith('\n'))
      body += '\n'
    if (!body.endsWith('\n\n'))
      body += '\n'
    raw += body + '\n'
  }

  return {
    type: 'vmr_container', name, attrs, children, raw: raw + ':::',
    loading: status === 'open' && !context.final,
    ...sourceMap([span[0], Math.max(span[1], bodyEnd)], context),
  }
}

function escapeHtml(value) {
  return value.replace(/[&<>"']/g, char => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  })[char])
}

function renderDetailChild(node, inlineParagraph = false) {
  const children = () => (node.children ?? []).map(child => renderDetailChild(child)).join('')
  switch (node.type) {
    case 'text':
      return escapeHtml(node.content)
    case 'inline_code':
      return '<code>' + escapeHtml(node.code) + '</code>'
    case 'strong':
      return '<strong>' + children() + '</strong>'
    case 'emphasis':
      return '<em>' + children() + '</em>'
    case 'strikethrough':
      return '<s>' + children() + '</s>'
    case 'hardbreak':
      return '<br>\n'
    case 'link':
      return '<a href="' + escapeHtml(node.href) + '">' + children() + '</a>'
    case 'paragraph':
      return inlineParagraph ? children() : '<p>' + children() + '</p>\n'
    case 'heading':
      return '<h' + node.level + '>' + children() + '</h' + node.level + '>\n'
    case 'blockquote':
      return '<blockquote>\n' + children() + '</blockquote>\n'
    case 'list': {
      const tag = node.ordered ? 'ol' : 'ul'
      const start = node.ordered && node.start !== 1 ? ' start="' + node.start + '"' : ''
      const items = node.items.map(item => '<li>' + item.children.map(child => (
        renderDetailChild(child, item.children.length === 1)
      )).join('') + '</li>\n').join('')
      return '<' + tag + start + '>\n' + items + '</' + tag + '>\n'
    }
    case 'code_block':
      return '<div class="code-block" data-lang="' + escapeHtml(node.language) + '"><pre>'
        + escapeHtml(node.code) + '</pre></div>'
    case 'html_block':
      return node.tag === 'summary' && node.loading ? node.raw ?? node.content : node.content
    case 'html_inline':
      return node.content
    default:
      return escapeHtml(node.raw)
  }
}

function detailNode(block, nodes, raw, map, context) {
  const { status, data } = block
  const open = status === 'open'
  const summary = block.kind === 'summary'
  const attrs = data.attrs
  if (summary) {
    if (context.markstream && open && !context.final) {
      const visible = normalize(context.read([block.span[0], context.length]))
      if (/<\/[^>]*$/.test(visible))
        return { type: 'html_block', content: visible, tag: 'summary', loading: true }
      return {
        type: 'html_block', content: visible + '\n</summary>', raw: visible, tag: 'summary',
        attrs: attrs.length ? attrs : undefined, loading: true,
      }
    }
    const content = raw.trimEnd()
    return {
      type: 'html_block', tag: 'summary', content, raw: content,
      attrs: attrs.length ? attrs : undefined,
      loading: open && !context.final, children: projectInlines(data.content, context), ...map,
    }
  }

  const children = projectChildren(nodes, block.children, context)
  const opening = normalize(context.read(data.opening))
  const openerLine = lineAt(context.lineStarts, data.opening[0])
  const openerLineEnd = context.lineStarts[openerLine + 1]
  const last = nodes.get(block.children.at(-1))
  if (context.markstream && open && last?.kind === 'summary' && last.status === 'open') {
    const surfaceSpan = [block.span[0], context.length]
    raw = normalize(context.read(surfaceSpan))
    map = sourceMap(surfaceSpan, context)
  }
  if (open && !children.length && openerLineEnd > block.span[1]
    && !context.read([data.opening[1], openerLineEnd]).trim()) {
    const surfaceSpan = [block.span[0], openerLineEnd]
    raw = normalize(context.read(surfaceSpan))
    map = sourceMap(surfaceSpan, context)
  }
  const body = children.map((child, index) => renderDetailChild(
    child,
    openerLine === lineAt(context.lineStarts, nodes.get(block.children[index]).span[0]),
  )).join('')
  const closing = status === 'closed' ? raw.match(/<\/details[ \t]*>[ \t]*\n?$/i)?.[0] : null
  return {
    type: 'html_block', tag: 'details', attrs, children, raw,
    content: opening + body + (closing ?? '</details>'),
    loading: open && !context.final, ...map,
  }
}

function projectBlock(block, nodes, context) {
  const { kind, span, data, status } = block
  const open = status === 'open'
  const raw = normalize(context.read(span))
  const map = sourceMap(span, context)

  switch (kind) {
    case 'paragraph': {
      const preview = block.preview
      if (preview?.kind === 'table') {
        const previewMap = sourceMap([span[0], context.length], context)
        return projectTable(preview.data, preview.loading, previewMap, context)
      }
      const rawContent = block.rawFragments ? { fragments: block.rawFragments } : data
      let projectedRaw = inlineRaw(rawContent, raw.trim(), context, span[1])
      const parsed = block.previewInlines?.parsed
      const last = parsed?.at(-1)
      if (context.markstream && open && last && last.span[1] < span[1]) {
        const tail = context.read([last.span[1], span[1]])
        if (tail.trimStart().startsWith('<'))
          projectedRaw = normalize(context.read([data.plain?.[0] ?? span[0], last.span[1]]))
        if (!context.final && parsed.length === 1 && last.kind === 'html'
          && /^<img(?:[\s/]|$)/i.test(tail)) {
          const anchor = htmlHeader(projectedRaw)
          if (anchor?.tag === 'a' && !anchor.closed)
            return { type: kind, children: [], raw: projectedRaw, ...map }
        }
      }
      return { type: kind, children: projectInlines(block.previewInlines ?? data, context), raw: projectedRaw, ...map }
    }
    case 'heading': {
      const raw = inlineRaw(data.content, '', context, span[1])
      return { type: kind, level: data.level, text: raw, children: projectInlines(block.previewInlines ?? data.content, context), raw, ...map }
    }
    case 'code_block':
      return displayFence(block, context)
    case 'indented_code': {
      const content = rawLines(data.lines, context)
      const text = content.trim()
      const literal = /^(?:const|let|var)\b/.test(text) || /^<\/?[a-z][\w:.-]*(?:\s|>)/i.test(text)
      if (!context.markstream || !context.fixIndentedCodeBlock || data.lines.length > 1 || literal) {
        const code = content.endsWith('\n') ? content : content + '\n'
        return { type: 'code_block', language: '', code, raw: code, loading: false, ...map, ...map.sourceMap }
      }
      return { type: 'paragraph', children: text ? [textNode(text)] : [], raw: text }
    }
    case 'vmr_container':
      return vmrNode(block, nodes, context)
    case 'admonition': {
      const kind = data.kind
      const title = data.title ? readText(data.title, context) : kind[0].toUpperCase() + kind.slice(1)
      const children = projectChildren(nodes, block.children, context)
      const raw = `:::${kind} ${title}\n${children.map(child => child.raw).join('\n')}\n:::`
      return { type: 'admonition', kind, title, children, raw, ...map }
    }
    case 'thematic_break':
      return { type: kind, raw: '---', ...map }
    case 'math_block':
      if (map.sourceMap && context.markstream && open && !context.final)
        map.sourceMap.endLine++
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
      const row = block.preview?.kind === 'table_row' ? block.preview.cells : null
      const tableMap = row ? sourceMap([span[0], context.length], context) : map
      return projectTable(data, false, tableMap, context, row)
    }
    case 'custom': {
      const body = readText(data.body, context)
      if (data.promoted) {
        return {
          type: data.name, tag: data.name, attrs: data.attrs, content: body, raw,
          children: projectInlineNodes(data.children, context), loading: open && !context.final, ...map,
        }
      }
      const inlineRaw = raw.trimEnd()
      if (context.markstream && status === 'closed' && !inlineRaw.includes('\n')
        && context.read([data.body[1], span[1]]).startsWith('</')) {
        const inline = customInlineNode(data, status, inlineRaw, context)
        return { type: 'paragraph', children: [inline], raw: inlineRaw, ...map }
      }
      let content = context.markstream ? body.replace(/^\n/, '') : body
      let projectedRaw = raw
      let projectedMap = map
      if (context.markstream && status === 'closed')
        content = content.replace(/\n$/, '')
      const partialBlock = context.markstream && block.parent === null && status !== 'closed'
      if (partialBlock) {
        // The native node can hold a potential same-tag closer. Legacy raw displays those bytes.
        const suffix = open ? normalize(context.read([data.body[1], context.length])) : ''
        const sourceBody = body + suffix
        const onSameLine = /^[ \t]*\S/.test(sourceBody)
        const visibleBody = onSameLine ? sourceBody.replace(/^[ \t]+/, '') : sourceBody.replace(/^\n/, '')
        const pendingClose = '</' + data.name
        content = open && visibleBody.endsWith(pendingClose)
          ? visibleBody.slice(0, -pendingClose.length)
          : visibleBody
        const opening = normalize(context.read([span[0], data.body[0]]))
        projectedRaw = opening + (onSameLine ? '\n' + visibleBody : sourceBody)
        if (open)
          projectedMap = sourceMap([span[0], context.length], context)
      }
      const children = data.children?.length && !partialBlock && (!open || !context.markstream)
        ? projectInlineNodes(data.children, context)
        : undefined
      return {
        type: data.name, tag: data.name, attrs: data.attrs.length ? data.attrs : undefined,
        content, raw: projectedRaw, ...(children && { children }), loading: open && !context.final, ...projectedMap,
      }
    }
    case 'details':
    case 'summary':
      return detailNode(block, nodes, raw, map, context)
    case 'html_wrapper': {
      const tag = data.tag.toLowerCase()
      let visibleEnd = span[1]
      let rawEnd = visibleEnd
      if (context.markstream && open && !context.final) {
        const last = nodes.get(block.children.at(-1))
        if (last?.kind === 'paragraph' && last.status === 'open') {
          // The child can already have text beyond the wrapper's last complete line.
          visibleEnd = Math.max(visibleEnd, last.span[1])
          rawEnd = visibleEnd
        }
        if (last?.kind === 'details' && last.status === 'open') {
          const summary = nodes.get(last.children.at(-1))
          const close = '</summary'
          const remaining = context.length - rawEnd
          if (summary?.kind === 'summary' && summary.status === 'open' && summary.span[1] === rawEnd
            && remaining >= 2 && remaining <= close.length) {
            const tail = context.read([rawEnd, context.length]).toLowerCase()
            if (tail.startsWith('</') && close.startsWith(tail))
              rawEnd = context.length
          }
        }
      }
      const visible = visibleEnd === span[1] ? raw : readText([span[0], visibleEnd], context)
      const wrapperRaw = rawEnd === visibleEnd ? visible : readText([span[0], rawEnd], context)
      return {
        type: 'html_block', tag, raw: wrapperRaw,
        content: status === 'closed' ? visible : visible + `\n</${tag}>`,
        ...(data.attrs.length && { attrs: data.attrs }),
        loading: open && !context.final,
        children: projectChildren(nodes, block.children, context),
        ...(rawEnd === span[1] ? map : sourceMap([span[0], rawEnd], context)),
      }
    }
    case 'html_block': {
      const content = data.lines ? rawLines(data.lines, context) : raw
      const header = htmlHeader(content.trimStart())
      const loading = status === 'open' && !context.final && Boolean(header) && !header.closed
      const visible = block.preview?.kind === 'html_content'
        ? readText(block.preview.span, context)
        : content
      const display = context.markstream && loading
        ? visible + `\n</${header.tag}>`
        : visible
      return {
        type: kind, content: display, raw: content, tag: header?.tag ?? '',
        ...(header?.attrs.length && { attrs: header.attrs }),
        loading,
        ...map,
      }
    }
    default:
      return { type: kind, raw, ...data, ...map }
  }
}
