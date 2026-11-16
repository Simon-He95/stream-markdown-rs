use crate::{
    consume_stream,
    trace::{self, Trace},
    Engine, Mode, OwnedPulldown, Pulldown,
};
use serde_json::{json, Value};
use stream_markdown_rs::{
    parse, Alignment, Block, BlockData, BlockId, BlockStatus, Change, Document, Inline,
    InlineContent, InlineKind, Options, Parser, RawLine, Span,
};

pub struct Candidate;

pub struct Session {
    parser: Parser,
    changes: Vec<Change>,
    nodes: Vec<Option<Block>>,
    parents: Vec<Option<BlockId>>,
}

impl<'a> Engine<'a> for Candidate {
    type Document = Document<'a>;
    type Session = Session;

    fn parse(source: &'a str) -> Result<Self::Document, String> {
        parse(source, &Options::default()).map_err(|error| error.to_string())
    }

    fn start() -> Self::Session {
        Session {
            parser: Parser::new(Options::default()),
            changes: Vec::new(),
            nodes: Vec::new(),
            parents: Vec::new(),
        }
    }

    fn append(session: &mut Self::Session, source: &'a str, start: usize) -> Result<(), String> {
        session
            .parser
            .append(&source[start..], &mut session.changes)
            .map_err(|error| error.to_string())?;
        session.apply()
    }

    fn finish(session: &mut Self::Session) -> Result<(), String> {
        session
            .parser
            .finish(&mut session.changes)
            .map_err(|error| error.to_string())?;
        session.apply()?;
        session.changes = Vec::new();
        Ok(())
    }

    #[cfg(not(feature = "count-allocations"))]
    fn has_output(session: &Self::Session) -> bool {
        session.nodes.iter().any(Option::is_some)
    }
}

impl Session {
    fn apply(&mut self) -> Result<(), String> {
        for change in self.changes.drain(..) {
            match change {
                Change::Insert { parent, block } => {
                    let id = block.id.0;
                    if let Some(parent) = parent {
                        node_mut(&mut self.nodes, parent)?;
                        if id >= self.parents.len() {
                            self.parents.resize(id + 1, None);
                        }
                        self.parents[id] = Some(parent);
                    } else if let Some(slot) = self.parents.get_mut(id) {
                        *slot = None;
                    }
                    if id >= self.nodes.len() {
                        self.nodes.resize_with(id + 1, || None);
                    }
                    if self.nodes[id].is_some() {
                        return Err(format!("candidate inserted live node {id}"));
                    }
                    self.nodes[id] = Some(block);
                }
                Change::Replace(block) => {
                    let id = block.id;
                    *node_mut(&mut self.nodes, id)? = block;
                }
                Change::Remove { id } => {
                    node_mut(&mut self.nodes, id)?;
                    self.nodes[id.0] = None;
                    if let Some(slot) = self.parents.get_mut(id.0) {
                        *slot = None;
                    }
                }
                Change::SetStatus { id, status, end } => {
                    let block = node_mut(&mut self.nodes, id)?;
                    block.status = status;
                    block.span.end = end;
                }
                Change::AppendText { id, span } => {
                    let block = node_mut(&mut self.nodes, id)?;
                    let body =
                        match &mut block.data {
                            BlockData::CodeBlock { body, .. }
                            | BlockData::Paragraph(InlineContent::Plain(body))
                            | BlockData::Heading {
                                content: InlineContent::Plain(body),
                                ..
                            } => body,
                            BlockData::CustomTag(element) => &mut element.body,
                            BlockData::Html => &mut block.span,
                            BlockData::Paragraph(InlineContent::Fragments(parts)) => parts
                                .last_mut()
                                .ok_or_else(|| format!("node {} has no text fragments", id.0))?,
                            _ => return Err(format!("unexpected plain append to node {}", id.0)),
                        };
                    if body.end != span.start {
                        return Err(format!("noncontiguous plain append to node {}", id.0));
                    }
                    body.end = span.end;
                    block.span.end = span.end;
                }
                Change::AddTextFragment { id, span } => {
                    let block = node_mut(&mut self.nodes, id)?;
                    let BlockData::Paragraph(content) = &mut block.data else {
                        return Err(format!("unexpected text fragment for node {}", id.0));
                    };
                    match content {
                        InlineContent::Plain(first) => {
                            *content = InlineContent::Fragments(Box::new(vec![*first, span]));
                        }
                        InlineContent::Fragments(parts) => parts.push(span),
                        _ => return Err(format!("unexpected text fragment for node {}", id.0)),
                    }
                    block.span.end = span.end;
                }
                Change::AddRawLine { id, line, end } => {
                    let block = node_mut(&mut self.nodes, id)?;
                    let lines = match &mut block.data {
                        BlockData::SegmentedCode(code) => &mut code.lines,
                        BlockData::IndentedCode { lines } | BlockData::SegmentedHtml(lines) => {
                            lines
                        }
                        _ => return Err(format!("unexpected raw line for node {}", id.0)),
                    };
                    lines.push(line);
                    block.span.end = end;
                }
                Change::ReplaceTableCell { id, index, content } => {
                    let block = node_mut(&mut self.nodes, id)?;
                    let BlockData::Table(table) = &mut block.data else {
                        return Err(format!("replaced a cell in non-table block {id:?}"));
                    };
                    table.replace_cell(index, content);
                }
                Change::AddRow { id, end, cells } => {
                    let block = node_mut(&mut self.nodes, id)?;
                    let BlockData::Table(table) = &mut block.data else {
                        return Err(format!(
                            "candidate appended a row to non-table node {}",
                            id.0
                        ));
                    };
                    table.add_row(cells);
                    block.span.end = end;
                }
                Change::Preview { hint: None, .. } => {}
                change => {
                    return Err(format!(
                        "unsupported candidate change projection: {change:?}"
                    ));
                }
            }
        }
        Ok(())
    }
}

