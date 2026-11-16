// A spec-only HTML consumer of the native node API.
use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write as _,
    io::{self, BufRead, BufWriter, Read, Write as _},
    ops::Range,
    panic,
};

use stream_markdown_rs::{
    Alignment, Block, BlockData, BlockId, BlockStatus, Change, Inline, InlineContent, InlineKind,
    Options, Parser, Preview, RawLine, Span, Syntax, Text, parse,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = io::stdin().lock();
    let mut output = BufWriter::new(io::stdout().lock());
    let mut header = String::new();
    let mut bytes = Vec::new();

    loop {
        header.clear();
        if input.read_line(&mut header)? == 0 {
            break;
        }
        let mut fields = header.split_ascii_whitespace();
        let size: usize = fields.next().ok_or("missing example size")?.parse()?;
        let syntax = match fields.next() {
            Some("commonmark") => Syntax::CommonMark,
            Some("gfm") => Syntax::Gfm,
            _ => return Err("unknown example dialect".into()),
        };
        let legacy_gfm = fields.next() == Some("gfm-legacy");
        let tagfilter = fields.next() == Some("tagfilter");
        let streaming = fields.next() == Some("stream");
        bytes.resize(size, 0);
        input.read_exact(&mut bytes)?;
        let source = std::str::from_utf8(&bytes)?;
        let html = match panic::catch_unwind(|| {
            render(source, syntax, legacy_gfm, tagfilter, streaming)
        }) {
            Ok(Ok(html)) => html,
            Ok(Err(error)) => format!("[native parser error: {error}]"),
            Err(_) => "[native parser panicked]".to_owned(),
        };
        writeln!(output, "{}", html.len())?;
        output.write_all(html.as_bytes())?;
    }
    output.flush()?;
    Ok(())
}

fn render(
    source: &str,
    syntax: Syntax,
    legacy_gfm: bool,
    tagfilter: bool,
    streaming: bool,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut options = Options {
        syntax,
        ..Options::default()
    };
    if streaming {
        options.preview = Preview::Ai;
        let view = stream_view(source, options)?;
        return Ok(render_view(
            source,
            legacy_gfm,
            tagfilter,
            view.values().map(|(parent, block)| (*parent, block)),
        ));
    }
    let document = parse(source, &options)?;
    Ok(render_view(
        source,
        legacy_gfm,
        tagfilter,
        document
            .nodes
            .iter()
            .map(|block| (document.parent(block.id), block)),
    ))
}

fn render_view<'src>(
    source: &'src str,
    legacy_gfm: bool,
    tagfilter: bool,
    nodes: impl Iterator<Item = (Option<BlockId>, &'src Block)>,
) -> String {
    let mut children = HashMap::<Option<BlockId>, Vec<&Block>>::new();
    for (parent, block) in nodes {
        children.entry(parent).or_default().push(block);
    }
    let mut renderer = Renderer {
        source,
        legacy_gfm,
        tagfilter,
        html: String::new(),
        children: &children,
    };
    renderer.children(None, false);
    renderer.html
}

type StreamView = BTreeMap<BlockId, (Option<BlockId>, Block)>;

fn stream_view(source: &str, options: Options) -> Result<StreamView, Box<dyn std::error::Error>> {
    let chunks = source
        .char_indices()
        .map(|(start, character)| &source[start..start + character.len_utf8()]);
    stream_view_chunks(chunks, options)
}

fn stream_view_chunks<'src>(
    chunks: impl Iterator<Item = &'src str>,
    options: Options,
) -> Result<StreamView, Box<dyn std::error::Error>> {
    let mut parser = Parser::new(options);
    let mut changes = Vec::new();
    let mut blocks = BTreeMap::new();
    for chunk in chunks {
        parser.append(chunk, &mut changes)?;
        apply_changes(&mut blocks, &mut changes)?;
    }
    parser.finish(&mut changes)?;
    apply_changes(&mut blocks, &mut changes)?;
    if blocks
        .values()
        .any(|(_, block)| block.status == BlockStatus::Open)
    {
        return Err("stream finish left a block open".into());
    }
    Ok(blocks)
}

