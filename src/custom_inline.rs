//! Typed source ranges for registered inline tags.

use crate::{BlockStatus, Inline, InlineKind, Options, References, Span, Tag, inline, tag};
use std::ops::Range;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InlineElement {
    pub tag: Tag,
    pub body: Span,
    pub status: BlockStatus,
    pub children: Vec<Inline>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CustomElement {
    pub tag: Tag,
    pub body: Span,
    pub children: Vec<Inline>,
    pub promoted: bool,
}

struct OpenElement {
    start: usize,
    tag: Tag,
    content_start: usize,
    children: Vec<Inline>,
}

impl OpenElement {
    fn into_inline(self, body_end: usize, end: usize, status: BlockStatus) -> Inline {
        Inline {
            span: Span::new(self.start, end),
            kind: InlineKind::CustomTag(Box::new(InlineElement {
                tag: self.tag,
                body: Span::new(self.content_start, body_end),
                status,
                children: self.children,
            })),
        }
    }
}

pub(crate) fn group(
    source: &str,
    mut nodes: Vec<Inline>,
    end: usize,
    options: &Options,
    preview: bool,
) -> Vec<Inline> {
    let mut contains_html = false;
    for node in &mut nodes {
        let children = match &mut node.kind {
            InlineKind::Html => {
                contains_html = true;
                continue;
            }
            InlineKind::Emphasis(children)
            | InlineKind::Strong(children)
            | InlineKind::Strikethrough(children)
            | InlineKind::Highlight(children)
            | InlineKind::Insert(children) => children,
            InlineKind::Link(link) | InlineKind::LinkPreview(link) => &mut link.children,
            _ => continue,
        };
        if let Some(last) = children.last() {
            let end = last.span.end;
            *children = group(source, std::mem::take(children), end, options, preview);
        }
    }
    if !contains_html {
        return nodes;
    }

    let mut output = Vec::with_capacity(nodes.len());
    let mut opened = Vec::<OpenElement>::new();
    for mut node in nodes {
        if node.kind == InlineKind::Html {
            let raw = &source[Range::from(node.span)];
            if raw.starts_with("</") {
                if let Some(element) = opened.pop_if(|last| {
                    raw.strip_prefix("</")
                        .and_then(|rest| rest.strip_prefix(last.tag.tag_name(source)))
                        .is_some_and(|tail| tail.trim_start_matches([' ', '\t', '\r', '\n']) == ">")
                }) {
                    let child =
                        element.into_inline(node.span.start, node.span.end, BlockStatus::Closed);
                    opened
                        .last_mut()
                        .map_or(&mut output, |last| &mut last.children)
                        .push(child);
                    continue;
                }
            } else if let Some(opening) = tag::opening(source, node.span.start, node.span.end) {
                let name = opening.tag.tag_name(source);
                if options
                    .custom_tags
                    .iter()
                    .any(|registered| registered == name)
                {
                    let element = OpenElement {
                        start: node.span.start,
                        tag: opening.tag,
                        content_start: opening.end,
                        children: Vec::new(),
                    };
                    if opening.self_closing {
                        let child =
                            element.into_inline(opening.end, node.span.end, BlockStatus::Closed);
                        opened
                            .last_mut()
                            .map_or(&mut output, |last| &mut last.children)
                            .push(child);
                    } else {
                        opened.push(element);
                    }
                    continue;
                }
                if options.markstream && name.eq_ignore_ascii_case("br") {
                    node.kind = InlineKind::HardBreak;
                }
            }
        }
        opened
            .last_mut()
            .map_or(&mut output, |last| &mut last.children)
            .push(node);
    }
    while let Some(element) = opened.pop() {
        let status = if preview {
            BlockStatus::Open
        } else {
            BlockStatus::Incomplete
        };
        let child = element.into_inline(end, end, status);
        opened
            .last_mut()
            .map_or(&mut output, |last| &mut last.children)
            .push(child);
    }
    output
}

pub(crate) fn after_hardbreak(
    source: &str,
    line: Span,
    options: &Options,
    refs: &References,
) -> Option<usize> {
    let raw = &source.as_bytes()[line.start..line.end];
    if !raw.windows(3).any(|part| part.eq_ignore_ascii_case(b"<br")) {
        return None;
    }
    let nodes = group(
        source,
        inline::parse_preview(source, line, options, refs),
        line.end,
        options,
        true,
    );
    let mut after_break = false;
    for node in nodes {
        match node.kind {
            InlineKind::HardBreak => after_break = true,
            InlineKind::CustomTag(element)
                if after_break && element.body.start < element.body.end =>
            {
                return Some(node.span.start);
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn tag_boundary(
    source: &str,
    begin: usize,
    end: usize,
    tags: &[String],
) -> Option<usize> {
    let raw = source.get(begin..end)?.strip_prefix('<')?;
    if let Some(closing) = raw.strip_prefix('/') {
        for name in tags {
            if let Some(tail) = closing.strip_prefix(name)
                && let Some(rest) = tail
                    .trim_start_matches([' ', '\t', '\r', '\n'])
                    .strip_prefix('>')
            {
                return Some(end - rest.len());
            }
        }
        return None;
    }
    if tags.iter().any(|name| {
        raw.strip_prefix(name).is_some_and(|tail| {
            tail.as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_whitespace() || matches!(byte, b'>' | b'/'))
        })
    }) {
        return tag::opening(source, begin, end).map(|opening| opening.end);
    }
    None
}
