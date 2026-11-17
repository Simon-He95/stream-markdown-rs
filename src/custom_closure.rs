use super::{Dependent, Engine, Line, Output, container, custom_inline};
use crate::{BlockId, BlockStatus, Change, Inline, InlineContent, InlineKind, ParseError};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct State {
    pending: Option<Candidate>,
    resolved: HashMap<BlockId, Closure>,
}

enum Candidate {
    Known(Target),
    Deferred(Dependent),
}

impl Candidate {
    fn leaf_end(&self) -> usize {
        match self {
            Self::Known(target) => target.leaf_end,
            Self::Deferred(leaf) => leaf.span.end,
        }
    }
}

#[derive(Clone, Copy)]
struct Target {
    id: BlockId,
    leaf_end: usize,
    opening: usize,
    name_end: usize,
}

#[derive(Clone, Copy)]
struct Closure {
    start: usize,
    end: usize,
}

impl InlineContent {
    /// Closes a materialized custom inline element at its original opener, without changing its body.
    pub fn close_inline_element(&mut self, start: usize, end: usize) -> bool {
        let Self::Parsed(nodes) = self else {
            return false;
        };
        let Some(mut node) = nodes.last_mut() else {
            return false;
        };
        loop {
            if let InlineKind::CustomTag(element) = &mut node.kind
                && node.span.start == start
            {
                if node.span.end > end {
                    return false;
                }
                node.span.end = end;
                element.status = BlockStatus::Closed;
                return true;
            }
            let Some(child) =
                tail_children(&mut node.kind).and_then(|children| children.last_mut())
            else {
                return false;
            };
            node = child;
        }
    }
}

fn tail_children(kind: &mut InlineKind) -> Option<&mut Vec<Inline>> {
    match kind {
        InlineKind::CustomTag(element) => Some(&mut element.children),
        InlineKind::Emphasis(nodes)
        | InlineKind::Strong(nodes)
        | InlineKind::Strikethrough(nodes)
        | InlineKind::Highlight(nodes)
        | InlineKind::Insert(nodes) => Some(nodes),
        _ => None,
    }
}

fn target(source: &str, leaf: &Dependent, content: &InlineContent) -> Option<Target> {
    let InlineContent::Parsed(nodes) = content else {
        return None;
    };
    let mut node = nodes.last()?;
    let mut result = None;
    loop {
        if !whitespace(source.as_bytes().get(node.span.end..leaf.span.end)?) {
            return result;
        }
        let children = match &node.kind {
            InlineKind::CustomTag(element) if element.status == BlockStatus::Incomplete => {
                result = Some(Target {
                    id: leaf.id,
                    leaf_end: leaf.span.end,
                    opening: node.span.start,
                    name_end: node.span.start + 1 + element.tag.tag_name(source).len(),
                });
                &element.children
            }
            InlineKind::Emphasis(nodes)
            | InlineKind::Strong(nodes)
            | InlineKind::Strikethrough(nodes)
            | InlineKind::Highlight(nodes)
            | InlineKind::Insert(nodes) => nodes,
            _ => return result,
        };
        let Some(child) = children.last() else {
            return result;
        };
        node = child;
    }
}

fn whitespace(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
}

impl Engine {
    pub(super) fn remember_custom_closure(
        &mut self,
        source: &str,
        leaf: &Dependent,
        content: &mut InlineContent,
    ) {
        if self.options.custom_tags.is_empty() {
            return;
        }
        if self.apply_custom_closure(leaf.id, content) {
            return;
        }
        if self
            .custom_closure
            .as_ref()
            .and_then(|state| state.pending.as_ref())
            .is_some_and(|old| old.leaf_end() > leaf.span.end)
        {
            return;
        }
        let candidate = if self.batch {
            memchr::memchr(
                b'<',
                &source.as_bytes()[leaf.content.start..leaf.content.end],
            )
            .map(|_| Candidate::Deferred(leaf.clone()))
        } else {
            target(source, leaf, content).map(Candidate::Known)
        };
        if let Some(candidate) = candidate {
            self.custom_closure
                .get_or_insert_with(Box::<State>::default)
                .pending = Some(candidate);
        } else if let Some(state) = &mut self.custom_closure {
            state.pending = None;
        }
    }

