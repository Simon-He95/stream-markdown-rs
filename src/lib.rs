//! Source ranges and incremental changes for Rust Markdown consumers.

mod block;
mod container;
mod inline;
mod plugin;
mod tag;

use std::{borrow::Cow, collections::HashMap, fmt, ops::Range};

pub use inline::{Inline, InlineKind, Link};
pub use plugin::{Phase, PluginError, PluginRule, Rule, RuleInput, RuleResult, Scope};
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
    Fragments(Vec<Span>),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Alignment {
    #[default]
    None,
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawLine {
    pub span: Span,
    pub leading_spaces: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockData {
    Paragraph(InlineContent),
    Heading {
        level: u8,
        content: InlineContent,
    },
    CodeBlock {
        info: Option<Span>,
        body: Span,
    },
    SegmentedCode {
        info: Option<Span>,
        lines: Vec<RawLine>,
    },
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
    Table {
        alignments: Vec<Alignment>,
        header: Vec<InlineContent>,
        rows: Vec<Vec<InlineContent>>,
    },
    CustomTag {
        tag: Tag,
        body: Span,
    },
    Extension {
        kind: u32,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Block {
    pub id: BlockId,
    pub span: Span,
    pub status: BlockStatus,
    pub data: BlockData,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewHint {
    Table,
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
    SetStatus {
        id: BlockId,
        status: BlockStatus,
        end: usize,
    },
    Remove {
        id: BlockId,
    },
    Preview {
        id: BlockId,
        hint: Option<PreviewHint>,
    },
}

#[derive(Debug)]
pub struct Document<'src> {
    pub source: &'src str,
    pub nodes: Vec<Block>,
    /// Parent relationships indexed by BlockId. Empty for a flat document.
    pub parents: Vec<Option<BlockId>>,
}

impl Document<'_> {
    pub fn parent(&self, id: BlockId) -> Option<BlockId> {
        self.parents.get(id.0).copied().flatten()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseError {
    Finished,
    Plugin(PluginError),
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Finished => formatter.write_str("the parser session has ended"),
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
pub(crate) struct References(HashMap<String, Reference>);

impl References {
    pub(crate) fn get(&self, label: &str) -> Option<&Reference> {
        self.0.get(label)
    }

    fn insert(&mut self, label: Cow<'_, str>, reference: Reference) -> bool {
        use std::collections::hash_map::Entry;

        match self.0.entry(label.into_owned()) {
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
}

pub struct Parser {
    source: String,
    engine: block::Engine,
    finished: bool,
}

impl Parser {
    pub fn new(options: Options) -> Self {
        Self {
            source: String::new(),
            engine: block::Engine::new(options, false),
            finished: false,
        }
    }

    pub fn add_plugin(&mut self, plugin: impl PluginRule + 'static) -> Result<(), ParseError> {
        if !self.source.is_empty() || self.finished {
            return Err(PluginError::new("register plugins before appending source").into());
        }
        self.engine.add_plugin(Box::new(plugin))?;
        Ok(())
    }

    pub fn append(&mut self, chunk: &str, out: &mut Vec<Change>) -> Result<(), ParseError> {
        if self.finished {
            return Err(ParseError::Finished);
        }
        if chunk.is_empty() {
            return Ok(());
        }
        let start = out.len();
        self.source.push_str(chunk);
        if let Err(error) =
            self.engine
                .consume(&self.source, false, block::Output::Stream(out, start))
        {
            out.truncate(start);
            self.finished = true;
            return Err(error);
        }
        Ok(())
    }

    pub fn finish(&mut self, out: &mut Vec<Change>) -> Result<(), ParseError> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        let start = out.len();
        if let Err(error) =
            self.engine
                .consume(&self.source, true, block::Output::Stream(out, start))
        {
            out.truncate(start);
            return Err(error);
        }
        Ok(())
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
        self.engine.metrics()
    }
}

/// Parses borrowed source and materializes the final nodes once.
pub fn parse<'src>(source: &'src str, options: &Options) -> Result<Document<'src>, ParseError> {
    parse_with_plugins(source, options, Vec::new())
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