fn node_mut(nodes: &mut [Option<Block>], id: BlockId) -> Result<&mut Block, String> {
    nodes
        .get_mut(id.0)
        .and_then(Option::as_mut)
        .ok_or_else(|| format!("candidate update to unknown node {}", id.0))
}

pub fn verify(mode: Mode, source: &str, ends: &[usize]) -> Result<Value, String> {
    let expected = trace::pulldown(Pulldown::parse(source)?)?;
    let document = Candidate::parse(source)?;
    if document
        .nodes
        .iter()
        .any(|block| block.status == BlockStatus::Open)
    {
        return Err("candidate full parse left a block in the open state".into());
    }
    let actual = blocks(
        source,
        document
            .nodes
            .iter()
            .map(|block| (block, document.parent(block.id))),
    )?;
    let mut difference = expected.difference(&actual);
    let mut counters = Value::Null;

    if matches!(mode, Mode::Stream) && difference.is_none() {
        let mut pulldown = Pulldown::start();
        consume_stream::<Pulldown>(&mut pulldown, source, ends, Pulldown::append)?;
        Pulldown::finish(&mut pulldown)?;
        if let Some(difference) = expected.difference(&trace::pulldown(pulldown)?) {
            return Err(format!(
                "pulldown replay diverged from its full parse: {difference}"
            ));
        }

        let mut owned = OwnedPulldown::start();
        consume_stream::<OwnedPulldown>(&mut owned, source, ends, OwnedPulldown::append)?;
        OwnedPulldown::finish(&mut owned)?;
        if owned.source != source {
            return Err("source-owning pulldown replay did not retain appended input".into());
        }
        if let Some(difference) = expected.difference(&trace::pulldown(owned.events)?) {
            return Err(format!(
                "source-owning pulldown replay diverged from its full parse: {difference}"
            ));
        }

        let mut session = Candidate::start();
        consume_stream::<Candidate>(&mut session, source, ends, Candidate::append)?;
        Candidate::finish(&mut session)?;
        if session.parser.source_text() != source {
            return Err("candidate did not retain the appended source".into());
        }
        let streamed = blocks(
            source,
            session
                .nodes
                .iter()
                .flatten()
                .map(|block| (block, session.parents.get(block.id.0).copied().flatten())),
        )?;
        difference = expected.difference(&streamed).or_else(|| {
            let mut ordered = document.nodes.iter().collect::<Vec<_>>();
            ordered.sort_unstable_by_key(|block| block.id);
            let full_states = ordered.into_iter().map(|block| (block.span, block.status));
            let stream_states = session
                .nodes
                .iter()
                .flatten()
                .map(|block| (block.span, block.status));
            (!full_states.eq(stream_states))
                .then(|| "candidate full and streaming source ranges or final states differ".into())
        });
        let metrics = session.parser.metrics();
        counters = json!({
            "input_bytes": metrics.input_bytes,
            "scanned_bytes": metrics.scanned_bytes,
            "inline_bytes": metrics.inline_bytes,
            "invalidated_blocks": metrics.invalidated_blocks,
        });
    }

    Ok(json!({
        "measurement": "verification",
        "verified": difference.is_none(),
        "scope": "final-semantic-trace",
        "difference": difference,
        "stream_work": counters,
        "native_layout_bytes": {
            "span": std::mem::size_of::<Span>(),
            "block": std::mem::size_of::<Block>(),
            "stream_block_slot": std::mem::size_of::<Option<Block>>(),
            "change": std::mem::size_of::<Change>(),
            "inline_content": std::mem::size_of::<InlineContent>(),
            "pulldown_event": std::mem::size_of::<pulldown_cmark::Event<'static>>(),
        },
    }))
}