fn apply_changes(blocks: &mut StreamView, changes: &mut Vec<Change>) -> Result<(), String> {
    for change in changes.drain(..) {
        match change {
            Change::Insert { parent, block } => {
                if let Some(parent) = parent
                    && !blocks.contains_key(&parent)
                {
                    return Err(format!(
                        "inserted block {:?} under missing parent {parent:?}",
                        block.id
                    ));
                }
                let id = block.id;
                if blocks.insert(id, (parent, block)).is_some() {
                    return Err(format!("inserted existing block {id:?}"));
                }
            }
            Change::Replace(block) => {
                let id = block.id;
                *existing_block(blocks, id)? = block;
            }
            Change::Remove { id } => {
                if blocks.remove(&id).is_none() {
                    return Err(format!("removed unknown block {id:?}"));
                }
            }
            Change::SetStatus { id, status, end } => {
                let block = existing_block(blocks, id)?;
                block.status = status;
                block.span.end = end;
            }
            Change::AppendText { id, span } => {
                let block = existing_block(blocks, id)?;
                let body = match &mut block.data {
                    BlockData::Paragraph(InlineContent::Plain(body))
                    | BlockData::Footnote {
                        content: InlineContent::Plain(body),
                        ..
                    }
                    | BlockData::Heading {
                        content: InlineContent::Plain(body),
                        ..
                    }
                    | BlockData::Summary {
                        content: InlineContent::Plain(body),
                        ..
                    }
                    | BlockData::Math {
                        body: Text::Source(body),
                    }
                    | BlockData::CodeBlock { body, .. } => body,
                    BlockData::Paragraph(InlineContent::Fragments(parts)) => parts
                        .last_mut()
                        .ok_or_else(|| format!("appended to empty text fragments in {id:?}"))?,
                    BlockData::Html => &mut block.span,
                    BlockData::CustomTag(element) => &mut element.body,
                    _ => return Err(format!("appended to non-text block {id:?}")),
                };
                if body.end != span.start {
                    return Err(format!("noncontiguous text append to {id:?}"));
                }
                body.end = span.end;
                block.span.end = span.end;
            }
            Change::AddTextFragment { id, span } => {
                let block = existing_block(blocks, id)?;
                let BlockData::Paragraph(content) = &mut block.data else {
                    return Err(format!("added a text fragment to non-text block {id:?}"));
                };
                match content {
                    InlineContent::Plain(first) => {
                        *content = InlineContent::Fragments(Box::new(vec![*first, span]));
                    }
                    InlineContent::Fragments(parts) => parts.push(span),
                    _ => return Err(format!("added a text fragment to parsed block {id:?}")),
                }
                block.span.end = span.end;
            }
            Change::AddRawLine { id, line, end } => {
                let block = existing_block(blocks, id)?;
                match &mut block.data {
                    BlockData::SegmentedHtml(lines) | BlockData::IndentedCode { lines } => {
                        lines.push(line)
                    }
                    BlockData::SegmentedCode(code) => code.lines.push(line),
                    _ => return Err(format!("added a raw line to unsupported block {id:?}")),
                }
                block.span.end = end;
            }
            Change::ReplaceTableCell { id, index, content } => {
                let block = existing_block(blocks, id)?;
                let BlockData::Table(table) = &mut block.data else {
                    return Err(format!("replaced a cell in non-table block {id:?}"));
                };
                table.replace_cell(index, content);
            }
            Change::AddRow { id, end, cells } => {
                let block = existing_block(blocks, id)?;
                let BlockData::Table(table) = &mut block.data else {
                    return Err(format!("added a table row to non-table block {id:?}"));
                };
                table.add_row(cells);
                block.span.end = end;
            }
            Change::CloseInlineElement { id, start, end } => {
                let block = existing_block(blocks, id)?;
                let content = match &mut block.data {
                    BlockData::Paragraph(content) | BlockData::Heading { content, .. } => content,
                    _ => panic!("invalid inline closer target: {id:?}"),
                };
                assert!(content.close_inline_element(start, end));
            }
            Change::Preview { .. } | Change::PreviewInlines { .. } => {}
        }
    }
    Ok(())
}

