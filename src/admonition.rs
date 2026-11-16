use super::{Engine, Line, Output, container};
use crate::Span;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmonitionKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
    Info,
    Danger,
    Error,
    Admonition,
}

impl AdmonitionKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Tip => "tip",
            Self::Important => "important",
            Self::Warning => "warning",
            Self::Caution => "caution",
            Self::Info => "info",
            Self::Danger => "danger",
            Self::Error => "error",
            Self::Admonition => "admonition",
        }
    }

    fn parse(name: &str, quoted: bool) -> Option<Self> {
        if quoted {
            [
                Self::Note,
                Self::Tip,
                Self::Important,
                Self::Warning,
                Self::Caution,
            ]
            .into_iter()
            .find(|kind| name.eq_ignore_ascii_case(kind.as_str()))
        } else {
            [
                Self::Note,
                Self::Tip,
                Self::Warning,
                Self::Caution,
                Self::Info,
                Self::Danger,
                Self::Error,
                Self::Admonition,
            ]
            .into_iter()
            .find(|kind| name == kind.as_str())
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Admonition {
    pub header: Span,
    pub kind: AdmonitionKind,
    pub quoted: bool,
}

impl Admonition {
    pub fn title(self, source: &str) -> Option<Span> {
        let header = source.get(Range::from(self.header))?.trim_start();
        let tail = if self.quoted {
            let indicator = header.strip_prefix('>')?.trim_start().strip_prefix("[!")?;
            indicator.split_once(']')?.1
        } else {
            let marker = header.trim_start_matches(':').trim_start();
            &marker[marker.find(char::is_whitespace)?..]
        };
        let start = self.header.end - tail.trim_start().len();
        let end = self.header.end - tail.len() + tail.trim_end().len();
        (start < end).then_some(Span::new(start, end))
    }
}

pub(super) fn colon_kind(source: &str, header: Span) -> Option<AdmonitionKind> {
    let text = source[Range::from(header)].trim_start_matches(':');
    let kind = text.split_whitespace().next()?;
    AdmonitionKind::parse(kind, false)
}

impl container::Kind {
    pub(crate) fn is_colon_container(self) -> bool {
        matches!(
            self,
            Self::Vmr { .. } | Self::Admonition { quoted: false, .. }
        )
    }

    pub(crate) fn is_quote_container(self) -> bool {
        matches!(self, Self::Quote | Self::Admonition { quoted: true, .. })
    }
}

impl Engine {
    /// A GitHub-style marker consumes the first line of a newly opened quote.
    pub(super) fn open_quoted_admonition(
        &mut self,
        source: &str,
        line: Line,
        prefix: container::Prefix,
        output: &mut Output<'_>,
    ) -> bool {
        let start = line.start + prefix.bytes;
        let indent = container::whitespace(
            &source.as_bytes()[start..line.end],
            line.column + prefix.columns,
            prefix.padding,
            4,
        );
        if indent.columns >= 4 {
            return false;
        }
        let remaining = &source[start + indent.bytes..line.end];
        let Some((name, tail)) = remaining
            .strip_prefix("[!")
            .and_then(|rest| rest.split_once(']'))
        else {
            return false;
        };
        if !(tail.is_empty() || tail.starts_with([' ', '\t'])) {
            return false;
        }
        let Some(kind) = AdmonitionKind::parse(name, true) else {
            return false;
        };
        self.open_container(
            container::Kind::Admonition {
                header_end: line.end,
                kind,
                quoted: true,
                closed: false,
                preview_closed: false,
            },
            line.start,
            line.next,
            output,
        );
        self.extend_containers(line.next);
        true
    }
}
