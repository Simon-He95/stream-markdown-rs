//! Source ranges and incremental changes for Rust Markdown consumers.

mod block;
mod container;
mod custom_inline;
mod inline;
mod math_block;
mod plugin;
mod tag;

use std::{borrow::Cow, collections::HashMap, fmt, num::NonZeroUsize, ops::Range, sync::Arc};

pub use block::admonition::{Admonition, AdmonitionKind};
pub use block::details::HtmlOpening;
pub use block::vmr::{Vmr, VmrParameters};
pub use custom_inline::{CustomElement, InlineElement};
pub use inline::{Inline, InlineKind, Link};
pub use plugin::{
    Phase, PluginError, PluginField, PluginFieldValue, PluginRule, Rule, RuleInput, RuleResult,
    Scope,
};
pub use tag::{Attribute, Attributes, Tag};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

impl From<Span> for Range<usize> {
    fn from(span: Span) -> Self {
        span.start..span.end
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Text {
    Source(Span),
    Decoded(Box<str>),
}

impl Text {
    pub fn as_str<'a>(&'a self, source: &'a str) -> &'a str {
        match self {
            Self::Source(span) => &source[Range::from(*span)],
            Self::Decoded(text) => text,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Syntax {
    CommonMark,
    #[default]
    Gfm,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Preview {
    #[default]
    Off,
    Ai,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Options {
    pub syntax: Syntax,
    pub preview: Preview,
    /// Preview a complete pipe-delimited header before its separator arrives.
    pub preview_table_header: bool,
    pub math: bool,
    pub footnotes: bool,
    /// Enables Markstream syntax and typography extensions.
    pub markstream: bool,
    pub require_closing_strong: bool,
    /// HTML-like element names that produce structured custom nodes.
    pub custom_tags: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BlockId(pub usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockStatus {
    Open,
    Closed,
    Incomplete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InlineContent {
    Plain(Span),
    Parsed(Vec<Inline>),
    // This provisional view keeps amortized appends and leaves the final variants compact.
    #[allow(clippy::box_collection)]
    Fragments(Box<Vec<Span>>),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Alignment {
    #[default]
    None,
    Left,
    Center,
    Right,
}

/// Row-major storage. The first row is the header.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableData {
    pub(crate) alignments: Vec<Alignment>,
    pub(crate) cells: Vec<InlineContent>,
}

impl TableData {
    pub fn alignments(&self) -> &[Alignment] {
        &self.alignments
    }

    pub fn header(&self) -> &[InlineContent] {
        &self.cells[..self.alignments.len()]
    }

    pub fn rows(&self) -> std::slice::ChunksExact<'_, InlineContent> {
        self.cells[self.alignments.len()..].chunks_exact(self.alignments.len())
    }

    /// Applies an emitted row to a materialized table view.
    pub fn add_row(&mut self, cells: Vec<InlineContent>) {
        assert_eq!(cells.len(), self.alignments.len());
        self.cells.extend(cells);
    }

    /// Applies a cell correction. The flat index includes the header.
    pub fn replace_cell(&mut self, index: usize, content: InlineContent) {
        self.cells[index] = content;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawLine {
    pub span: Span,
    pub leading_spaces: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentedCode {
    pub info: Option<Span>,
    pub lines: Vec<RawLine>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockData {
    Paragraph(InlineContent),
    Heading {
        level: u8,
        content: InlineContent,
    },
    CodeBlock {
        /// A zero-width range denotes an absent info string.
        info: Span,
        body: Span,
    },
    SegmentedCode(Box<SegmentedCode>),
    IndentedCode {
        lines: Vec<RawLine>,
    },
    ThematicBreak,
    Quote,
    List {
        ordered: bool,
        start: u64,
        tight: bool,
    },
    ListItem {
        task: Option<bool>,
    },
    Html,
    SegmentedHtml(Vec<RawLine>),
    Table(Box<TableData>),
    Vmr(Vmr),
    Admonition(Admonition),
    Details(HtmlOpening),
    HtmlWrapper(HtmlOpening),
    Summary {
        opening_end: usize,
        content: InlineContent,
    },
    Math {
        body: Text,
    },
    Footnote {
        label_end: usize,
        content: InlineContent,
    },
    CustomTag(Box<CustomElement>),
    Extension {
        kind: u32,
    },
    ExtensionWithFields {
        kind: u32,
        fields: Box<[PluginField]>,
    },
}

impl BlockData {
    pub fn code_info(&self) -> Option<Span> {
        match self {
            Self::CodeBlock { info, .. } if info.start < info.end => Some(*info),
            Self::SegmentedCode(code) => code.info,
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Block {
    pub id: BlockId,
    pub span: Span,
    pub status: BlockStatus,
    pub data: BlockData,
}

impl Block {
    /// Borrow display TeX, retaining surrounding whitespace when it is contiguous.
    pub fn display_math_source<'a>(&'a self, source: &'a str) -> Option<&'a str> {
        let BlockData::Math { body } = &self.data else {
            return None;
        };
        if self.status == BlockStatus::Closed
            && let Text::Source(content) = body
            && let Some(delimiter) = math_block::opening(&source[Range::from(self.span)])
            && let Some(close) = delimiter.closes(source, content.end, self.span.end)
        {
            let start = self.span.start + delimiter.width();
            let before = &source[start..content.start];
            let after = &source[content.end..close.start];
            if before.trim().is_empty() && after.trim().is_empty() {
                return Some(&source[start..close.start]);
            }
        }
        Some(body.as_str(source))
    }

    pub fn summary_opening(&self) -> Option<HtmlOpening> {
        match self.data {
            BlockData::Summary { opening_end, .. } => Some(HtmlOpening {
                span: Span::new(self.span.start, opening_end),
            }),
            _ => None,
        }
    }

    pub fn footnote_label(&self) -> Option<Span> {
        match self.data {
            BlockData::Footnote { label_end, .. } => {
                Some(Span::new(self.span.start + 2, label_end))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreviewHint {
    Table {
        table: Box<TableData>,
        loading: bool,
    },
    TableRow(Vec<InlineContent>),
    HtmlContent(Span),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Change {
    Insert {
        parent: Option<BlockId>,
        block: Block,
    },
    Replace(Block),
    AppendText {
        id: BlockId,
        span: Span,
    },
    AddTextFragment {
        id: BlockId,
        span: Span,
    },
    AddRawLine {
        id: BlockId,
        line: RawLine,
        end: usize,
    },
    AddRow {
        id: BlockId,
        end: usize,
        cells: Vec<InlineContent>,
    },
    ReplaceTableCell {
        id: BlockId,
        index: usize,
        content: InlineContent,
    },
    SetStatus {
        id: BlockId,
        status: BlockStatus,
        end: usize,
    },
    Remove {
        id: BlockId,
    },
    CloseInlineElement {
        id: BlockId,
        start: usize,
        end: usize,
    },
    Preview {
        id: BlockId,
        hint: Option<PreviewHint>,
    },
    /// Temporary inline overlay for a visible paragraph or heading.
    /// An empty vector clears it. Replacement or closure also clears it.
    PreviewInlines {
        id: BlockId,
        nodes: Vec<Inline>,
    },
}

#[derive(Debug)]
pub struct Document<'src> {
    pub source: &'src str,
    pub nodes: Vec<Block>,
    parents: Vec<Option<NonZeroUsize>>,
}

impl Document<'_> {
    pub fn parent(&self, id: BlockId) -> Option<BlockId> {
        self.parents
            .get(id.0)
            .copied()
            .flatten()
            .map(|parent| BlockId(parent.get() - 1))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseError {
    Finished,
    DifferentSource,
    InvalidSourceEnd,
    SourceShrunk,
    Plugin(PluginError),
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Finished => formatter.write_str("the parser session has ended"),
            Self::DifferentSource => {
                formatter.write_str("the parser is bound to a different source buffer")
            }
            Self::InvalidSourceEnd => {
                formatter.write_str("the visible end must be a UTF-8 boundary within the source")
            }
            Self::SourceShrunk => formatter.write_str("the visible source prefix cannot shrink"),
            Self::Plugin(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ParseError {}

impl From<PluginError> for ParseError {
    fn from(error: PluginError) -> Self {
        Self::Plugin(error)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Reference {
    pub(crate) destination: Span,
    pub(crate) title: Option<Span>,
}

#[derive(Default)]
pub(crate) struct References(Option<HashMap<String, Reference>>);

impl References {
    pub(crate) fn get(&self, label: &str) -> Option<&Reference> {
        self.0.as_ref()?.get(label)
    }

    fn remove(&mut self, label: &str) {
        if let Some(references) = &mut self.0 {
            references.remove(label);
        }
    }

    fn insert(&mut self, label: Cow<'_, str>, reference: Reference) -> bool {
        use std::collections::hash_map::Entry;

        let references = self.0.get_or_insert_with(HashMap::new);
        match references.entry(label.into_owned()) {
            Entry::Occupied(_) => false,
            Entry::Vacant(entry) => {
                entry.insert(reference);
                true
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Metrics {
    pub input_bytes: usize,
    pub scanned_bytes: usize,
    pub inline_bytes: usize,
    pub invalidated_blocks: usize,
    /// Bytes submitted to the active paragraph or heading preview probe.
    pub preview_probe_bytes: usize,
    /// Bytes submitted to the active paragraph or heading preview lexer.
    pub preview_inline_bytes: usize,
}

pub struct Parser {
    source: String,
    stream: StreamState,
}

impl Parser {
    pub fn new(options: Options) -> Self {
        Self {
            source: String::new(),
            stream: StreamState::new(options),
        }
    }

    pub fn add_plugin(&mut self, plugin: impl PluginRule + 'static) -> Result<(), ParseError> {
        self.stream.add_plugin(plugin)
    }

    pub fn append(&mut self, chunk: &str, out: &mut Vec<Change>) -> Result<(), ParseError> {
        let source = &mut self.source;
        self.stream.append(out, || {
            source.push_str(chunk);
            source.as_str()
        })
    }

    pub fn finish(&mut self, out: &mut Vec<Change>) -> Result<(), ParseError> {
        self.stream.finish(&self.source, out)
    }

    pub fn source_text(&self) -> &str {
        &self.source
    }

    pub fn source(&self, span: Span) -> &str {
        &self.source[Range::from(span)]
    }

    pub fn into_source(self) -> String {
        self.source
    }

    pub fn metrics(&self) -> Metrics {
        self.stream.metrics()
    }
}

/// An append-only source buffer owned by the caller.
pub struct SourceBuffer {
    text: String,
    identity: Arc<()>,
}

impl SourceBuffer {
    /// Takes ownership of the source allocation.
    pub fn new(text: String) -> Self {
        Self {
            text,
            identity: Arc::new(()),
        }
    }

    pub fn push_str(&mut self, chunk: &str) {
        self.text.push_str(chunk);
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn into_string(self) -> String {
        self.text
    }
}

/// An incremental parser bound to a caller-owned source buffer.
pub struct BorrowedParser {
    identity: Arc<()>,
    stream: StreamState,
}

impl BorrowedParser {
    pub fn new(source: &SourceBuffer, options: Options) -> Self {
        Self {
            identity: source.identity.clone(),
            stream: StreamState::new(options),
        }
    }

    pub fn add_plugin(&mut self, plugin: impl PluginRule + 'static) -> Result<(), ParseError> {
        self.stream.add_plugin(plugin)
    }

    /// Advances to the visible UTF-8 byte end and emits changes with absolute source spans.
    pub fn append(
        &mut self,
        source: &SourceBuffer,
        end: usize,
        out: &mut Vec<Change>,
    ) -> Result<(), ParseError> {
        let prefix = self.prefix(source, end)?;
        self.stream.append(out, || prefix)
    }

    /// Consumes the visible prefix and settles pending blocks.
    pub fn finish(
        &mut self,
        source: &SourceBuffer,
        end: usize,
        out: &mut Vec<Change>,
    ) -> Result<(), ParseError> {
        self.stream.finish(self.prefix(source, end)?, out)
    }

    pub fn metrics(&self) -> Metrics {
        self.stream.metrics()
    }

    fn prefix<'src>(
        &self,
        source: &'src SourceBuffer,
        end: usize,
    ) -> Result<&'src str, ParseError> {
        if !Arc::ptr_eq(&self.identity, &source.identity) {
            return Err(ParseError::DifferentSource);
        }
        source.text.get(..end).ok_or(ParseError::InvalidSourceEnd)
    }
}

struct StreamState {
    engine: Option<block::Engine>,
    finished_metrics: Metrics,
}

impl StreamState {
    fn new(options: Options) -> Self {
        Self {
            engine: Some(block::Engine::new(options, false)),
            finished_metrics: Metrics::default(),
        }
    }

    fn add_plugin(&mut self, plugin: impl PluginRule + 'static) -> Result<(), ParseError> {
        if let Some(engine) = &mut self.engine
            && engine.metrics().input_bytes == 0
        {
            return Ok(engine.add_plugin(Box::new(plugin))?);
        }
        Err(PluginError::new("register plugins before appending source").into())
    }

    fn append<'src>(
        &mut self,
        out: &mut Vec<Change>,
        source: impl FnOnce() -> &'src str,
    ) -> Result<(), ParseError> {
        let Some(engine) = &mut self.engine else {
            return Err(ParseError::Finished);
        };
        let previous_end = engine.metrics().input_bytes;
        let source = source();
        if source.len() < previous_end {
            return Err(ParseError::SourceShrunk);
        }
        if source.len() == previous_end {
            return Ok(());
        }
        let start = out.len();
        if let Err(error) = engine.consume(source, false, block::Output::Stream(out, start)) {
            out.truncate(start);
            self.finished_metrics = engine.metrics();
            self.engine = None;
            return Err(error);
        }
        engine.publish_html_container_progress(previous_end, out);
        Ok(())
    }

    fn finish(&mut self, source: &str, out: &mut Vec<Change>) -> Result<(), ParseError> {
        let Some(engine) = &mut self.engine else {
            return Ok(());
        };
        if source.len() < engine.metrics().input_bytes {
            return Err(ParseError::SourceShrunk);
        }
        let start = out.len();
        let result = engine.consume(source, true, block::Output::Stream(out, start));
        self.finished_metrics = engine.metrics();
        self.engine = None;
        if result.is_err() {
            out.truncate(start);
        }
        result
    }

    fn metrics(&self) -> Metrics {
        self.engine
            .as_ref()
            .map_or(self.finished_metrics, block::Engine::metrics)
    }
}

/// Parses borrowed source and materializes the final nodes once.
pub fn parse<'src>(source: &'src str, options: &Options) -> Result<Document<'src>, ParseError> {
    parse_with_plugins(source, options, Vec::new())
}

/// Lazily parses independent chat messages, yielding visible messages first.
/// The index identifies the message in the input. The visible range is clamped
/// to the input. Pull as many items as the current restore batch can process.
pub fn restore_history<'src, 'work>(
    messages: &'work [&'src str],
    visible: Range<usize>,
    options: &'work Options,
) -> impl Iterator<Item = (usize, Result<Document<'src>, ParseError>)> + use<'src, 'work> {
    let start = visible.start.min(messages.len());
    let end = visible.end.clamp(start, messages.len());
    (start..end)
        .chain(0..start)
        .chain(end..messages.len())
        .map(move |index| (index, parse(messages[index], options)))
}

pub fn parse_with_plugins<'src>(
    source: &'src str,
    options: &Options,
    plugins: Vec<Box<dyn PluginRule>>,
) -> Result<Document<'src>, ParseError> {
    let mut engine = block::Engine::new(options.clone(), true);
    for plugin in plugins {
        engine.add_plugin(plugin)?;
    }
    let mut nodes = Vec::new();
    let mut parents = Vec::new();
    engine.consume(source, true, block::Output::Batch(&mut nodes, &mut parents))?;
    engine.resolve_batch(source, &mut nodes)?;
    Ok(Document {
        source,
        nodes,
        parents,
    })
}
