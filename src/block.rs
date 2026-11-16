#[path = "footnotes.rs"]
mod footnotes;
#[path = "vmr.rs"]
pub(crate) mod vmr;

use std::{collections::HashMap, num::NonZeroUsize, ops::Range, sync::Arc};

use crate::container::{self, Frame, Kind, Style};
use crate::{
    Alignment, Block, BlockData, BlockId, BlockStatus, Change, InlineContent, Metrics, Options,
    ParseError, Phase, PluginError, PluginRule, Preview, PreviewHint, RawLine, Reference,
    References, Rule, RuleInput, RuleResult, Scope, Span, Syntax, TableData, Tag, Text, inline,
    math_block, tag,
};

pub(crate) enum Output<'a> {
    Stream(&'a mut Vec<Change>, usize),
    Batch(&'a mut Vec<Block>, &'a mut Vec<Option<NonZeroUsize>>),
}

impl Output<'_> {
    fn open(&mut self, block: Block, parent: Option<BlockId>) {
        self.update(Change::Insert { parent, block });
    }

    fn done(&mut self, block: Block, present: bool, parent: Option<BlockId>) {
        match self {
            Self::Batch(nodes, parents) => {
                if let Some(parent) = parent {
                    parents.resize(block.id.0, None);
                    parents.push(NonZeroUsize::new(parent.0 + 1));
                }
                nodes.push(block);
            }
            Self::Stream(changes, _) if present => changes.push(Change::Replace(block)),
            Self::Stream(changes, _) => changes.push(Change::Insert { parent, block }),
        }
    }

    fn batch_index(&self) -> usize {
        match self {
            Self::Batch(nodes, _) => nodes.len(),
            Self::Stream(..) => 0,
        }
    }

    fn close_container(&mut self, index: usize, block: Block) {
        match self {
            Self::Batch(nodes, _) => nodes[index] = block,
            Self::Stream(changes, _) => changes.push(Change::Replace(block)),
        }
    }

    fn append_text(&mut self, id: BlockId, span: Span) {
        if let Self::Stream(changes, first) = self {
            if changes.len() > *first
                && let Some(Change::AppendText {
                    id: previous,
                    span: previous_span,
                }) = changes.last_mut()
                && *previous == id
                && previous_span.end == span.start
            {
                previous_span.end = span.end;
            } else {
                changes.push(Change::AppendText { id, span });
            }
        }
    }

    fn reserve_single_block(&mut self) {
        if let Self::Batch(nodes, _) = self
            && nodes.capacity() == 0
        {
            nodes.reserve_exact(1);
        }
    }

    fn update(&mut self, change: Change) {
        if let Self::Stream(changes, _) = self {
            changes.push(change);
        }
    }
}

#[derive(Clone, Copy)]
struct Line {
    start: usize,
    end: usize,
    next: usize,
    column: usize,
    padding: usize,
    lazy: bool,
}

impl Line {
    fn new(start: usize, end: usize, next: usize) -> Self {
        Self {
            start,
            end,
            next,
            column: 0,
            padding: 0,
            lazy: false,
        }
    }

    fn span(self) -> Span {
        Span::new(self.start, self.end)
    }

    fn advance(&mut self, prefix: container::Prefix) {
        self.start += prefix.bytes;
        self.column += prefix.columns;
        self.padding = prefix.padding;
    }

    fn raw(self) -> RawLine {
        RawLine {
            span: Span::new(self.start, self.next),
            leading_spaces: self.padding as u8,
        }
    }
}

struct Paragraph {
    id: BlockId,
    span: Span,
    first: Span,
    published: usize,
    visible: bool,
    lines: usize,
    next: usize,
    parts: Vec<Span>,
}

impl Paragraph {
    fn leaf(mut self, span: Span, heading: Option<u8>) -> Dependent {
        if let Some(last) = self.parts.last_mut() {
            last.end = self.span.end;
        }
        Dependent {
            id: self.id,
            span,
            content: self.span,
            heading,
            parts: (!self.parts.is_empty()).then(|| self.parts.into()),
        }
    }
}

struct Fence {
    id: BlockId,
    start: usize,
    info: Option<Span>,
    marker: u8,
    count: usize,
    indent: usize,
    body: Span,
    lines: Option<Vec<RawLine>>,
}

#[derive(Clone, Copy)]
enum FencePrefix {
    Indent(u8),
    Run(usize),
    Spaces,
    Literal,
}

struct FenceProbe {
    start: usize,
    checked: usize,
    prefix: FencePrefix,
}

impl FenceProbe {
    fn update(&mut self, source: &str, end: usize, marker: u8, width: usize) -> bool {
        for byte in source.as_bytes()[self.checked..end].iter().copied() {
            self.prefix = match self.prefix {
                FencePrefix::Indent(count) if byte == b' ' && count < 3 => {
                    FencePrefix::Indent(count + 1)
                }
                FencePrefix::Indent(_) if byte == marker => FencePrefix::Run(1),
                FencePrefix::Run(count) if byte == marker => FencePrefix::Run(count + 1),
                FencePrefix::Run(count) if count >= width && matches!(byte, b' ' | b'\t') => {
                    FencePrefix::Spaces
                }
                FencePrefix::Spaces if matches!(byte, b' ' | b'\t') => FencePrefix::Spaces,
                _ => FencePrefix::Literal,
            };
            if matches!(self.prefix, FencePrefix::Literal) {
                break;
            }
        }
        self.checked = end;
        matches!(self.prefix, FencePrefix::Literal)
    }
}

struct Table {
    id: BlockId,
    span: Span,
    columns: usize,
    next_cell: usize,
    batch_data: Option<Box<TableData>>,
}

#[derive(Clone, Copy)]
enum SeparatorStage {
    Start,
    LeadingColon,
    Dashes,
    TrailingColon,
    Spaces,
    Invalid,
}

struct SeparatorProbe {
    start: usize,
    scanned: usize,
    alignments: Vec<Alignment>,
    completed: usize,
    saw_dash: bool,
    stage: SeparatorStage,
}

impl SeparatorProbe {
    fn new(source: &str, start: usize, header: Span) -> Self {
        let columns = if source[Range::from(header)].contains('|') {
            cell_spans(source, header).count()
        } else {
            0
        };
        Self {
            start,
            scanned: start + 1,
            alignments: vec![Alignment::None; columns],
            completed: 0,
            saw_dash: false,
            stage: SeparatorStage::Start,
        }
    }

    fn ready(&self) -> bool {
        self.completed == self.alignments.len()
            || (self.completed + 1 == self.alignments.len()
                && matches!(
                    self.stage,
                    SeparatorStage::Dashes | SeparatorStage::TrailingColon | SeparatorStage::Spaces
                ))
    }

    fn update(&mut self, source: &str, end: usize) -> (bool, bool) {
        use SeparatorStage::*;
        let was_ready = self.ready();
        let mut alignment_changed = false;
        for byte in source.as_bytes()[self.scanned..end].iter().copied() {
            if matches!(self.stage, Invalid) {
                break;
            }
            if self.completed == self.alignments.len() {
                if !matches!(byte, b' ' | b'\t') {
                    self.stage = Invalid;
                }
                continue;
            }
            self.stage = match (self.stage, byte) {
                (Start, b' ' | b'\t') => Start,
                (Start, b':') => {
                    self.alignments[self.completed] = Alignment::Left;
                    alignment_changed = true;
                    LeadingColon
                }
                (Start | LeadingColon | Dashes, b'-') => {
                    self.saw_dash = true;
                    Dashes
                }
                (Dashes, b':') => {
                    let alignment = &mut self.alignments[self.completed];
                    *alignment = if *alignment == Alignment::Left {
                        Alignment::Center
                    } else {
                        Alignment::Right
                    };
                    alignment_changed = true;
                    TrailingColon
                }
                (Dashes | TrailingColon | Spaces, b' ' | b'\t') => Spaces,
                (Dashes | TrailingColon | Spaces, b'|') => {
                    self.completed += 1;
                    Start
                }
                _ => Invalid,
            };
        }
        self.scanned = end;
        (
            self.saw_dash && !matches!(self.stage, Invalid),
            alignment_changed || self.ready() != was_ready,
        )
    }
}

struct Html {
    id: BlockId,
    span: Span,
    end: HtmlEnd,
    lines: Option<Vec<RawLine>>,
}