fn read(source: &str, span: Span) -> &str {
    &source[span.start..span.end]
}

struct Container {
    id: BlockId,
    tag: &'static str,
    tight_list: bool,
    task: Option<bool>,
}

fn blocks<'a>(
    source: &str,
    blocks: impl IntoIterator<Item = (&'a Block, Option<BlockId>)>,
) -> Result<Trace, String> {
    let mut trace = Trace::default();
    let mut ordered = blocks.into_iter().collect::<Vec<_>>();
    ordered.sort_unstable_by_key(|(block, _)| block.id);
    let mut stack: Vec<Container> = Vec::new();

    for (block, parent) in ordered {
        while stack
            .last()
            .is_some_and(|container| Some(container.id) != parent)
        {
            close_container(&mut trace, stack.pop().unwrap());
        }
        if stack.last().map(|container| container.id) != parent {
            return Err(format!(
                "candidate has an invalid parent for node {}",
                block.id.0
            ));
        }

        let task = stack.last_mut().and_then(|container| container.task.take());
        if !matches!(block.data, BlockData::Paragraph(_)) {
            task_marker(&mut trace, task);
        }
        match &block.data {
            BlockData::Quote => {
                trace.start("quote");
                stack.push(Container {
                    id: block.id,
                    tag: "quote",
                    tight_list: false,
                    task: None,
                });
            }
            BlockData::List {
                ordered,
                start,
                tight,
            } => {
                trace.start("list");
                trace.field(if *ordered {
                    start.to_string()
                } else {
                    String::new()
                });
                stack.push(Container {
                    id: block.id,
                    tag: "list",
                    tight_list: *tight,
                    task: None,
                });
            }
            BlockData::ListItem { task } => {
                trace.start("item");
                let tight_list = stack.last().is_some_and(|container| container.tight_list);
                stack.push(Container {
                    id: block.id,
                    tag: "item",
                    tight_list,
                    task: *task,
                });
            }
            BlockData::Paragraph(content) => {
                let tight = stack
                    .last()
                    .is_some_and(|container| container.tag == "item" && container.tight_list);
                if !tight {
                    trace.start("paragraph");
                }
                task_marker(&mut trace, task);
                inlines(&mut trace, source, content)?;
                if !tight {
                    trace.end("paragraph");
                }
            }
            BlockData::Html => {
                trace.start("html-block");
                trace.html(read(source, block.span));
                trace.end("html-block");
            }
            BlockData::SegmentedHtml(lines) => {
                trace.start("html-block");
                raw_lines(&mut trace, source, lines, Trace::html);
                trace.end("html-block");
            }
            BlockData::Heading { level, content } => {
                trace.start("heading");
                trace.field(level.to_string());
                inlines(&mut trace, source, content)?;
                trace.end("heading");
            }
            BlockData::CodeBlock { body, .. } => {
                trace.start("code-block");
                trace.field(
                    block
                        .data
                        .code_info()
                        .map(|span| read(source, span))
                        .unwrap_or_default(),
                );
                trace.text(read(source, *body));
                trace.end("code-block");
            }
            BlockData::SegmentedCode(code) => {
                trace.start("code-block");
                trace.field(code.info.map(|span| read(source, span)).unwrap_or_default());
                raw_lines(&mut trace, source, &code.lines, Trace::text);
                trace.end("code-block");
            }
            BlockData::IndentedCode { lines } => {
                trace.start("code-block");
                trace.field("");
                raw_lines(&mut trace, source, lines, Trace::text);
                trace.end("code-block");
            }
            BlockData::ThematicBreak => trace.marker("thematic-break"),
            BlockData::Table(table) => {
                let alignments = table.alignments();
                let header = table.header();
                let rows = table.rows();
                trace.start("table");
                for alignment in alignments {
                    trace.field(match alignment {
                        Alignment::None => "none",
                        Alignment::Left => "left",
                        Alignment::Center => "center",
                        Alignment::Right => "right",
                    });
                }
                table_row(&mut trace, source, "table-head", header)?;
                for row in rows {
                    table_row(&mut trace, source, "table-row", row)?;
                }
                trace.end("table");
            }
            block => return Err(format!("unsupported candidate block projection: {block:?}")),
        }
    }
    for container in stack.into_iter().rev() {
        close_container(&mut trace, container);
    }
    Ok(trace)
}