    pub(super) fn apply_custom_closure(&self, id: BlockId, content: &mut InlineContent) -> bool {
        let Some(closure) = self
            .custom_closure
            .as_ref()
            .and_then(|state| state.resolved.get(&id))
        else {
            return false;
        };
        content.close_inline_element(closure.start, closure.end)
    }

    pub(super) fn close_inline_tail_line(
        &mut self,
        source: &str,
        line: Line,
        output: &mut Output<'_>,
    ) -> Result<bool, ParseError> {
        if self.active.is_some() || self.running.is_some() || !self.containers.is_empty() {
            return Ok(false);
        }
        let Some(end) = self.consume_inline_closer(source, line, output)? else {
            return Ok(false);
        };
        if !whitespace(&source.as_bytes()[end..line.end]) {
            self.line(source, Line { start: end, ..line }, output)?;
        }
        Ok(true)
    }

    pub(super) fn partial_inline_closer(
        &mut self,
        source: &str,
        output: &mut Output<'_>,
    ) -> Result<bool, ParseError> {
        if self.active.is_some()
            || self.running.is_some()
            || self.line_start >= self.scan
            || (!self.containers.is_empty()
                && (source.as_bytes()[self.line_start] != b'<'
                    || self.containers.iter().any(|frame| {
                        !matches!(
                            frame.kind,
                            container::Kind::List { .. }
                                | container::Kind::Item { .. }
                                | container::Kind::Quote
                        )
                    })))
        {
            return Ok(false);
        }
        let line = Line::new(self.line_start, self.scan, self.scan);
        let Some(end) = self.consume_inline_closer(source, line, output)? else {
            return Ok(false);
        };
        self.close_containers(0, output);
        self.line_start = end;
        Ok(end == self.scan)
    }

    fn consume_inline_closer(
        &mut self,
        source: &str,
        line: Line,
        output: &mut Output<'_>,
    ) -> Result<Option<usize>, ParseError> {
        let Some(pending) = self
            .custom_closure
            .as_ref()
            .and_then(|state| state.pending.as_ref())
        else {
            return Ok(None);
        };
        let indentation = container::whitespace(
            &source.as_bytes()[line.start..line.end],
            line.column,
            line.padding,
            4,
        );
        if indentation.columns >= 4 {
            return Ok(None);
        }
        let start = line.start + indentation.bytes;
        if !source[start..line.end].starts_with("</") {
            return Ok(None);
        }
        let Some(stop) =
            custom_inline::tag_boundary(source, start, line.end, &self.options.custom_tags)
        else {
            return Ok(None);
        };
        let Some(gap) = source.as_bytes().get(pending.leaf_end()..start) else {
            return Ok(None);
        };
        if !whitespace(gap) {
            return Ok(None);
        }
        let closing = source[start + 2..stop - 1].trim_end_matches([' ', '\t']);
        let pending = self
            .custom_closure
            .as_mut()
            .unwrap()
            .pending
            .take()
            .unwrap();
        let target = match pending {
            Candidate::Known(target) => Some(target),
            Candidate::Deferred(leaf) => {
                let content = match &leaf.parts {
                    Some(parts) => self.parse_parts(source, parts, false)?,
                    None => self.parse_inline(source, leaf.content, false)?,
                };
                target(source, &leaf, &content)
            }
        };
        let Some(target) =
            target.filter(|target| &source[target.opening + 1..target.name_end] == closing)
        else {
            return Ok(None);
        };
        let closure = Closure {
            start: target.opening,
            end: stop,
        };
        self.custom_closure
            .as_mut()
            .unwrap()
            .resolved
            .insert(target.id, closure);
        output.update(Change::CloseInlineElement {
            id: target.id,
            start: closure.start,
            end: closure.end,
        });
        Ok(Some(stop))
    }
}