#[derive(Clone, Copy)]
enum HtmlEnd {
    Blank,
    Literal(&'static str),
    AsciiCase(&'static str),
}

impl HtmlEnd {
    fn closes(self, line: &str) -> bool {
        match self {
            Self::Blank => blank(line),
            Self::Literal(marker) => line.contains(marker),
            Self::AsciiCase(marker) => line
                .as_bytes()
                .windows(marker.len())
                .any(|window| window.eq_ignore_ascii_case(marker.as_bytes())),
        }
    }
}

struct Indented {
    id: BlockId,
    span: Span,
    lines: Vec<RawLine>,
    pending_blank: Vec<RawLine>,
}

struct Element {
    id: BlockId,
    start: usize,
    tag: Tag,
    depth: usize,
    body: Span,
    scanner: Option<Box<tag::BodyScanner>>,
}

struct DisplayMath {
    id: BlockId,
    start: usize,
    delimiter: math_block::Delimiter,
    body: Span,
    line_next: usize,
    end: usize,
    present: bool,
    shown: bool,
    evidence: bool,
    checked: usize,
    parts: Vec<Span>,
}

impl DisplayMath {
    fn add(&mut self, source: &str, start: usize, end: usize, next: usize) {
        if end > start {
            if self.body.start == self.body.end {
                self.body.start = start;
            } else if !self.parts.is_empty() || start != self.line_next {
                if self.parts.is_empty() {
                    self.parts.push(Span::new(self.body.start, self.line_next));
                }
                self.parts.push(Span::new(start, next));
            }
            self.body.end = end;
            self.evidence |= math_block::evidence(&source[start..end], false);
        }
        self.line_next = next;
        self.end = next;
    }

    fn into_text(mut self, source: &str) -> Text {
        if self.parts.is_empty() {
            Text::Source(self.body)
        } else {
            self.parts.last_mut().unwrap().end = self.body.end;
            let mut text = String::new();
            for span in self.parts {
                text.push_str(&source[Range::from(span)]);
            }
            Text::Decoded(text.into_boxed_str())
        }
    }
}

enum Active {
    Paragraph(Paragraph),
    Fence(Fence),
    Math(DisplayMath),
    Footnote(footnotes::Note),
    Table(Table),
    Element(Element),
    Html(Html),
    Indented(Indented),
}

#[derive(Clone, Copy)]
struct Draft {
    id: BlockId,
    start: usize,
    published: usize,
}

#[derive(Clone, PartialEq)]
struct Dependent {
    id: BlockId,
    span: Span,
    content: Span,
    heading: Option<u8>,
    parts: Option<Arc<[Span]>>,
}

#[derive(Clone)]
enum Dependency {
    Leaf(Dependent),
    Footnote(footnotes::NoteBody),
    Cell {
        id: BlockId,
        index: usize,
        span: Span,
    },
}

impl Dependency {
    fn key(&self) -> (BlockId, Option<usize>) {
        match self {
            Self::Leaf(leaf) => (leaf.id, None),
            Self::Footnote(body) => (body.id, None),
            Self::Cell { id, index, .. } => (*id, Some(*index)),
        }
    }
}

struct Registered {
    rule: Rule,
    implementation: Box<dyn PluginRule>,
}

struct PluginMatch {
    index: usize,
    start: usize,
    cursor: usize,
}

pub(crate) struct Engine {
    options: Options,
    batch: bool,
    scan: usize,
    line_start: usize,
    next_id: usize,
    active: Option<Active>,
    containers: Vec<Frame>,
    draft: Option<Draft>,
    preview: Option<BlockId>,
    inline_preview: Option<(BlockId, usize)>,
    separator: Option<SeparatorProbe>,
    fence_probe: Option<FenceProbe>,
    tag_probe: Option<tag::OpeningProbe>,
    refs: References,
    footnotes: Option<Box<footnotes::Index>>,
    waiting: HashMap<String, Vec<Dependency>>,
    batch_parts: HashMap<BlockId, Arc<[Span]>>,
    plugins: Vec<Registered>,
    block_triggers: [bool; 128],
    running: Option<PluginMatch>,
    unresolved: Vec<String>,
    metrics: Metrics,
}

impl Engine {
    pub(crate) fn new(options: Options, batch: bool) -> Self {
        let footnotes = options.footnotes.then(Box::<footnotes::Index>::default);
        Self {
            options,
            batch,
            scan: 0,
            line_start: 0,
            next_id: 0,
            active: None,
            containers: Vec::new(),
            draft: None,
            preview: None,
            inline_preview: None,
            separator: None,
            fence_probe: None,
            tag_probe: None,
            refs: References::default(),
            footnotes,
            waiting: HashMap::new(),
            batch_parts: HashMap::new(),
            plugins: Vec::new(),
            block_triggers: [false; 128],
            running: None,
            unresolved: Vec::new(),
            metrics: Metrics::default(),
        }
    }

    pub(crate) fn add_plugin(
        &mut self,
        implementation: Box<dyn PluginRule>,
    ) -> Result<(), PluginError> {
        let rule = implementation.rule();
        if !rule.trigger.is_ascii() || rule.trigger.is_ascii_whitespace() {
            return Err(PluginError::new(
                "plugin triggers must be non-whitespace ASCII",
            ));
        }
        if rule.scope == Scope::Block {
            self.block_triggers[usize::from(rule.trigger)] = true;
        }
        self.plugins.push(Registered {
            rule,
            implementation,
        });
        Ok(())
    }

    pub(crate) fn metrics(&self) -> Metrics {
        self.metrics
    }

    pub(crate) fn consume(
        &mut self,
        source: &str,
        final_input: bool,
        mut output: Output<'_>,
    ) -> Result<(), ParseError> {
        self.metrics.input_bytes = source.len();
        let bytes = source.as_bytes();
        while self.scan < source.len() {
            if self.batch
                && self.containers.is_empty()
                && matches!(
                    self.active.as_ref(),
                    Some(Active::Fence(Fence { lines: None, .. }))
                )
            {
                let Some(Active::Fence(mut fence)) = self.active.take() else {
                    unreachable!()
                };
                let close = batch_fence_close(source, self.scan, fence.marker, fence.count);
                fence.body.end = close.map_or(source.len(), |line| line.start);
                let end = close.map_or(source.len(), |line| line.next);
                let status = if close.is_some() {
                    BlockStatus::Closed
                } else {
                    BlockStatus::Incomplete
                };
                self.metrics.scanned_bytes += end - self.scan;
                self.scan = end;
                self.line_start = end;
                self.active = None;
                if end == source.len() {
                    output.reserve_single_block();
                }
                self.close_code(fence, end, status, &mut output);
                continue;
            }
            if !self.batch && !final_input && self.stream_fence_text(bytes, &mut output) {
                break;
            }
            let Some(distance) = memchr::memchr2(b'\n', b'\r', &bytes[self.scan..]) else {
                self.metrics.scanned_bytes += source.len() - self.scan;
                self.scan = source.len();
                break;
            };
            let end = self.scan + distance;
            self.metrics.scanned_bytes += distance;
            self.scan = end;
            if bytes[end] == b'\r' && end + 1 == bytes.len() && !final_input {
                break;
            }
            let next =
                end + 1 + usize::from(bytes[end] == b'\r' && bytes.get(end + 1) == Some(&b'\n'));
            self.metrics.scanned_bytes += next - end;
            self.container_line(source, Line::new(self.line_start, end, next), &mut output)?;
            self.line_start = next;
            self.scan = next;
        }

        if final_input {
            if self.line_start != source.len() {
                let end = source.len();
                self.container_line(source, Line::new(self.line_start, end, end), &mut output)?;
                self.line_start = end;
            }
            if let Some(running) = self.running.take() {
                self.resume_plugin(source, running, source.len(), Phase::Finish, &mut output)?;
            }
            if let Some(active) = self.active.take() {
                self.end_active(source, active, BlockStatus::Incomplete, &mut output)?;
            }
            self.close_containers(0, &mut output);
            self.clear_preview(&mut output);
        } else {
            if self.active.is_none()
                && !self.containers.is_empty()
                && let Some(&byte) = bytes.get(self.line_start)
                && (ordinary_start(byte) || byte == b'|')
            {
                let limit = self
                    .containers
                    .iter()
                    .rposition(|frame| matches!(frame.kind, Kind::Vmr { .. }))
                    .map_or(0, |index| index + 1);
                self.close_containers(limit, &mut output);
            }
            if self.options.markstream && self.partial_vmr(source, &mut output)? {
                return Ok(());
            }
            if self.options.footnotes
                && self.running.is_none()
                && self.open_partial_footnote(source, &mut output)?
            {
                return Ok(());
            }
            if self.running.is_none()
                && self.containers.is_empty()
                && !self.options.custom_tags.is_empty()
            {
                while self.open_partial_tag(source, &mut output)? {}
            }
            if self.options.math
                && self.running.is_none()
                && self.containers.is_empty()
                && self.partial_display(source, &mut output)?
            {
                return Ok(());
            }
            self.partial(source, &mut output);
            if self.options.preview == Preview::Ai {
                self.preview_inline(source, &mut output);
            }
        }
        Ok(())
    }

    fn partial_display(
        &mut self,
        source: &str,
        output: &mut Output<'_>,
    ) -> Result<bool, ParseError> {
        loop {
            if self.line_start >= self.scan {
                return Ok(false);
            }
            if let Some(Active::Math(math)) = &mut self.active {
                let line = self.line_start;
                let end = self.scan;
                let mark = math.delimiter;
                let from = math.checked.saturating_sub(mark.width() - 1).max(line);
                if mark != math_block::Delimiter::Brackets
                    && let Some(close) = mark.explicit_close(source, line, from, end)
                {
                    self.line(source, Line::new(line, close.end, close.end), output)?;
                    self.line_start = close.end;
                    continue;
                }
                let old_end = math.body.end;
                let raw = &source[line..end];
                let pending = mark.closing();
                let display_end = if pending.len() == 2 && raw.as_bytes().last() == pending.first()
                {
                    end - 1
                } else {
                    end
                };
                let newly_visible = math.checked.saturating_sub(4).max(line);
                let mut evidence_start = newly_visible.min(display_end);
                while !source.is_char_boundary(evidence_start) {
                    evidence_start -= 1;
                }
                if display_end > old_end
                    && !blank(&source[old_end.max(line)..display_end])
                    && math.parts.is_empty()
                {
                    if math.body.start == math.body.end {
                        math.body.start = line;
                    }
                    math.body.end = display_end;
                    math.end = display_end;
                    math.evidence |=
                        math_block::evidence(&source[evidence_start..display_end], false);
                }
                math.checked = end;
                let mut active = self.active.take().unwrap();
                let Active::Math(math) = &mut active else {
                    unreachable!()
                };
                self.publish_display_progress(math, old_end, output);
                self.active = Some(active);
                return Ok(true);
            }
            if !matches!(self.active, None | Some(Active::Paragraph(_))) {
                return Ok(false);
            }
            let head = unindent(&source[self.line_start..self.scan]);
            let start = self.scan - head.len();
            if matches!(head, "$" | "$$" | "\\" | "\\[") {
                return Ok(true);
            }
            let marker = if let Some(body) = head.strip_prefix("$$") {
                (!body.starts_with('$')).then_some(math_block::Delimiter::Dollars)
            } else if head.starts_with("\\[") && head.len() <= 1024 {
                math_block::opening(head)
            } else {
                None
            };
            let Some(marker) = marker else {
                return Ok(false);
            };
            self.stop_leaf(source, output)?;
            let open_end = start + marker.width();
            self.start_display(
                source,
                Line::new(self.line_start, open_end, open_end),
                start,
                marker,
                output,
            )?;
            self.line_start = open_end;
        }
    }

    fn preview_inline(&mut self, source: &str, output: &mut Output<'_>) {
        const MAX_PREVIEW_BYTES: usize = 1024;
        let target = match &self.active {
            Some(Active::Paragraph(paragraph))
                if paragraph.visible && paragraph.parts.is_empty() =>
            {
                Some((
                    paragraph.id,
                    Span::new(paragraph.span.start, paragraph.published),
                ))
            }
            None => self.draft.map(|draft| {
                let mut start = draft.start;
                if source.as_bytes()[start] == b'#'
                    && let Some((_, content)) = atx(&source[start..draft.published], start)
                {
                    start = content.start;
                }
                (draft.id, Span::new(start, draft.published))
            }),
            _ => None,
        };
        let Some((id, body)) = target else {
            self.inline_preview = None;
            return;
        };
        let had_preview = self
            .inline_preview
            .is_some_and(|(previous, _)| previous == id);
        if body.end - body.start <= MAX_PREVIEW_BYTES {
            if self.inline_preview == Some((id, body.end)) {
                return;
            }
            let text = &source[Range::from(body)];
            if inline::needs_markup(text, self.options.syntax)
                || self.options.math && text.as_bytes().contains(&b'$')
                || self.options.markstream && inline::needs_markstream_markup(text)
            {
                let mut nodes = inline::parse_preview(source, body, &self.options, &self.refs);
                if self.options.footnotes {
                    self.resolve_footnote_nodes(source, &mut nodes, false);
                }
                let trimmed = trim_span(source, body);
                if !nodes.is_empty()
                    && !matches!(nodes.as_slice(),
                    [crate::Inline { kind: crate::InlineKind::Text(Text::Source(span)), .. }] if *span == trimmed)
                {
                    self.inline_preview = Some((id, body.end));
                    output.update(Change::PreviewInlines { id, nodes });
                    return;
                }
            }
        }
        if had_preview {
            output.update(Change::PreviewInlines {
                id,
                nodes: Vec::new(),
            });
        }
        self.inline_preview = None;
    }

    fn stream_fence_text(&mut self, bytes: &[u8], output: &mut Output<'_>) -> bool {
        let Some(Active::Fence(fence)) = &mut self.active else {
            return false;
        };
        if fence.lines.is_some()
            || !(self.scan == self.line_start || self.scan == fence.body.end)
            || memchr::memchr(fence.marker, &bytes[self.scan..]).is_some()
        {
            return false;
        }
        let end = bytes.len() - usize::from(bytes.last() == Some(&b'\r'));
        if let Some(at) = memchr::memrchr2(b'\n', b'\r', &bytes[self.scan..end]) {
            self.line_start = self.scan + at + 1;
        }
        let line = &bytes[self.line_start..end];
        let published = if line.len() <= 3 && line.iter().all(|&byte| byte == b' ') {
            self.line_start
        } else {
            end
        };
        emit_append(output, fence.id, &mut fence.body.end, published);
        self.metrics.scanned_bytes += end - self.scan;
        self.scan = end;
        true
    }

    fn id(&mut self) -> BlockId {
        let id = BlockId(self.next_id);
        self.next_id += 1;
        id
    }

    fn clear_preview(&mut self, output: &mut Output<'_>) {
        if let Some(id) = self.preview.take() {
            output.update(Change::Preview { id, hint: None });
        }
    }

    fn open_partial_tag(
        &mut self,
        source: &str,
        output: &mut Output<'_>,
    ) -> Result<bool, ParseError> {
        if let Some(Active::Element(element)) = &mut self.active
            && let Some(scanner) = &mut element.scanner
        {
            let (through, close) =
                scanner.advance(source, self.scan, element.tag, &mut element.depth);
            emit_append(output, element.id, &mut element.body.end, through);
            if let Some(close) = close {
                let Some(Active::Element(element)) = self.active.take() else {
                    unreachable!()
                };
                self.close_element(element, close.end, BlockStatus::Closed, output);
                self.line_start = close.end;
                return Ok(true);
            }
            return Ok(false);
        }
        if !matches!(self.active, None | Some(Active::Paragraph(_))) {
            return Ok(false);
        }
        let head = unindent(&source[self.line_start..self.scan]);
        let start = self.scan - head.len();
        if self
            .tag_probe
            .as_ref()
            .is_none_or(|probe| probe.start != start)
        {
            let Some(name) = head.strip_prefix('<') else {
                return Ok(false);
            };
            let Some(width) = self.options.custom_tags.iter().find_map(|registered| {
                let suffix = name.strip_prefix(registered)?;
                suffix
                    .as_bytes()
                    .first()
                    .filter(|byte| matches!(byte, b' ' | b'\t' | b'/' | b'>'))
                    .map(|_| registered.len())
            }) else {
                return Ok(false);
            };
            self.tag_probe = Some(tag::OpeningProbe::new(start, start + 1 + width));
        }
        let probe = self.tag_probe.as_mut().unwrap();
        let Some(end) = probe.update(source, self.scan) else {
            return Ok(false);
        };
        if tag::opening(source, start, end).is_none() {
            probe.reject();
            return Ok(false);
        }
        self.line(source, Line::new(self.line_start, end, end), output)?;
        self.line_start = end;
        self.tag_probe = None;
        Ok(true)
    }

    fn partial(&mut self, source: &str, output: &mut Output<'_>) {
        let end = self.scan;
        let start = self.line_start;
        if self.running.is_some()
            || start >= end
            || self
                .containers
                .iter()
                .any(|frame| !matches!(frame.kind, Kind::Vmr { .. }))
        {
            return;
        }
        let head = unindent(&source[start..end]);
        let byte = head.as_bytes().first().copied().unwrap_or(b' ');
        if let Some(Active::Fence(fence)) = &mut self.active {
            if fence.lines.is_none() && fence.body.end < end {
                if self
                    .fence_probe
                    .as_ref()
                    .is_none_or(|probe| probe.start != start)
                {
                    self.fence_probe = Some(FenceProbe {
                        start,
                        checked: start,
                        prefix: FencePrefix::Indent(0),
                    });
                }
                if self
                    .fence_probe
                    .as_mut()
                    .unwrap()
                    .update(source, end, fence.marker, fence.count)
                {
                    emit_append(output, fence.id, &mut fence.body.end, end);
                }
            }
            return;
        }
        if matches!(self.active, Some(Active::Element(_))) {
            return;
        }
        if let Some(Active::Paragraph(paragraph)) = &self.active
            && self.options.preview == Preview::Ai
            && self.options.syntax == Syntax::Gfm
            && paragraph.lines == 1
            && paragraph.visible
            && byte == b'|'
        {
            let start = end - head.len();
            if self
                .separator
                .as_ref()
                .is_none_or(|separator| separator.start != start)
            {
                self.separator = Some(SeparatorProbe::new(source, start, paragraph.first));
            }
            let separator = self.separator.as_mut().unwrap();
            let (pending, alignment_changed) = separator.update(source, end);
            if pending {
                if self.preview != Some(paragraph.id) || alignment_changed {
                    self.preview = Some(paragraph.id);
                    let cells = cell_spans(source, paragraph.first)
                        .map(InlineContent::Plain)
                        .collect();
                    output.update(Change::Preview {
                        id: paragraph.id,
                        hint: Some(PreviewHint::Table {
                            table: Box::new(TableData {
                                alignments: separator.alignments.clone(),
                                cells,
                            }),
                            loading: !separator.ready(),
                        }),
                    });
                }
            } else {
                self.clear_preview(output);
            }
            return;
        }
        self.clear_preview(output);
        if byte == b'#' && self.active.is_none() {
            if let Some(draft) = &mut self.draft {
                emit_append(output, draft.id, &mut draft.published, end);
            } else {
                let start = end - head.len();
                if let Some((level, content)) = atx(head, start)
                    && content.start < content.end
                {
                    let id = self.id();
                    self.draft = Some(Draft {
                        id,
                        start,
                        published: end,
                    });
                    output.open(
                        Block {
                            id,
                            span: Span::new(self.line_start, end),
                            status: BlockStatus::Open,
                            data: BlockData::Heading {
                                level,
                                content: InlineContent::Plain(Span::new(content.start, end)),
                            },
                        },
                        self.parent(),
                    );
                }
            }
            return;
        }
        let header_text =
            byte == b'|' && self.active.is_none() && (self.draft.is_some() || !blank(&head[1..]));
        let inline_text =
            self.options.markstream && (self.draft.is_some() || partial_inline_start(head));
        if !(ordinary_start(byte) || header_text || inline_text)
            || self
                .block_triggers
                .get(usize::from(byte))
                .copied()
                .unwrap_or(false)
        {
            return;
        }
        match &mut self.active {
            Some(Active::Paragraph(paragraph)) if paragraph.visible => {
                emit_append(output, paragraph.id, &mut paragraph.published, end)
            }
            None => {
                if let Some(draft) = &mut self.draft {
                    emit_append(output, draft.id, &mut draft.published, end);
                } else {
                    let id = self.id();
                    let text_start = end - head.len();
                    let span = Span::new(text_start, end);
                    let draft = Draft {
                        id,
                        start: text_start,
                        published: end,
                    };
                    self.draft = Some(draft);
                    output.open(
                        Block {
                            id,
                            span,
                            status: BlockStatus::Open,
                            data: BlockData::Paragraph(InlineContent::Plain(span)),
                        },
                        self.parent(),
                    );
                }
            }
            _ => {}
        }
    }

    fn parent(&self) -> Option<BlockId> {
        self.containers.last().map(|frame| frame.id)
    }

    fn container_line(
        &mut self,
        source: &str,
        mut line: Line,
        output: &mut Output<'_>,
    ) -> Result<(), ParseError> {
        if self.containers.is_empty() {
            let raw = &source[line.start..line.end];
            let marker = if raw.starts_with(' ') {
                unindent(raw).as_bytes().first()
            } else {
                raw.as_bytes().first()
            };
            if !matches!(marker, Some(b'>' | b'-' | b'+' | b'*' | b'0'..=b'9')) {
                return self.line(source, line, output);
            }
        }
        let physical_blank = blank(&source[line.start..line.end]);
        let mut matched = 0;
        let mut vmr_close = None;
        while let Some(frame) = self.containers.get(matched) {
            let remaining = &source[line.start..line.end];
            let prefix = match frame.kind {
                Kind::Quote => container::quote_at(remaining, line.column, line.padding),
                Kind::Vmr { .. } => {
                    vmr_close = self
                        .vmr_closer(source, line, *frame)
                        .then_some((matched, line));
                    Some(container::Prefix {
                        bytes: 0,
                        columns: 0,
                        padding: line.padding,
                    })
                }
                Kind::List { .. } => Some(container::Prefix {
                    bytes: 0,
                    columns: 0,
                    padding: line.padding,
                }),
                Kind::Item {
                    indent,
                    blank: saw_blank,
                    has_content,
                    ..
                } => {
                    if blank(remaining) {
                        Some(container::Prefix {
                            bytes: remaining.len(),
                            columns: 0,
                            padding: 0,
                        })
                    } else if saw_blank && !has_content {
                        None
                    } else {
                        let prefix = container::whitespace(
                            remaining.as_bytes(),
                            line.column,
                            line.padding,
                            indent,
                        );
                        (prefix.columns == indent).then_some(prefix)
                    }
                }
            };
            let Some(prefix) = prefix else {
                break;
            };
            matched += 1;
            line.advance(prefix);
        }

        if let Some((index, close)) = vmr_close
            && (matched < self.containers.len() || !self.vmr_leaf_protected())
        {
            return self.close_vmr(source, close, index, output);
        }

        if matched < self.containers.len() {
            let remaining = &source[line.start..line.end];
            let starts_container = container::quote_at(remaining, line.column, line.padding)
                .is_some()
                || container::list_at(remaining, line.column, line.padding).is_some()
                || self.options.markstream && self.vmr_opener(source, line);
            if matches!(self.active, Some(Active::Paragraph(_)))
                && !blank(remaining)
                && !starts_container
                && !interrupts_paragraph(unindent(remaining))
            {
                self.extend_containers(line.next);
                return self.line(source, Line { lazy: true, ..line }, output);
            }

            self.stop_leaf(source, output)?;
            if matched > 0
                && let Kind::List { style, .. } = self.containers[matched - 1].kind
                && !container::list_at(remaining, line.column, line.padding).is_some_and(
                    |(marker, _)| marker.style == style && !thematic(unindent(remaining)),
                )
            {
                matched -= 1;
            }
            let previous_blank = self
                .containers
                .get(matched)
                .is_some_and(|frame| matches!(frame.kind, Kind::Item { blank: true, .. }));
            self.close_containers(matched, output);
            if previous_blank
                && let Some(Frame {
                    kind: Kind::List { tight, .. },
                    ..
                }) = self.containers.last_mut()
            {
                *tight = false;
            }
        } else if matches!(
            self.active,
            Some(
                Active::Fence(_)
                    | Active::Element(_)
                    | Active::Html(_)
                    | Active::Math(_)
                    | Active::Footnote(_)
            )
        ) || self.running.is_some()
        {
            self.extend_containers(line.next);
            return self.line(source, line, output);
        }

        if !blank(&source[line.start..line.end]) && !self.containers.is_empty() {
            let ancestors = self.containers.len() - 1;
            for frame in &mut self.containers[..ancestors] {
                if let Kind::Item { blank, .. } = &mut frame.kind {
                    *blank = false;
                }
            }
        }
        let mut opened_item = false;
        loop {
            let remaining = &source[line.start..line.end];
            if let Some(prefix) = container::quote_at(remaining, line.column, line.padding) {
                self.stop_leaf(source, output)?;
                self.mark_item_content();
                self.open_container(Kind::Quote, line.start, line.next, output);
                line.advance(prefix);
                continue;
            }
            let Some((marker, prefix)) = container::list_at(remaining, line.column, line.padding)
            else {
                break;
            };
            if thematic(unindent(remaining)) {
                break;
            }
            if matches!(self.active, Some(Active::Paragraph(_))) && !marker.interrupts(remaining) {
                break;
            }
            self.stop_leaf(source, output)?;
            self.mark_item_content();
            if !matches!(self.containers.last().map(|frame| frame.kind), Some(Kind::List { style, .. }) if style == marker.style)
            {
                self.open_container(
                    Kind::List {
                        style: marker.style,
                        ordinal: marker.ordinal,
                        tight: true,
                    },
                    line.start,
                    line.next,
                    output,
                );
            }
            let content = &remaining[marker.content..];
            let (task, marker_length) = if self.options.syntax == Syntax::Gfm {
                task_marker(content)
            } else {
                (None, 0)
            };
            self.open_container(
                Kind::Item {
                    indent: marker.indent,
                    blank: false,
                    has_content: task.is_some(),
                    task,
                },
                line.start,
                line.next,
                output,
            );
            line.advance(prefix);
            line.start += marker_length;
            line.column += marker_length;
            opened_item = true;
        }

        if blank(&source[line.start..line.end]) {
            if !opened_item
                && (physical_blank
                    || matches!(
                        self.containers.last().map(|frame| frame.kind),
                        Some(Kind::Item { .. })
                    ))
            {
                let start = if physical_blank {
                    0
                } else {
                    self.containers
                        .iter()
                        .rposition(|frame| matches!(frame.kind, Kind::Quote))
                        .map_or(0, |index| index + 1)
                };
                for frame in &mut self.containers[start..] {
                    if let Kind::Item {
                        blank: saw_blank, ..
                    } = &mut frame.kind
                    {
                        *saw_blank = true;
                    }
                }
            }
        } else {
            self.mark_item_content();
        }
        self.extend_containers(line.next);
        self.line(source, line, output)
    }

    fn mark_item_content(&mut self) {
        let Some((index, frame)) = self.containers.iter_mut().enumerate().next_back() else {
            return;
        };
        if let Kind::Item {
            blank, has_content, ..
        } = &mut frame.kind
        {
            let was_blank = *blank;
            *has_content = true;
            *blank = false;
            if was_blank && let Kind::List { tight, .. } = &mut self.containers[index - 1].kind {
                *tight = false;
            }
        }
    }

    fn open_container(&mut self, kind: Kind, start: usize, end: usize, output: &mut Output<'_>) {
        let frame = Frame {
            id: self.id(),
            batch_index: output.batch_index(),
            start,
            end,
            kind,
        };
        output.done(
            container_block(frame, BlockStatus::Open),
            false,
            self.parent(),
        );
        self.containers.push(frame);
    }

    fn close_containers(&mut self, depth: usize, output: &mut Output<'_>) {
        while self.containers.len() > depth {
            let frame = self.containers.pop().unwrap();
            output.close_container(
                frame.batch_index,
                container_block(frame, BlockStatus::Closed),
            );
        }
    }

    fn extend_containers(&mut self, end: usize) {
        for frame in &mut self.containers {
            frame.end = end;
        }
    }

    fn stop_leaf(&mut self, source: &str, output: &mut Output<'_>) -> Result<(), ParseError> {
        if let Some(active) = self.active.take() {
            self.end_active(source, active, BlockStatus::Incomplete, output)?;
        }
        self.clear_preview(output);
        Ok(())
    }

    fn line(
        &mut self,
        source: &str,
        line: Line,
        output: &mut Output<'_>,
    ) -> Result<(), ParseError> {
        if let Some(running) = self.running.take() {
            return self.resume_plugin(source, running, line.next, Phase::Advance, output);
        }

        let raw = &source[line.start..line.end];
        let leading = container::whitespace(raw.as_bytes(), line.column, line.padding, 4);
        let head = if leading.columns < 4 {
            &raw[leading.bytes..]
        } else {
            raw
        };
        let start = line.end - head.len();

        if self.options.markstream
            && head.starts_with(':')
            && self.open_vmr_line(source, line, output)?
        {
            return Ok(());
        }
        if self.options.footnotes
            && head.starts_with("[^")
            && self.try_open_footnote(source, line, start, output)?
        {
            return Ok(());
        }
        match self.active.take() {
            Some(Active::Footnote(note)) => {
                if self.advance_footnote(source, note, line, output)? {
                    return Ok(());
                }
                return self.line(source, line, output);
            }
            Some(Active::Math(math)) => {
                return self.display_line(source, line, line.start, math, output);
            }
            Some(Active::Fence(mut fence)) => {
                if closing_fence(head, fence.marker, fence.count) {
                    self.close_code(fence, line.next, BlockStatus::Closed, output);
                } else {
                    if let Some(lines) = &mut fence.lines {
                        let mut body = line;
                        body.advance(container::whitespace(
                            raw.as_bytes(),
                            line.column,
                            line.padding,
                            fence.indent,
                        ));
                        let body = body.raw();
                        fence.body.end = line.next;
                        if self.batch {
                            lines.push(body);
                        } else {
                            output.update(Change::AddRawLine {
                                id: fence.id,
                                line: body,
                                end: line.next,
                            });
                        }
                    } else {
                        emit_append(output, fence.id, &mut fence.body.end, line.next);
                    }
                    self.active = Some(Active::Fence(fence));
                }
                return Ok(());
            }
            Some(Active::Element(mut element)) => {
                let close = match &mut element.scanner {
                    Some(scanner) => scanner.finish_line(
                        source,
                        line.end,
                        line.next,
                        element.tag,
                        &mut element.depth,
                    ),
                    None => tag::close(
                        source,
                        line.start,
                        line.end,
                        element.tag,
                        &mut element.depth,
                    ),
                };
                if let Some(close) = close {
                    let committed_body = if close.start == line.start && !self.containers.is_empty()
                    {
                        element.body.end
                    } else {
                        close.start
                    };
                    emit_append(output, element.id, &mut element.body.end, committed_body);
                    self.close_element(element, close.end, BlockStatus::Closed, output);
                    if !source[close.end..line.end].trim().is_empty() {
                        self.line(
                            source,
                            Line {
                                start: close.end,
                                ..line
                            },
                            output,
                        )?;
                    }
                } else {
                    emit_append(output, element.id, &mut element.body.end, line.next);
                    self.active = Some(Active::Element(element));
                }
                return Ok(());
            }
            Some(Active::Html(mut html)) => {
                let line_end = html.end.closes(raw);
                let break_blank = line_end && matches!(html.end, HtmlEnd::Blank);
                if !break_blank {
                    if let Some(lines) = &mut html.lines {
                        let body = line.raw();
                        html.span.end = line.next;
                        if self.batch {
                            lines.push(body);
                        } else {
                            output.update(Change::AddRawLine {
                                id: html.id,
                                line: body,
                                end: line.next,
                            });
                        }
                    } else {
                        emit_append(output, html.id, &mut html.span.end, line.next);
                    }
                }
                if line_end {
                    self.close_html(html, BlockStatus::Closed, output);
                    if break_blank {
                        return self.line(source, line, output);
                    }
                } else {
                    self.active = Some(Active::Html(html));
                }
                return Ok(());
            }
            Some(Active::Indented(mut code)) => {
                if blank(raw) {
                    let raw_line = if leading.columns == 4 {
                        let mut body = line;
                        body.advance(leading);
                        body.raw()
                    } else {
                        RawLine {
                            span: Span::new(line.end, line.next),
                            leading_spaces: 0,
                        }
                    };
                    code.pending_blank.push(raw_line);
                    self.active = Some(Active::Indented(code));
                    return Ok(());
                }
                if leading.columns == 4 {
                    let mut body = line;
                    body.advance(leading);
                    code.lines.append(&mut code.pending_blank);
                    code.lines.push(body.raw());
                    code.span.end = line.next;
                    self.active = Some(Active::Indented(code));
                    return Ok(());
                }
                self.close_indented(code, output);
            }
            Some(Active::Table(mut table)) => {
                if !blank(head) && !interrupts_paragraph(head) {
                    let spans = cell_spans(source, line.span())
                        .take(table.columns)
                        .chain(std::iter::repeat(Span::new(line.end, line.end)))
                        .take(table.columns);
                    table.span.end = line.next;
                    if let Some(data) = &mut table.batch_data {
                        data.cells.extend(spans.map(InlineContent::Plain));
                    } else {
                        let mut cells = Vec::with_capacity(table.columns);
                        for (column, span) in spans.enumerate() {
                            cells.push(self.stream_cell(
                                source,
                                table.id,
                                table.next_cell + column,
                                span,
                            )?);
                        }
                        output.update(Change::AddRow {
                            id: table.id,
                            end: line.next,
                            cells,
                        });
                    }
                    table.next_cell += table.columns;
                    self.active = Some(Active::Table(table));
                    return Ok(());
                }
                self.end_active(source, Active::Table(table), BlockStatus::Closed, output)?;
            }
            Some(Active::Paragraph(mut paragraph)) => {
                if !line.lazy
                    && let Some(level) = setext(head)
                {
                    self.clear_preview(output);
                    if let Some(paragraph) = self.resolve_definitions(source, paragraph, output)? {
                        let span = Span::new(paragraph.span.start, line.next);
                        let visible = paragraph.visible;
                        return self.finish_leaf(
                            source,
                            paragraph.leaf(span, Some(level)),
                            visible,
                            output,
                        );
                    }
                    return self.line(source, line, output);
                }
                if self.options.syntax == Syntax::Gfm && paragraph.lines == 1 && head.contains('|')
                {
                    let head_cells = cell_spans(source, paragraph.first);
                    if let Some(alignments) = separator_alignments(source, line.span())
                        && alignments.len() == head_cells.clone().count()
                    {
                        self.clear_preview(output);
                        let columns = alignments.len();
                        let mut cells = Vec::with_capacity(columns);
                        for (index, span) in head_cells.enumerate() {
                            cells.push(if self.batch {
                                InlineContent::Plain(span)
                            } else {
                                self.stream_cell(source, paragraph.id, index, span)?
                            });
                        }
                        let span = Span::new(paragraph.span.start, line.next);
                        let data = Box::new(TableData { alignments, cells });
                        let batch_data = if self.batch {
                            Some(data)
                        } else {
                            let block = Block {
                                id: paragraph.id,
                                span,
                                status: BlockStatus::Open,
                                data: BlockData::Table(data),
                            };
                            if paragraph.visible {
                                output.update(Change::Replace(block));
                            } else {
                                output.open(block, self.parent());
                            }
                            None
                        };
                        let table = Table {
                            id: paragraph.id,
                            span,
                            columns,
                            next_cell: columns,
                            batch_data,
                        };
                        self.active = Some(Active::Table(table));
                        return Ok(());
                    }
                }
                self.clear_preview(output);
                if !blank(head)
                    && !interrupts_paragraph(head)
                    && !self.is_custom_start(source, start, line.end)
                    && !(self.options.math && math_block::opening(head).is_some())
                {
                    paragraph.lines += 1;
                    if paragraph.parts.is_empty() && line.start == paragraph.next {
                        if paragraph.visible {
                            emit_append(output, paragraph.id, &mut paragraph.published, line.end);
                        }
                    } else {
                        if paragraph.parts.is_empty() {
                            if paragraph.visible {
                                emit_append(
                                    output,
                                    paragraph.id,
                                    &mut paragraph.published,
                                    paragraph.next,
                                );
                            }
                            paragraph
                                .parts
                                .push(Span::new(paragraph.span.start, paragraph.next));
                        } else if paragraph.visible {
                            output.update(Change::AddTextFragment {
                                id: paragraph.id,
                                span: Span::new(paragraph.span.end, paragraph.next),
                            });
                        }
                        if paragraph.visible {
                            output.update(Change::AddTextFragment {
                                id: paragraph.id,
                                span: line.span(),
                            });
                        }
                        if let Some(last) = paragraph
                            .parts
                            .last_mut()
                            .filter(|last| last.end == line.start)
                        {
                            last.end = line.next;
                        } else {
                            paragraph.parts.push(Span::new(line.start, line.next));
                        }
                    }
                    paragraph.span.end = line.end;
                    paragraph.next = line.next;
                    self.active = Some(Active::Paragraph(paragraph));
                    return Ok(());
                }
                self.end_active(
                    source,
                    Active::Paragraph(paragraph),
                    BlockStatus::Closed,
                    output,
                )?;
            }
            None => {}
        }

        if blank(head) {
            return Ok(());
        }

        if leading.columns == 4 {
            let id = self.id();
            let mut body = line;
            body.advance(leading);
            self.active = Some(Active::Indented(Indented {
                id,
                span: Span::new(line.start, line.next),
                lines: vec![body.raw()],
                pending_blank: Vec::new(),
            }));
            return Ok(());
        }

        if self.options.math
            && let Some(delimiter) = math_block::opening(head)
        {
            return self.start_display(source, line, start, delimiter, output);
        }
        if let Some((level, content)) = atx(head, start) {
            let (id, present) = self.block_id(start);
            return self.finish_leaf(
                source,
                Dependent {
                    id,
                    span: Span::new(line.start, line.next),
                    content,
                    heading: Some(level),
                    parts: None,
                },
                present,
                output,
            );
        }
        if let Some((marker, count, info)) = opening_fence(head, start) {
            let id = self.id();
            let body = Span::new(line.next, line.next);
            let indent = leading.columns;
            let segmented = indent != 0 || !self.containers.is_empty();
            let fence = Fence {
                id,
                start: line.start,
                info,
                marker,
                count,
                indent,
                body,
                lines: segmented.then(Vec::new),
            };
            let data = if segmented {
                BlockData::SegmentedCode {
                    info,
                    lines: Vec::new(),
                }
            } else {
                BlockData::CodeBlock { info, body }
            };
            output.open(
                Block {
                    id,
                    span: Span::new(line.start, line.next),
                    status: BlockStatus::Open,
                    data,
                },
                self.parent(),
            );
            self.active = Some(Active::Fence(fence));
            return Ok(());
        }
        if thematic(head) {
            let (id, present) = self.block_id(start);
            output.done(
                Block {
                    id,
                    span: Span::new(line.start, line.next),
                    status: BlockStatus::Closed,
                    data: BlockData::ThematicBreak,
                },
                present,
                self.parent(),
            );
            return Ok(());
        }
        if head.starts_with('<')
            && !self.options.custom_tags.is_empty()
            && let Some(opener) = tag::opening(source, start, line.end)
            && self
                .options
                .custom_tags
                .iter()
                .any(|name| name == opener.tag.tag_name(source))
        {
            let id = self.id();
            let mut element = Element {
                id,
                start,
                tag: opener.tag,
                depth: 1,
                body: Span::new(opener.end, opener.end),
                scanner: None,
            };
            if opener.self_closing {
                self.emit_element(element, opener.end, BlockStatus::Closed, false, output);
                if !source[opener.end..line.end].trim().is_empty() {
                    self.line(
                        source,
                        Line {
                            start: opener.end,
                            ..line
                        },
                        output,
                    )?;
                }
            } else if let Some(close) = tag::close(
                source,
                opener.end,
                line.end,
                element.tag,
                &mut element.depth,
            ) {
                element.body.end = close.start;
                self.emit_element(element, close.end, BlockStatus::Closed, false, output);
                if !source[close.end..line.end].trim().is_empty() {
                    self.line(
                        source,
                        Line {
                            start: close.end,
                            ..line
                        },
                        output,
                    )?;
                }
            } else {
                element.body.end = line.next;
                if !self.batch && self.containers.is_empty() {
                    element.scanner = Some(Box::new(tag::BodyScanner::new(line.next)));
                }
                output.open(
                    Block {
                        id,
                        span: Span::new(start, line.next),
                        status: BlockStatus::Open,
                        data: BlockData::CustomTag {
                            tag: element.tag,
                            body: element.body,
                        },
                    },
                    self.parent(),
                );
                self.active = Some(Active::Element(element));
            }
            return Ok(());
        }
        if let Some(end) = html_start(head, true) {
            let id = self.id();
            let span = Span::new(line.start, line.next);
            let segmented = self.parent().is_some();
            let first_line = segmented.then(|| vec![line.raw()]);
            if end.closes(raw) {
                let data = first_line.map_or(BlockData::Html, BlockData::SegmentedHtml);
                output.done(
                    Block {
                        id,
                        span,
                        status: BlockStatus::Closed,
                        data,
                    },
                    false,
                    self.parent(),
                );
            } else {
                let lines = if self.batch {
                    first_line
                } else {
                    segmented.then(Vec::new)
                };
                let html = Html {
                    id,
                    span,
                    end,
                    lines,
                };
                let data = (segmented && !self.batch)
                    .then(|| vec![line.raw()])
                    .map_or(BlockData::Html, BlockData::SegmentedHtml);
                output.open(
                    Block {
                        id,
                        span,
                        status: BlockStatus::Open,
                        data,
                    },
                    self.parent(),
                );
                self.active = Some(Active::Html(html));
            }
            return Ok(());
        }
        if let Some(byte) = head.as_bytes().first().filter(|byte| byte.is_ascii())
            && self.block_triggers[usize::from(*byte)]
            && self.start_plugin(source, start, line.next, *byte, output)?
        {
            return Ok(());
        }

        let draft = self.draft;
        let (id, _) = self.block_id(start);
        let visible = !self.batch && !(head.starts_with('[') && head.contains("]:"));
        let span = Span::new(start, line.end);
        if let Some(mut draft) = draft.filter(|draft| draft.id == id) {
            emit_append(output, id, &mut draft.published, line.end);
        } else if visible {
            output.open(
                Block {
                    id,
                    span,
                    status: BlockStatus::Open,
                    data: BlockData::Paragraph(InlineContent::Plain(span)),
                },
                self.parent(),
            );
        }
        self.active = Some(Active::Paragraph(Paragraph {
            id,
            span,
            first: span,
            published: line.end,
            visible,
            lines: 1,
            next: line.next,
            parts: Vec::new(),
        }));
        Ok(())
    }

    fn block_id(&mut self, start: usize) -> (BlockId, bool) {
        if let Some(draft) = self.draft.take().filter(|draft| draft.start == start) {
            (draft.id, true)
        } else {
            (self.id(), false)
        }
    }

    fn is_custom_start(&self, source: &str, start: usize, end: usize) -> bool {
        !self.options.custom_tags.is_empty()
            && source.as_bytes().get(start) == Some(&b'<')
            && tag::opening(source, start, end).is_some_and(|opening| {
                self.options
                    .custom_tags
                    .iter()
                    .any(|name| name == opening.tag.tag_name(source))
            })
    }

    fn end_active(
        &mut self,
        source: &str,
        active: Active,
        status: BlockStatus,
        output: &mut Output<'_>,
    ) -> Result<(), ParseError> {
        match active {
            Active::Paragraph(p) => {
                if let Some(p) = self.resolve_definitions(source, p, output)? {
                    let span = p.span;
                    let visible = p.visible;
                    self.finish_leaf(source, p.leaf(span, None), visible, output)?;
                }
            }
            Active::Math(math) => self.display_fallback(source, math, output)?,
            Active::Footnote(note) => self.finish_footnote(source, note, output)?,
            Active::Fence(fence) => {
                let end = fence.body.end;
                self.close_code(fence, end, status, output);
            }
            Active::Element(element) => {
                let end = element.body.end;
                self.close_element(element, end, status, output)
            }
            Active::Html(html) => {
                let status = if matches!(html.end, HtmlEnd::Blank) {
                    BlockStatus::Closed
                } else {
                    status
                };
                self.close_html(html, status, output);
            }
            Active::Indented(code) => self.close_indented(code, output),
            Active::Table(table) => {
                if let Some(data) = table.batch_data {
                    output.done(
                        Block {
                            id: table.id,
                            span: table.span,
                            status: BlockStatus::Closed,
                            data: BlockData::Table(data),
                        },
                        true,
                        self.parent(),
                    );
                } else {
                    output.update(Change::SetStatus {
                        id: table.id,
                        status: BlockStatus::Closed,
                        end: table.span.end,
                    });
                }
            }
        }
        Ok(())
    }

    fn start_display(
        &mut self,
        source: &str,
        line: Line,
        start: usize,
        delimiter: math_block::Delimiter,
        output: &mut Output<'_>,
    ) -> Result<(), ParseError> {
        let (id, present) = self.block_id(start);
        let content = start + delimiter.width();
        let math = DisplayMath {
            id,
            start,
            delimiter,
            body: Span::new(content, content),
            line_next: content,
            end: content,
            present,
            shown: false,
            evidence: false,
            checked: content,
            parts: Vec::new(),
        };
        self.display_line(source, line, content, math, output)
    }

    fn display_line(
        &mut self,
        source: &str,
        line: Line,
        start: usize,
        mut math: DisplayMath,
        output: &mut Output<'_>,
    ) -> Result<(), ParseError> {
        if let Some(close) = math.delimiter.closes(source, start, line.end) {
            let same_line = start == math.start + math.delimiter.width();
            math.add(source, start, close.start, close.start);
            let span = Span::new(math.start, close.end);
            let id = math.id;
            let present = math.present;
            let delimiter = math.delimiter;
            let body = math.into_text(source);
            if delimiter.accepts(body.as_str(source), same_line) {
                output.done(
                    Block {
                        id,
                        span,
                        status: BlockStatus::Closed,
                        data: BlockData::Math { body },
                    },
                    present,
                    self.parent(),
                );
            } else {
                self.finish_leaf(
                    source,
                    Dependent {
                        id,
                        span,
                        content: span,
                        heading: None,
                        parts: None,
                    },
                    present,
                    output,
                )?;
            }
            if !blank(&source[close.end..line.end]) {
                return self.line(
                    source,
                    Line {
                        start: close.end,
                        ..line
                    },
                    output,
                );
            }
        } else {
            let old_end = math.body.end;
            math.add(source, start, line.end, line.next);
            self.publish_display_progress(&mut math, old_end, output);
            self.active = Some(Active::Math(math));
        }
        Ok(())
    }

    fn publish_display_progress(
        &self,
        math: &mut DisplayMath,
        old_end: usize,
        output: &mut Output<'_>,
    ) {
        if self.batch || !math.evidence || !math.parts.is_empty() {
            return;
        }
        if !math.shown {
            let block = Block {
                id: math.id,
                span: Span::new(math.start, math.end),
                status: BlockStatus::Open,
                data: BlockData::Math {
                    body: Text::Source(math.body),
                },
            };
            if math.present {
                output.update(Change::Replace(block));
            } else {
                output.open(block, self.parent());
                math.present = true;
            }
            math.shown = true;
        } else if old_end < math.body.end {
            output.append_text(math.id, Span::new(old_end, math.body.end));
        }
    }

    fn display_fallback(
        &mut self,
        source: &str,
        math: DisplayMath,
        output: &mut Output<'_>,
    ) -> Result<(), ParseError> {
        let span = trim_span(source, Span::new(math.start, math.end));
        self.finish_leaf(
            source,
            Dependent {
                id: math.id,
                span,
                content: span,
                heading: None,
                parts: None,
            },
            math.present,
            output,
        )
    }

    fn close_html(&self, html: Html, status: BlockStatus, output: &mut Output<'_>) {
        if self.batch {
            let data = html.lines.map_or(BlockData::Html, BlockData::SegmentedHtml);
            output.done(
                Block {
                    id: html.id,
                    span: html.span,
                    status,
                    data,
                },
                true,
                self.parent(),
            );
        } else {
            output.update(Change::SetStatus {
                id: html.id,
                status,
                end: html.span.end,
            });
        }
    }

    fn close_indented(&self, code: Indented, output: &mut Output<'_>) {
        output.done(
            Block {
                id: code.id,
                span: code.span,
                status: BlockStatus::Closed,
                data: BlockData::IndentedCode { lines: code.lines },
            },
            false,
            self.parent(),
        );
    }

    fn close_code(&self, fence: Fence, end: usize, status: BlockStatus, output: &mut Output<'_>) {
        if self.batch {
            let data = match fence.lines {
                Some(lines) => BlockData::SegmentedCode {
                    info: fence.info,
                    lines,
                },
                None => BlockData::CodeBlock {
                    info: fence.info,
                    body: fence.body,
                },
            };
            output.done(
                Block {
                    id: fence.id,
                    span: Span::new(fence.start, end),
                    status,
                    data,
                },
                true,
                self.parent(),
            );
        } else {
            output.update(Change::SetStatus {
                id: fence.id,
                status,
                end,
            });
        }
    }

    fn close_element(
        &self,
        element: Element,
        end: usize,
        status: BlockStatus,
        output: &mut Output<'_>,
    ) {
        if self.batch {
            self.emit_element(element, end, status, true, output);
        } else {
            output.update(Change::SetStatus {
                id: element.id,
                status,
                end,
            });
        }
    }

    fn emit_element(
        &self,
        element: Element,
        end: usize,
        status: BlockStatus,
        present: bool,
        output: &mut Output<'_>,
    ) {
        output.done(
            Block {
                id: element.id,
                span: Span::new(element.start, end),
                status,
                data: BlockData::CustomTag {
                    tag: element.tag,
                    body: element.body,
                },
            },
            present,
            self.parent(),
        );
    }

    fn resolve_definitions(
        &mut self,
        source: &str,
        mut paragraph: Paragraph,
        output: &mut Output<'_>,
    ) -> Result<Option<Paragraph>, ParseError> {
        if !paragraph.parts.is_empty()
            || source.as_bytes().get(paragraph.span.start) != Some(&b'[')
            || self.options.footnotes
                && source[paragraph.span.start..paragraph.span.end].starts_with("[^")
        {
            return Ok(Some(paragraph));
        }
        let mut start = paragraph.span.start;
        while let Some((label, reference, next)) =
            reference_prefix(source, start, paragraph.span.end)
        {
            self.publish_reference(source, label, reference, output)?;
            start = next;
        }
        if start == paragraph.span.end {
            if paragraph.visible {
                output.update(Change::Remove { id: paragraph.id });
            }
            return Ok(None);
        }
        paragraph.span.start = start;
        Ok(Some(paragraph))
    }

    fn publish_reference(
        &mut self,
        source: &str,
        label: &str,
        reference: Reference,
        output: &mut Output<'_>,
    ) -> Result<(), ParseError> {
        let label = inline::normalize_label(label);
        if self.refs.insert(label.clone(), reference)
            && !self.batch
            && let Some(waiters) = self.waiting.remove(label.as_ref())
        {
            let mut previous = None;
            for dependent in waiters {
                let id = dependent.key().0;
                if previous != Some(id) {
                    self.metrics.invalidated_blocks += 1;
                    previous = Some(id);
                }
                match dependent {
                    Dependency::Leaf(leaf) => self.finish_leaf(source, leaf, true, output)?,
                    Dependency::Footnote(body) => {
                        self.finish_footnote_body(source, body, output)?
                    }
                    Dependency::Cell { id, index, span } => {
                        let content = self.stream_cell(source, id, index, span)?;
                        output.update(Change::ReplaceTableCell { id, index, content });
                    }
                }
            }
        }
        Ok(())
    }

    fn finish_leaf(
        &mut self,
        source: &str,
        leaf: Dependent,
        present: bool,
        output: &mut Output<'_>,
    ) -> Result<(), ParseError> {
        self.unresolved.clear();
        self.clear_footnote_unresolved();
        let content = match &leaf.parts {
            Some(parts) if self.batch => {
                self.batch_parts.insert(leaf.id, parts.clone());
                InlineContent::Plain(leaf.content)
            }
            Some(parts) => self.parse_parts(source, parts, true)?,
            None => self.inline(source, leaf.content)?,
        };
        if self.options.footnotes {
            self.track_footnote_dependency(&Dependency::Leaf(leaf.clone()));
        }
        if !self.unresolved.is_empty() {
            self.track_dependency(Dependency::Leaf(leaf.clone()));
        }
        output.done(
            Block {
                id: leaf.id,
                span: leaf.span,
                status: BlockStatus::Closed,
                data: match leaf.heading {
                    Some(level) => BlockData::Heading { level, content },
                    None => BlockData::Paragraph(content),
                },
            },
            present,
            self.parent(),
        );
        Ok(())
    }

    fn track_dependency(&mut self, dependency: Dependency) {
        let key = dependency.key();
        self.unresolved.sort_unstable();
        self.unresolved.dedup();
        for label in self.unresolved.drain(..) {
            let waiters = self.waiting.entry(label).or_default();
            if let Err(index) = waiters.binary_search_by_key(&key, Dependency::key) {
                waiters.insert(index, dependency.clone());
            }
        }
    }

    fn inline(&mut self, source: &str, span: Span) -> Result<InlineContent, ParseError> {
        self.unresolved.clear();
        if self.batch {
            return Ok(InlineContent::Plain(span));
        }
        self.parse_inline(source, span, true)
    }

    fn parse_inline(
        &mut self,
        source: &str,
        span: Span,
        track: bool,
    ) -> Result<InlineContent, ParseError> {
        let span = trim_span(source, span);
        self.metrics.inline_bytes += span.end - span.start;
        let text = &source[Range::from(span)];
        let plugin_hit = self.plugins.iter().any(|registered| {
            registered.rule.scope == Scope::Inline
                && text.as_bytes().contains(&registered.rule.trigger)
        });
        if !(plugin_hit
            || inline::needs_markup(text, self.options.syntax)
            || self.options.math && text.as_bytes().contains(&b'$')
            || self.options.markstream && inline::needs_markstream_markup(text))
        {
            return Ok(InlineContent::Plain(span));
        }
        let mut nodes = if track {
            inline::parse_tracked(
                source,
                span,
                &self.options,
                &self.refs,
                &mut self.unresolved,
            )
        } else {
            inline::parse(source, span, &self.options, &self.refs)
        };
        if self.options.footnotes {
            self.resolve_footnote_nodes(source, &mut nodes, track);
        }
        // Registered inline rules share the same source coordinates as built-in nodes.
        if plugin_hit {
            return self
                .apply_inline_plugins(source, nodes)
                .map(InlineContent::Parsed);
        }
        Ok(InlineContent::Parsed(nodes))
    }

    fn parse_parts(
        &mut self,
        source: &str,
        parts: &[Span],
        track: bool,
    ) -> Result<InlineContent, ParseError> {
        self.metrics.inline_bytes += parts
            .iter()
            .map(|part| part.end - part.start)
            .sum::<usize>();
        let unresolved = track.then_some(&mut self.unresolved);
        let mut nodes =
            inline::parse_fragments(source, parts, &self.options, &self.refs, unresolved);
        if self.options.footnotes {
            self.resolve_footnote_nodes(source, &mut nodes, track);
        }
        if self
            .plugins
            .iter()
            .any(|entry| entry.rule.scope == Scope::Inline)
        {
            return self
                .apply_inline_plugins(source, nodes)
                .map(InlineContent::Parsed);
        }
        Ok(InlineContent::Parsed(nodes))
    }

    fn stream_cell(
        &mut self,
        source: &str,
        id: BlockId,
        index: usize,
        span: Span,
    ) -> Result<InlineContent, ParseError> {
        self.unresolved.clear();
        self.clear_footnote_unresolved();
        let content = self.table_cell(source, span, true)?;
        if self.options.footnotes {
            self.track_footnote_dependency(&Dependency::Cell { id, index, span });
        }
        if !self.unresolved.is_empty() {
            self.track_dependency(Dependency::Cell { id, index, span });
        }
        Ok(content)
    }

    fn table_cell(
        &mut self,
        source: &str,
        span: Span,
        track: bool,
    ) -> Result<InlineContent, ParseError> {
        let mut start = span.start;
        let mut parts = Vec::new();
        for at in memchr::memchr_iter(b'|', &source.as_bytes()[span.start..span.end]) {
            let pipe = span.start + at;
            let escapes = source.as_bytes()[start..pipe]
                .iter()
                .rev()
                .take_while(|&&byte| byte == b'\\')
                .count();
            if escapes % 2 == 1 {
                parts.push(Span::new(start, pipe - 1));
                start = pipe;
            }
        }
        if parts.is_empty() {
            return self.parse_inline(source, span, track);
        }
        parts.push(Span::new(start, span.end));
        self.parse_parts(source, &parts, track)
    }

    pub(crate) fn resolve_batch(
        &mut self,
        source: &str,
        nodes: &mut [Block],
    ) -> Result<(), ParseError> {
        for block in nodes {
            match &mut block.data {
                BlockData::Paragraph(InlineContent::Plain(span))
                | BlockData::Footnote {
                    content: InlineContent::Plain(span),
                    ..
                }
                | BlockData::Heading {
                    content: InlineContent::Plain(span),
                    ..
                } => {
                    let content = match self.batch_parts.remove(&block.id) {
                        Some(parts) => self.parse_parts(source, &parts, false)?,
                        None => self.parse_inline(source, *span, false)?,
                    };
                    match &mut block.data {
                        BlockData::Paragraph(node)
                        | BlockData::Heading { content: node, .. }
                        | BlockData::Footnote { content: node, .. } => *node = content,
                        _ => unreachable!(),
                    }
                }
                BlockData::Table(data) => {
                    for cell in &mut data.cells {
                        if let InlineContent::Plain(span) = cell {
                            *cell = self.table_cell(source, *span, false)?;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn start_plugin(
        &mut self,
        source: &str,
        start: usize,
        end: usize,
        byte: u8,
        output: &mut Output<'_>,
    ) -> Result<bool, ParseError> {
        for index in 0..self.plugins.len() {
            let rule = self.plugins[index].rule;
            if rule.scope != Scope::Block || rule.trigger != byte {
                continue;
            }
            let running = PluginMatch {
                index,
                start,
                cursor: start,
            };
            if !self.drive_plugin(source, running, end, Phase::Probe, output)? {
                continue;
            }
            return Ok(true);
        }
        Ok(false)
    }

    fn resume_plugin(
        &mut self,
        source: &str,
        running: PluginMatch,
        end: usize,
        phase: Phase,
        output: &mut Output<'_>,
    ) -> Result<(), ParseError> {
        self.drive_plugin(source, running, end, phase, output)?;
        Ok(())
    }

    fn drive_plugin(
        &mut self,
        source: &str,
        mut running: PluginMatch,
        end: usize,
        initial: Phase,
        output: &mut Output<'_>,
    ) -> Result<bool, ParseError> {
        let plugin = &mut self.plugins[running.index];
        let kind = plugin.rule.kind;
        let mut probe = initial == Phase::Probe;
        let final_input = initial == Phase::Finish;
        loop {
            let phase = if probe {
                Phase::Probe
            } else if final_input && end - running.cursor <= plugin.implementation.max_input_bytes()
            {
                Phase::Finish
            } else {
                Phase::Advance
            };
            probe = false;
            let result = plugin_step(plugin, source, running.cursor, end, phase)?;
            match result {
                accepted @ (RuleResult::Accept { consumed }
                | RuleResult::AcceptWithFields { consumed, .. }) => {
                    let stop = running.cursor + consumed;
                    if stop == running.start {
                        return Err(
                            PluginError::new("plugin accepted without consuming input").into()
                        );
                    }
                    let span = Span::new(running.start, stop);
                    let data = match accepted {
                        RuleResult::AcceptWithFields { fields, .. } if !fields.is_empty() => {
                            crate::plugin::validate_fields(&fields, &source[Range::from(span)])?;
                            BlockData::ExtensionWithFields { kind, fields }
                        }
                        _ => BlockData::Extension { kind },
                    };
                    self.emit_extension(data, span, output);
                    if !source[stop..end].trim().is_empty() {
                        let end_of_line = source[..end].trim_end_matches(['\r', '\n']).len();
                        self.line(source, Line::new(stop, end_of_line, end), output)?;
                    }
                    return Ok(true);
                }
                RuleResult::Reject if phase == Phase::Probe => return Ok(false),
                RuleResult::NeedMore { consumed } if phase != Phase::Finish => {
                    running.cursor += consumed;
                    if consumed == 0 || running.cursor == end {
                        self.running = Some(running);
                        return Ok(true);
                    }
                }
                RuleResult::Reject | RuleResult::NeedMore { .. } => {
                    let id = self.id();
                    let span = Span::new(running.start, end);
                    self.finish_leaf(
                        source,
                        Dependent {
                            id,
                            span,
                            content: span,
                            heading: None,
                            parts: None,
                        },
                        false,
                        output,
                    )?;
                    return Ok(true);
                }
            }
        }
    }

    fn emit_extension(&mut self, data: BlockData, span: Span, output: &mut Output<'_>) {
        let id = self.id();
        output.done(
            Block {
                id,
                span,
                status: BlockStatus::Closed,
                data,
            },
            false,
            self.parent(),
        );
    }

    fn apply_inline_plugins(
        &mut self,
        source: &str,
        nodes: Vec<crate::Inline>,
    ) -> Result<Vec<crate::Inline>, ParseError> {
        use crate::{Inline, InlineKind, Text};

        let mut output = Vec::with_capacity(nodes.len());
        for node in nodes {
            let InlineKind::Text(Text::Source(span)) = node.kind else {
                output.push(node);
                continue;
            };
            if node.span != span {
                output.push(node);
                continue;
            }
            let mut retained = span.start;
            let mut position = retained;
            while position < span.end {
                let byte = source.as_bytes()[position];
                let mut matched = None;
                for entry in &mut self.plugins {
                    if entry.rule.scope != Scope::Inline || entry.rule.trigger != byte {
                        continue;
                    }
                    let mut consumed_total = 0;
                    let mut phase = Phase::Probe;
                    loop {
                        let result =
                            plugin_step(entry, source, position + consumed_total, span.end, phase)?;
                        match result {
                            RuleResult::Accept { consumed } => {
                                matched = Some((entry.rule.kind, consumed_total + consumed, None));
                                break;
                            }
                            RuleResult::AcceptWithFields { consumed, fields } => {
                                matched = Some((
                                    entry.rule.kind,
                                    consumed_total + consumed,
                                    Some(fields),
                                ));
                                break;
                            }
                            RuleResult::Reject => break,
                            RuleResult::NeedMore { consumed } if phase != Phase::Finish => {
                                consumed_total += consumed;
                                phase = if position + consumed_total == span.end || consumed == 0 {
                                    Phase::Finish
                                } else {
                                    Phase::Advance
                                };
                            }
                            RuleResult::NeedMore { .. } => break,
                        }
                    }
                    if matched.is_some() {
                        break;
                    }
                }

                if let Some((kind, consumed, fields)) = matched.filter(|(_, width, _)| *width != 0)
                {
                    if retained != position {
                        let span = Span::new(retained, position);
                        output.push(Inline {
                            span,
                            kind: InlineKind::Text(Text::Source(span)),
                        });
                    }
                    let span = Span::new(position, position + consumed);
                    let kind = match fields {
                        Some(fields) if !fields.is_empty() => {
                            crate::plugin::validate_fields(&fields, &source[Range::from(span)])?;
                            InlineKind::ExtensionWithFields { kind, fields }
                        }
                        _ => InlineKind::Extension(kind),
                    };
                    output.push(Inline { span, kind });
                    position = span.end;
                    retained = position;
                } else {
                    position += source[position..span.end]
                        .chars()
                        .next()
                        .unwrap()
                        .len_utf8();
                }
            }
            if retained != span.end {
                let span = Span::new(retained, span.end);
                output.push(Inline {
                    span,
                    kind: InlineKind::Text(Text::Source(span)),
                });
            }
        }
        Ok(output)
    }
}

fn plugin_step(
    entry: &mut Registered,
    source: &str,
    start: usize,
    end: usize,
    phase: Phase,
) -> Result<RuleResult, PluginError> {
    let start = start.min(end);
    let cap = entry.implementation.max_input_bytes();
    let mut stop = end.min(start.saturating_add(cap));
    while !source.is_char_boundary(stop) {
        stop -= 1;
    }
    if stop == start && start < end {
        return Err(PluginError::new(
            "plugin input window cannot hold one UTF-8 character",
        ));
    }
    let text = &source[start..stop];
    let result = entry.implementation.step(RuleInput { phase, text })?;
    let consumed = match result {
        RuleResult::Accept { consumed }
        | RuleResult::AcceptWithFields { consumed, .. }
        | RuleResult::NeedMore { consumed } => consumed,
        RuleResult::Reject => return Ok(result),
    };
    if consumed > text.len() || !text.is_char_boundary(consumed) {
        return Err(PluginError::new(
            "plugin returned an invalid source boundary",
        ));
    }
    if consumed == 0
        && phase == Phase::Probe
        && matches!(
            result,
            RuleResult::Accept { .. } | RuleResult::AcceptWithFields { .. }
        )
    {
        return Err(PluginError::new("plugin accepted without consuming input"));
    }
    if consumed == 0 && stop != end && matches!(result, RuleResult::NeedMore { .. }) {
        return Err(PluginError::new(
            "plugin requested input beyond its bounded window",
        ));
    }
    Ok(result)
}

fn emit_append(output: &mut Output<'_>, id: BlockId, published: &mut usize, end: usize) {
    if end > *published {
        output.append_text(id, Span::new(*published, end));
        *published = end;
    }
}

fn task_marker(line: &str) -> (Option<bool>, usize) {
    let trimmed = line.trim_start_matches([' ', '\t']);
    let checked = match trimmed.as_bytes() {
        [b'[', b' ' | b'x' | b'X', b']', b' ' | b'\t', ..] => trimmed.as_bytes()[1] != b' ',
        _ => return (None, 0),
    };
    (Some(checked), line.len() - trimmed.len() + 4)
}

fn blank(line: &str) -> bool {
    line.bytes().all(|byte| matches!(byte, b' ' | b'\t'))
}

fn container_block(frame: Frame, mut status: BlockStatus) -> Block {
    let data = match frame.kind {
        Kind::Quote => BlockData::Quote,
        Kind::Vmr {
            header_end,
            closed,
            preview_closed,
        } => {
            if status == BlockStatus::Closed && !(closed || preview_closed) {
                status = BlockStatus::Incomplete;
            }
            BlockData::Vmr(vmr::Vmr {
                header: Span::new(frame.start, header_end),
            })
        }
        Kind::List {
            style,
            ordinal,
            tight,
        } => BlockData::List {
            ordered: matches!(style, Style::Ordered(_)),
            start: ordinal,
            tight,
        },
        Kind::Item { task, .. } => BlockData::ListItem { task },
    };
    Block {
        id: frame.id,
        span: Span::new(frame.start, frame.end),
        status,
        data,
    }
}

fn unindent(line: &str) -> &str {
    let indent = line
        .as_bytes()
        .iter()
        .take(4)
        .take_while(|&&byte| byte == b' ')
        .count();
    if indent == 4 { line } else { &line[indent..] }
}

fn trim_span(source: &str, span: Span) -> Span {
    let bytes = &source.as_bytes()[span.start..span.end];
    let content = |byte: &u8| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r');
    let Some(first) = bytes.iter().position(content) else {
        return Span::new(span.end, span.end);
    };
    let last = bytes.iter().rposition(content).unwrap();
    Span::new(span.start + first, span.start + last + 1)
}

fn partial_inline_start(head: &str) -> bool {
    let bytes = head.as_bytes();
    match bytes.first() {
        Some(&marker @ (b'*' | b'_' | b'`')) => {
            let limit = if marker == b'`' { 2 } else { 3 };
            let width = bytes
                .iter()
                .take(limit)
                .take_while(|&&byte| byte == marker)
                .count();
            bytes
                .get(width)
                .is_some_and(|byte| *byte != marker && !matches!(byte, b' ' | b'\t'))
        }
        Some(b'[') if bytes.len() <= 1024 => head.contains("]("),
        _ => false,
    }
}

fn ordinary_start(byte: u8) -> bool {
    !matches!(
        byte,
        b' ' | b'\t'
            | b'#'
            | b'>'
            | b'-'
            | b'+'
            | b'*'
            | b'_'
            | b'`'
            | b'~'
            | b'='
            | b'['
            | b'<'
            | b'|'
    ) && !byte.is_ascii_digit()
}

fn atx(text: &str, start: usize) -> Option<(u8, Span)> {
    let count = text
        .bytes()
        .take(7)
        .take_while(|&byte| byte == b'#')
        .count();
    if !(1..=6).contains(&count) {
        return None;
    }
    let remainder = &text[count..];
    if !remainder.is_empty() && !remainder.starts_with([' ', '\t']) {
        return None;
    }
    let trimmed = remainder.trim_end_matches([' ', '\t']);
    let without_hashes = trimmed.trim_end_matches('#');
    let content = if without_hashes.len() != trimmed.len() && without_hashes.ends_with([' ', '\t'])
    {
        without_hashes.trim_end_matches([' ', '\t'])
    } else {
        trimmed
    };
    let left = content.trim_start_matches([' ', '\t']);
    let begin = start + count + content.len() - left.len();
    Some((count as u8, Span::new(begin, begin + left.len())))
}

fn setext(text: &str) -> Option<u8> {
    let text = text.trim_end_matches([' ', '\t']);
    let first = text.as_bytes().first().copied()?;
    match first {
        b'=' | b'-' if text.bytes().all(|byte| byte == first) => {
            Some(if first == b'=' { 1 } else { 2 })
        }
        _ => None,
    }
}

fn thematic(text: &str) -> bool {
    let Some(first @ (b'-' | b'_' | b'*')) = text.as_bytes().first().copied() else {
        return false;
    };
    let mut count = 0;
    for byte in text.bytes() {
        if byte == first {
            count += 1;
        } else if !matches!(byte, b' ' | b'\t') {
            return false;
        }
    }
    count >= 3
}

fn opening_fence(text: &str, start: usize) -> Option<(u8, usize, Option<Span>)> {
    let marker @ (b'`' | b'~') = text.as_bytes().first().copied()? else {
        return None;
    };
    let count = text.bytes().take_while(|&byte| byte == marker).count();
    if count < 3 {
        return None;
    }
    let remainder = &text[count..];
    if marker == b'`' && remainder.contains('`') {
        return None;
    }
    let info = remainder.trim_matches([' ', '\t']);
    let info_start =
        start + count + remainder.len() - remainder.trim_start_matches([' ', '\t']).len();
    Some((
        marker,
        count,
        (!info.is_empty()).then(|| Span::new(info_start, info_start + info.len())),
    ))
}

fn closing_fence(text: &str, marker: u8, width: usize) -> bool {
    if text.as_bytes().first() != Some(&marker) {
        return false;
    }
    let count = text.bytes().take_while(|&byte| byte == marker).count();
    count >= width && blank(&text[count..])
}

fn batch_fence_close(source: &str, start: usize, marker: u8, width: usize) -> Option<Line> {
    let bytes = source.as_bytes();
    let mut skip = start;
    for relative in memchr::memchr_iter(marker, &bytes[start..]) {
        let offset = start + relative;
        if offset < skip {
            continue;
        }
        let mut left = offset;
        while left > start && offset - left < 3 && bytes[left - 1] == b' ' {
            left -= 1;
        }
        if left != start && !matches!(bytes[left - 1], b'\r' | b'\n') {
            continue;
        }
        let end = memchr::memchr2(b'\r', b'\n', &bytes[offset..])
            .map_or(bytes.len(), |length| offset + length);
        if closing_fence(&source[offset..end], marker, width) {
            let next = if end == bytes.len() {
                end
            } else {
                end + 1 + usize::from(bytes[end] == b'\r' && bytes.get(end + 1) == Some(&b'\n'))
            };
            return Some(Line::new(left, end, next));
        }
        skip = end;
    }
    None
}

fn interrupts_paragraph(text: &str) -> bool {
    match text.as_bytes().first() {
        Some(b'#') => atx(text, 0).is_some(),
        Some(b'`' | b'~') => opening_fence(text, 0).is_some(),
        Some(b'-' | b'_' | b'*') => thematic(text),
        Some(b'<') => html_start(text, false).is_some(),
        _ => false,
    }
}

fn html_start(line: &str, allow_type_seven: bool) -> Option<HtmlEnd> {
    let rest = line.strip_prefix('<')?;
    for (prefix, end) in [("!--", "-->"), ("?", "?>"), ("![CDATA[", "]]>")] {
        if rest.starts_with(prefix) {
            return Some(HtmlEnd::Literal(end));
        }
    }
    if rest
        .strip_prefix('!')
        .and_then(|tail| tail.as_bytes().first())
        .is_some_and(u8::is_ascii_alphabetic)
    {
        return Some(HtmlEnd::Literal(">"));
    }

    let name_input = rest.strip_prefix('/').unwrap_or(rest);
    let name_len = name_input
        .bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        .count();
    if name_len == 0 || !name_input.as_bytes()[0].is_ascii_alphabetic() {
        return None;
    }
    let (name, tail) = name_input.split_at(name_len);
    let boundary = tail.is_empty() || tail.starts_with([' ', '\t', '>']) || tail.starts_with("/>");
    if !boundary {
        return None;
    }
    if !rest.starts_with('/') {
        for (special, end) in [
            ("script", "</script>"),
            ("pre", "</pre>"),
            ("style", "</style>"),
            ("textarea", "</textarea>"),
        ] {
            if name.eq_ignore_ascii_case(special) {
                return Some(HtmlEnd::AsciiCase(end));
            }
        }
    }
    const BLOCK_TAGS: &[&str] = &[
        "address",
        "article",
        "aside",
        "base",
        "basefont",
        "blockquote",
        "body",
        "caption",
        "center",
        "col",
        "colgroup",
        "dd",
        "details",
        "dialog",
        "dir",
        "div",
        "dl",
        "dt",
        "fieldset",
        "figcaption",
        "figure",
        "footer",
        "form",
        "frame",
        "frameset",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "head",
        "header",
        "hr",
        "html",
        "iframe",
        "legend",
        "li",
        "link",
        "main",
        "menu",
        "menuitem",
        "nav",
        "noframes",
        "ol",
        "optgroup",
        "option",
        "p",
        "param",
        "search",
        "section",
        "summary",
        "table",
        "tbody",
        "td",
        "tfoot",
        "th",
        "thead",
        "title",
        "tr",
        "track",
        "ul",
    ];
    if BLOCK_TAGS.iter().any(|tag| name.eq_ignore_ascii_case(tag)) {
        return Some(HtmlEnd::Blank);
    }
    if allow_type_seven
        && (rest.starts_with('/') && tail.trim_matches([' ', '\t']) == ">"
            || tag::opening(line, 0, line.len()).is_some_and(|opening| blank(&line[opening.end..])))
    {
        return Some(HtmlEnd::Blank);
    }
    None
}

fn reference_prefix(source: &str, start: usize, end: usize) -> Option<(&str, Reference, usize)> {
    let bytes = source.as_bytes();
    let start = skip_horizontal(bytes, start, end);
    if bytes.get(start) != Some(&b'[') || start == end {
        return None;
    }
    let label_start = start + 1;
    let mut stop = label_start;
    while stop < end {
        match bytes[stop] {
            b']' => break,
            b'[' => return None,
            b'\\' if stop + 1 < end && bytes[stop + 1].is_ascii_punctuation() => stop += 1,
            _ => {}
        }
        stop += 1;
    }
    if stop + 1 >= end || &bytes[stop..stop + 2] != b"]:" {
        return None;
    }
    let label = &source[label_start..stop];
    if label.trim().is_empty() || label.chars().take(1000).count() > 999 {
        return None;
    }

    let offset = skip_horizontal(bytes, stop + 2, end);
    let offset =
        line_break(bytes, offset, end).map_or(offset, |next| skip_horizontal(bytes, next, end));
    let (destination, tail) = inline::link_destination(source, offset, end)?;
    if tail == offset {
        return None;
    }
    let after_destination = skip_horizontal(bytes, tail, end);
    let following_line = line_break(bytes, after_destination, end);
    let title_start =
        following_line.map_or(after_destination, |next| skip_horizontal(bytes, next, end));
    if title_start > tail
        && let Some((title, stop)) = inline::link_title(source, title_start, end)
    {
        let stop = skip_horizontal(bytes, stop, end);
        if let Some(next) = line_break(bytes, stop, end) {
            return Some((
                label,
                Reference {
                    destination,
                    title: Some(title),
                },
                next,
            ));
        }
        if stop == end {
            return Some((
                label,
                Reference {
                    destination,
                    title: Some(title),
                },
                stop,
            ));
        }
    }
    if let Some(next) = following_line {
        return Some((
            label,
            Reference {
                destination,
                title: None,
            },
            next,
        ));
    }
    (after_destination == end).then_some((
        label,
        Reference {
            destination,
            title: None,
        },
        end,
    ))
}

fn skip_horizontal(bytes: &[u8], mut start: usize, end: usize) -> usize {
    while start < end && matches!(bytes[start], b' ' | b'\t') {
        start += 1;
    }
    start
}

fn line_break(bytes: &[u8], start: usize, end: usize) -> Option<usize> {
    if start == end {
        return None;
    }
    match bytes[start] {
        b'\n' => Some(start + 1),
        b'\r' => Some(start + 1 + usize::from(start + 1 < end && bytes[start + 1] == b'\n')),
        _ => None,
    }
}

fn cell_spans(source: &str, span: Span) -> impl Iterator<Item = Span> + Clone + '_ {
    let span = trim_span(source, span);
    let bytes = source.as_bytes();
    let base =
        span.start + usize::from(bytes.get(span.start) == Some(&b'|') && span.start < span.end);
    let mut pipes = memchr::memchr_iter(b'|', &bytes[base..span.end]);
    let mut start = base;
    let mut emitted = false;
    let mut done = false;
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        for relative in pipes.by_ref() {
            let position = base + relative;
            let escapes = bytes[start..position]
                .iter()
                .rev()
                .take_while(|&&byte| byte == b'\\')
                .count();
            if escapes % 2 == 0 {
                let cell = trim_span(source, Span::new(start, position));
                start = position + 1;
                emitted = true;
                return Some(cell);
            }
        }
        done = true;
        (start < span.end || !emitted).then(|| trim_span(source, Span::new(start, span.end)))
    })
}

fn separator_alignments(source: &str, span: Span) -> Option<Vec<Alignment>> {
    cell_spans(source, span)
        .map(|span| {
            let marker = &source[Range::from(span)];
            let starts = marker.starts_with(':');
            let ends = marker.ends_with(':');
            let trimmed = marker.strip_prefix(':').unwrap_or(marker);
            let trimmed = trimmed.strip_suffix(':').unwrap_or(trimmed);
            if trimmed.is_empty() || !trimmed.bytes().all(|byte| byte == b'-') {
                return None;
            }
            Some(match (starts, ends) {
                (true, true) => Alignment::Center,
                (true, false) => Alignment::Left,
                (false, true) => Alignment::Right,
                (false, false) => Alignment::None,
            })
        })
        .collect()
}