fn raw_lines(trace: &mut Trace, source: &str, lines: &[RawLine], write: fn(&mut Trace, &str)) {
    for line in lines {
        for _ in 0..line.leading_spaces {
            write(trace, " ");
        }
        write(trace, read(source, line.span));
    }
}

fn close_container(trace: &mut Trace, container: Container) {
    task_marker(trace, container.task);
    trace.end(container.tag);
}

fn task_marker(trace: &mut Trace, task: Option<bool>) {
    if let Some(checked) = task {
        trace.marker(if checked { "checked" } else { "unchecked" });
    }
}

fn table_row(
    trace: &mut Trace,
    source: &str,
    tag: &'static str,
    cells: &[InlineContent],
) -> Result<(), String> {
    trace.start(tag);
    for cell in cells {
        trace.start("table-cell");
        inlines(trace, source, cell)?;
        trace.end("table-cell");
    }
    trace.end(tag);
    Ok(())
}

fn inlines(trace: &mut Trace, source: &str, content: &InlineContent) -> Result<(), String> {
    match content {
        InlineContent::Plain(span) => trace.text(read(source, *span)),
        InlineContent::Parsed(nodes) => inline_nodes(trace, source, nodes)?,
        InlineContent::Fragments(parts) => {
            for &part in parts.iter() {
                trace.text(read(source, part));
            }
        }
    }
    Ok(())
}

fn inline_nodes(trace: &mut Trace, source: &str, nodes: &[Inline]) -> Result<(), String> {
    for node in nodes {
        match &node.kind {
            InlineKind::Text(text) => trace.text(text.as_str(source)),
            InlineKind::Code(text) => {
                trace.start("code");
                trace.text(text.as_str(source));
                trace.end("code");
            }
            InlineKind::Emphasis(children)
            | InlineKind::Strong(children)
            | InlineKind::Strikethrough(children) => {
                let tag = match &node.kind {
                    InlineKind::Emphasis(_) => "emphasis",
                    InlineKind::Strong(_) => "strong",
                    _ => "strikethrough",
                };
                trace.start(tag);
                inline_nodes(trace, source, children)?;
                trace.end(tag);
            }
            InlineKind::Link(link) | InlineKind::Image(link) => {
                let tag = if matches!(node.kind, InlineKind::Link(_)) {
                    "link"
                } else {
                    "image"
                };
                trace.start(tag);
                trace.field(link.destination.as_str(source));
                trace.field(
                    link.title
                        .as_ref()
                        .map(|title| title.as_str(source))
                        .unwrap_or_default(),
                );
                inline_nodes(trace, source, &link.children)?;
                trace.end(tag);
            }
            InlineKind::Html => trace.html(read(source, node.span)),
            InlineKind::SoftBreak => trace.marker("softbreak"),
            InlineKind::HardBreak => trace.marker("hardbreak"),
            #[allow(unreachable_patterns)]
            kind => return Err(format!("unsupported candidate inline projection: {kind:?}")),
        }
    }
    Ok(())
}