fn existing_block(blocks: &mut StreamView, id: BlockId) -> Result<&mut Block, String> {
    blocks
        .get_mut(&id)
        .map(|(_, block)| block)
        .ok_or_else(|| format!("updated unknown block {id:?}"))
}

struct Renderer<'src> {
    source: &'src str,
    legacy_gfm: bool,
    tagfilter: bool,
    html: String,
    children: &'src HashMap<Option<BlockId>, Vec<&'src Block>>,
}

impl Renderer<'_> {
    fn children(&mut self, parent: Option<BlockId>, tight: bool) {
        let children = self.children.get(&parent).map(Vec::as_slice).unwrap_or(&[]);
        for block in children {
            self.block(block, tight);
        }
    }

    fn block(&mut self, block: &Block, tight_item: bool) {
        match &block.data {
            BlockData::Paragraph(content) => {
                self.html.push_str("<p>");
                self.inline_content(content);
                self.html.push_str("</p>\n");
            }
            BlockData::Heading { level, content } => {
                write!(self.html, "<h{level}>").unwrap();
                self.inline_content(content);
                writeln!(self.html, "</h{level}>").unwrap();
            }
            BlockData::CodeBlock { body, .. } => {
                self.code(
                    block.data.code_info(),
                    (body.start != body.end).then_some(RawLine {
                        span: *body,
                        leading_spaces: 0,
                    }),
                );
            }
            BlockData::SegmentedCode(code) => {
                self.code(code.info, code.lines.iter().copied());
            }
            BlockData::IndentedCode { lines } => self.code(None, lines.iter().copied()),
            BlockData::ThematicBreak => self.html.push_str("<hr />\n"),
            BlockData::Quote => {
                self.html.push_str("<blockquote>\n");
                self.children(Some(block.id), false);
                self.html.push_str("</blockquote>\n");
            }
            BlockData::Vmr(_) | BlockData::Admonition(_) => self.children(Some(block.id), false),
            BlockData::Details(opening) => {
                let raw = &self.source[Range::from(opening.span)];
                self.raw_html(raw);
                if !raw.ends_with("/>") {
                    self.children(Some(block.id), false);
                    if block.status == BlockStatus::Closed {
                        self.html.push_str("</details>");
                    }
                }
            }
            BlockData::HtmlWrapper(_) => self.raw_html(&self.source[Range::from(block.span)]),
            BlockData::Summary { content, .. } => {
                let opening = block.summary_opening().unwrap();
                let raw = &self.source[Range::from(opening.span)];
                self.raw_html(raw);
                if !raw.ends_with("/>") {
                    self.inline_content(content);
                    if block.status == BlockStatus::Closed {
                        self.html.push_str("</summary>");
                    }
                }
            }

            BlockData::List {
                ordered,
                start,
                tight,
            } => {
                let tag = if *ordered {
                    if *start == 1 {
                        self.html.push_str("<ol>\n");
                    } else {
                        writeln!(self.html, "<ol start=\"{start}\">").unwrap();
                    }
                    "ol"
                } else {
                    self.html.push_str("<ul>\n");
                    "ul"
                };
                self.children(Some(block.id), *tight);
                writeln!(self.html, "</{tag}>").unwrap();
            }
            BlockData::ListItem { task } => self.list_item(block.id, tight_item, *task),
            BlockData::Html => self.html_block(&self.source[Range::from(block.span)]),
            BlockData::SegmentedHtml(lines) => {
                let mut raw = String::new();
                for line in lines {
                    raw.extend(std::iter::repeat_n(' ', usize::from(line.leading_spaces)));
                    raw.push_str(&self.source[Range::from(line.span)]);
                }
                self.html_block(&raw);
            }
            BlockData::Table(table) => {
                let alignments = table.alignments();
                let header = table.header();
                let rows = table.rows();
                self.html.push_str("<table>\n<thead>\n");
                self.row("th", alignments, header);
                self.html.push_str("</thead>\n");
                if rows.len() != 0 {
                    self.html.push_str("<tbody>\n");
                    for row in rows {
                        self.row("td", alignments, row);
                    }
                    self.html.push_str("</tbody>\n");
                }
                self.html.push_str("</table>\n");
            }
            BlockData::CustomTag(_) => self.raw_html(&self.source[Range::from(block.span)]),
            BlockData::Math { body } => {
                escape(&mut self.html, body.as_str(self.source));
            }
            BlockData::Footnote { content, .. } => self.inline_content(content),
            BlockData::Extension { .. } | BlockData::ExtensionWithFields { .. } => {
                escape(&mut self.html, &self.source[Range::from(block.span)])
            }
        }
    }

    fn code(&mut self, info: Option<Span>, lines: impl IntoIterator<Item = RawLine>) {
        self.html.push_str("<pre><code");
        if let Some(info) = info {
            let info = html_escape::decode_html_entities(&self.source[Range::from(info)]);
            if let Some(language) = info.split_ascii_whitespace().next() {
                self.html.push_str(" class=\"language-");
                escape(&mut self.html, &unescape_punctuation(language));
                self.html.push('"');
            }
        }
        self.html.push('>');
        for line in lines {
            self.html
                .extend(std::iter::repeat_n(' ', usize::from(line.leading_spaces)));
            let line = &self.source[Range::from(line.span)];
            escape(&mut self.html, line);
            if !line.ends_with(['\n', '\r']) {
                self.html.push('\n');
            }
        }
        self.html.push_str("</code></pre>\n");
    }

    fn list_item(&mut self, id: BlockId, tight: bool, task: Option<bool>) {
        let children = self
            .children
            .get(&Some(id))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        self.html.push_str("<li>");
        if let Some(checked) = task {
            self.html.push_str("<input ");
            if checked {
                self.html.push_str("checked=\"\" ");
            }
            self.html.push_str("disabled=\"\" type=\"checkbox\"> ");
        }
        if children
            .first()
            .is_some_and(|block| !tight || !matches!(block.data, BlockData::Paragraph(_)))
        {
            self.html.push('\n');
        }
        for (index, child) in children.iter().enumerate() {
            if tight && let BlockData::Paragraph(content) = &child.data {
                self.inline_content(content);
                if index + 1 < children.len() {
                    self.html.push('\n');
                }
            } else {
                self.block(child, false);
            }
        }
        self.html.push_str("</li>\n");
    }

    fn row(&mut self, tag: &str, alignments: &[Alignment], cells: &[InlineContent]) {
        self.html.push_str("<tr>\n");
        for (index, alignment) in alignments.iter().enumerate() {
            write!(self.html, "<{tag}").unwrap();
            let value = match alignment {
                Alignment::Left => "left",
                Alignment::Center => "center",
                Alignment::Right => "right",
                Alignment::None => "",
            };
            if !value.is_empty() {
                write!(self.html, " align=\"{value}\"").unwrap();
            }
            self.html.push('>');
            if let Some(cell) = cells.get(index) {
                self.inline_content(cell);
            }
            writeln!(self.html, "</{tag}>").unwrap();
        }
        self.html.push_str("</tr>\n");
    }

    fn inline_content(&mut self, content: &InlineContent) {
        match content {
            InlineContent::Plain(span) => escape(&mut self.html, &self.source[Range::from(*span)]),
            InlineContent::Parsed(nodes) => self.inlines(nodes),
            InlineContent::Fragments(spans) => {
                for span in spans.iter() {
                    escape(&mut self.html, &self.source[Range::from(*span)]);
                }
            }
        }
    }

    fn inlines(&mut self, nodes: &[Inline]) {
        self.inline_children(nodes, false);
    }

    fn inline_children(&mut self, nodes: &[Inline], parent_is_strong: bool) {
        for inline in nodes {
            match &inline.kind {
                InlineKind::Text(text) | InlineKind::Math { body: text, .. } => {
                    escape(&mut self.html, text.as_str(self.source))
                }
                InlineKind::Code(text) | InlineKind::CodePreview(text) => {
                    self.html.push_str("<code>");
                    escape(&mut self.html, text.as_str(self.source));
                    self.html.push_str("</code>");
                }
                InlineKind::Emphasis(children) => self.inline_tag("em", children),
                InlineKind::Strong(children) => {
                    // cmark-gfm 0.29 omits wrappers whose direct parent is Strong.
                    let render_tag = !self.legacy_gfm || !parent_is_strong;
                    if render_tag {
                        self.html.push_str("<strong>");
                    }
                    self.inline_children(children, true);
                    if render_tag {
                        self.html.push_str("</strong>");
                    }
                }
                InlineKind::Strikethrough(children) => self.inline_tag("del", children),
                InlineKind::Highlight(children) => self.inline_tag("mark", children),
                InlineKind::Insert(children) => self.inline_tag("ins", children),
                InlineKind::Subscript(text) | InlineKind::Superscript(text) => {
                    let tag = if matches!(inline.kind, InlineKind::Subscript(_)) {
                        "sub"
                    } else {
                        "sup"
                    };
                    write!(self.html, "<{tag}>").unwrap();
                    escape(&mut self.html, text.as_str(self.source));
                    write!(self.html, "</{tag}>").unwrap();
                }
                InlineKind::Link(link) | InlineKind::LinkPreview(link) => {
                    self.html.push_str("<a href=\"");
                    escape_url(&mut self.html, link.destination.as_str(self.source));
                    self.html.push('"');
                    if let Some(title) = &link.title {
                        self.html.push_str(" title=\"");
                        escape(&mut self.html, title.as_str(self.source));
                        self.html.push('"');
                    }
                    self.html.push('>');
                    self.inlines(&link.children);
                    self.html.push_str("</a>");
                }
                InlineKind::Image(link) | InlineKind::ImagePreview(link) => {
                    self.html.push_str("<img src=\"");
                    escape_url(&mut self.html, link.destination.as_str(self.source));
                    self.html.push_str("\" alt=\"");
                    self.image_alt(&link.children);
                    self.html.push('"');
                    if let Some(title) = &link.title {
                        self.html.push_str(" title=\"");
                        escape(&mut self.html, title.as_str(self.source));
                        self.html.push('"');
                    }
                    self.html.push_str(" />");
                }
                InlineKind::Html | InlineKind::CustomTag(_) => {
                    self.raw_html(&self.source[Range::from(inline.span)])
                }
                InlineKind::Extension(_) | InlineKind::ExtensionWithFields { .. } => {
                    escape(&mut self.html, &self.source[Range::from(inline.span)])
                }
                InlineKind::SoftBreak => self.html.push('\n'),
                InlineKind::HardBreak => self.html.push_str("<br />\n"),
                InlineKind::NumericReference(_) => {
                    escape(&mut self.html, &self.source[Range::from(inline.span)])
                }
                InlineKind::FootnoteReference { label, .. } => {
                    escape(&mut self.html, &self.source[Range::from(*label)])
                }
            }
        }
    }

    fn inline_tag(&mut self, tag: &str, children: &[Inline]) {
        write!(self.html, "<{tag}>").unwrap();
        self.inlines(children);
        write!(self.html, "</{tag}>").unwrap();
    }

    fn image_alt(&mut self, nodes: &[Inline]) {
        for inline in nodes {
            match &inline.kind {
                InlineKind::Text(text)
                | InlineKind::Code(text)
                | InlineKind::CodePreview(text)
                | InlineKind::Subscript(text)
                | InlineKind::Superscript(text)
                | InlineKind::Math { body: text, .. } => {
                    escape(&mut self.html, text.as_str(self.source))
                }
                InlineKind::Emphasis(children)
                | InlineKind::Strong(children)
                | InlineKind::Strikethrough(children)
                | InlineKind::Highlight(children)
                | InlineKind::Insert(children) => self.image_alt(children),
                InlineKind::Link(link)
                | InlineKind::LinkPreview(link)
                | InlineKind::Image(link)
                | InlineKind::ImagePreview(link) => self.image_alt(&link.children),
                InlineKind::SoftBreak | InlineKind::HardBreak => self.html.push(' '),
                InlineKind::NumericReference(_) => {
                    escape(&mut self.html, &self.source[Range::from(inline.span)])
                }
                InlineKind::FootnoteReference { label, .. } => {
                    escape(&mut self.html, &self.source[Range::from(*label)])
                }
                InlineKind::Html
                | InlineKind::CustomTag(_)
                | InlineKind::Extension(_)
                | InlineKind::ExtensionWithFields { .. } => {}
            }
        }
    }

    fn html_block(&mut self, raw: &str) {
        self.raw_html(raw);
        if !self.html.ends_with('\n') {
            self.html.push('\n');
        }
    }

    fn raw_html(&mut self, mut raw: &str) {
        if !self.tagfilter {
            normalized(&mut self.html, raw);
            return;
        }
        while let Some(offset) = raw.find('<') {
            normalized(&mut self.html, &raw[..offset]);
            raw = &raw[offset + 1..];
            let tag = raw.strip_prefix('/').unwrap_or(raw);
            let name_end = tag
                .find(|ch: char| !ch.is_ascii_alphanumeric())
                .unwrap_or(tag.len());
            let bounded = tag
                .as_bytes()
                .get(name_end)
                .is_some_and(|b| b.is_ascii_whitespace() || matches!(b, b'/' | b'>'));
            let denied = bounded
                && [
                    "title",
                    "textarea",
                    "style",
                    "xmp",
                    "iframe",
                    "noembed",
                    "noframes",
                    "script",
                    "plaintext",
                ]
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&tag[..name_end]));
            self.html.push_str(if denied { "&lt;" } else { "<" });
        }
        normalized(&mut self.html, raw);
    }
}

fn normalized(out: &mut String, text: &str) {
    if !text.contains(['\r', '\0']) {
        out.push_str(text);
        return;
    }
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                chars.next_if_eq(&'\n');
                out.push('\n');
            }
            '\0' => out.push('\u{fffd}'),
            _ => out.push(ch),
        }
    }
}

fn escape(out: &mut String, text: &str) {
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\0' => out.push('\u{fffd}'),
            '\r' => {
                chars.next_if_eq(&'\n');
                out.push('\n');
            }
            _ => out.push(ch),
        }
    }
}

fn escape_url(out: &mut String, text: &str) {
    for byte in text.bytes() {
        if byte == b'&' {
            out.push_str("&amp;");
        } else if byte.is_ascii_alphanumeric() || b"-._~:/?#@!$'()*+,;=%".contains(&byte) {
            out.push(char::from(byte));
        } else {
            write!(out, "%{byte:02X}").unwrap();
        }
    }
}

fn unescape_punctuation(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\'
            && let Some(next) = chars.next_if(char::is_ascii_punctuation)
        {
            out.push(next);
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_stream_matches_full_nodes_at_varied_chunk_boundaries() {
        let normalize = |view: StreamView| {
            let positions: HashMap<_, _> = view
                .keys()
                .enumerate()
                .map(|(index, &id)| (id, index))
                .collect();
            view.into_values()
                .enumerate()
                .map(|(index, (parent, mut block))| {
                    block.id = BlockId(index);
                    (parent.map(|id| positions[&id]), block)
                })
                .collect::<Vec<_>>()
        };

        let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/corpus");
        for fixture in std::fs::read_dir(corpus).unwrap() {
            let path = fixture.unwrap().path();
            if path.extension().is_none_or(|extension| extension != "md") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            for markstream in [false, true] {
                let options = Options {
                    markstream,
                    math: markstream,
                    preview: Preview::Ai,
                    ..Options::default()
                };
                let document = parse(&source, &options).unwrap();
                let expected = normalize(
                    document
                        .nodes
                        .iter()
                        .map(|block| (block.id, (document.parent(block.id), block.clone())))
                        .collect(),
                );
                for width in [31, 137, 1024] {
                    let mut rest = source.as_str();
                    let chunks = std::iter::from_fn(|| {
                        if rest.is_empty() {
                            return None;
                        }
                        let mut end = rest.len().min(width);
                        while !rest.is_char_boundary(end) {
                            end -= 1;
                        }
                        let (chunk, remaining) = rest.split_at(end);
                        rest = remaining;
                        Some(chunk)
                    });
                    let context = format!("{path:?} at {width} bytes with Markstream={markstream}");
                    let view = stream_view_chunks(chunks, options.clone())
                        .unwrap_or_else(|error| panic!("{context}: {error}"));
                    let actual = normalize(view);
                    assert_eq!(actual.len(), expected.len(), "{context}");
                    for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
                        assert_eq!(actual, expected, "{context}, block {index}");
                    }
                }
            }
        }
    }
}
