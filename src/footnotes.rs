use super::{Active, Dependency, Draft, Engine, Line, Output, container, emit_append};
use crate::{
    Block, BlockData, BlockId, BlockStatus, Change, Inline, InlineContent, InlineKind, ParseError,
    Span, Text,
};
use std::{collections::HashMap, ops::Range, sync::Arc};

#[derive(Clone, Copy)]
pub(super) struct Reference {
    pub(super) label: Span,
    pub(super) end: usize,
}

#[derive(Clone, Copy)]
pub(super) struct Definition {
    pub(super) label: Span,
    pub(super) body: Span,
}

pub(super) fn reference(source: &str, start: usize, end: usize) -> Option<Reference> {
    let label_start = start + 2;
    let label = source.get(start..end)?.strip_prefix("[^")?;
    let width = label
        .as_bytes()
        .iter()
        .position(|byte| matches!(byte, b']' | b' ' | b'\r' | b'\n'))?;
    let stop = label_start + width;
    (width != 0 && source.as_bytes()[stop] == b']').then_some(Reference {
        label: Span::new(label_start, stop),
        end: stop + 1,
    })
}

pub(super) fn definition(source: &str, start: usize, end: usize) -> Option<Definition> {
    let Reference {
        label,
        end: marker_end,
    } = reference(source, start, end)?;
    let after_marker = source[marker_end..end].strip_prefix(':')?;
    let body = after_marker.trim_start_matches([' ', '\t']);
    Some(Definition {
        label,
        body: Span::new(end - body.len(), end),
    })
}

pub(super) fn pending_definition(source: &str, start: usize, end: usize) -> bool {
    let Some(tail) = source[start..end].strip_prefix("[^") else {
        return false;
    };
    let split = tail.find(']');
    let label = split.map_or(tail, |index| &tail[..index]);
    if label
        .chars()
        .any(|ch| ch.is_whitespace() || ch == '\u{feff}')
    {
        return false;
    }
    split.is_none_or(|index| {
        let suffix = &tail[index + 1..];
        suffix.is_empty()
            || suffix
                .strip_prefix(':')
                .is_some_and(|value| value.trim_matches([' ', '\t']).is_empty())
    })
}

#[derive(Default)]
pub(super) struct Index {
    definitions: HashMap<String, BlockId>,
    waiting: HashMap<String, Vec<Dependency>>,
    unresolved: Vec<String>,
    reference_draft: bool,
}

#[derive(Clone)]
pub(super) struct NoteBody {
    pub(super) id: BlockId,
    span: Span,
    label: Span,
    content: Span,
    parts: Option<Arc<[Span]>>,
}

pub(super) struct Note {
    id: BlockId,
    span: Span,
    label: Span,
    first: Span,
    continued: Vec<Span>,
    pending_blank: Option<Span>,
    present: bool,
}

impl Engine {
    pub(super) fn open_partial_footnote(
        &mut self,
        source: &str,
        output: &mut Output<'_>,
    ) -> Result<bool, ParseError> {
        let line = Line::new(self.line_start, self.scan, self.scan);
        if self.footnotes.is_none() || line.start >= line.end || !self.containers.is_empty() {
            return Ok(false);
        }
        let raw = &source[line.start..line.end];
        let leading = container::whitespace(raw.as_bytes(), 0, 0, 4);
        if let Some(Active::Footnote(note)) = &mut self.active {
            if line.start <= note.span.start {
                emit_append(output, note.id, &mut note.first.end, line.end);
                note.span.end = line.end;
                return Ok(true);
            }
            if leading.columns >= 4 || raw.trim_matches([' ', '\t']).is_empty() {
                return Ok(true);
            }
            let Some(Active::Footnote(note)) = self.active.take() else {
                unreachable!()
            };
            self.finish_footnote(source, note, output)?;
        }
        if leading.columns >= 4 || !matches!(self.active, None | Some(Active::Paragraph(_))) {
            return Ok(false);
        }
        let start = line.start + leading.bytes;
        if let Some(opener) = definition(source, start, line.end)
            && opener.body.start < opener.body.end
        {
            self.start_footnote(source, line, start, opener, output)?;
            return Ok(true);
        }

        let note = reference(source, start, line.end);
        let index = self.footnotes.as_mut().unwrap();
        if self.active.is_none()
            && let Some(note) = note
            && source.as_bytes().get(note.end) != Some(&b':')
            && (index.reference_draft || note.end == line.end)
            && let Some(&definition) = index.definitions.get(&source[Range::from(note.label)])
        {
            let (id, present) = self.block_id(start);
            let mut children = vec![Inline {
                span: Span::new(start, note.end),
                kind: InlineKind::FootnoteReference {
                    label: note.label,
                    definition,
                },
            }];
            if note.end < line.end {
                children.push(text(Span::new(note.end, line.end)));
            }
            let block = Block {
                id,
                span: Span::new(start, line.end),
                status: BlockStatus::Open,
                data: BlockData::Paragraph(InlineContent::Parsed(children)),
            };
            output.done(block, present, self.parent());
            self.draft = Some(Draft {
                id,
                start,
                published: line.end,
            });
            self.footnotes.as_mut().unwrap().reference_draft = true;
            return Ok(true);
        }
        if let Some(note) = note
            && note.end < line.end
            && source.as_bytes()[note.end] != b':'
        {
            match &mut self.active {
                Some(Active::Paragraph(paragraph)) if paragraph.visible => {
                    emit_append(output, paragraph.id, &mut paragraph.published, line.end);
                }
                None if !index.reference_draft => {
                    if let Some(draft) = &mut self.draft {
                        emit_append(output, draft.id, &mut draft.published, line.end);
                    } else {
                        let id = self.id();
                        let span = Span::new(start, line.end);
                        self.draft = Some(Draft {
                            id,
                            start,
                            published: line.end,
                        });
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
            return Ok(true);
        }
        if index.reference_draft
            && pending_definition(source, start, line.end)
            && let Some(draft) = self.draft.take()
        {
            output.update(Change::Remove { id: draft.id });
            index.reference_draft = false;
        }
        Ok(pending_definition(source, start, line.end))
    }

    pub(super) fn try_open_footnote(
        &mut self,
        source: &str,
        line: Line,
        start: usize,
        output: &mut Output<'_>,
    ) -> Result<bool, ParseError> {
        if self.footnotes.is_none() || !matches!(self.active, None | Some(Active::Paragraph(_))) {
            return Ok(false);
        }
        if let Some(opener) = definition(source, start, line.end) {
            self.start_footnote(source, line, start, opener, output)?;
            return Ok(true);
        }
        if self.footnotes.as_ref().unwrap().reference_draft {
            self.footnotes.as_mut().unwrap().reference_draft = false;
            if let Some(draft) = self.draft.as_mut() {
                let span = Span::new(draft.start, line.end);
                output.update(Change::Replace(Block {
                    id: draft.id,
                    span,
                    status: BlockStatus::Open,
                    data: BlockData::Paragraph(InlineContent::Plain(span)),
                }));
                draft.published = line.end;
            }
        }
        Ok(false)
    }

    fn start_footnote(
        &mut self,
        source: &str,
        line: Line,
        start: usize,
        opener: Definition,
        output: &mut Output<'_>,
    ) -> Result<(), ParseError> {
        self.stop_leaf(source, output)?;
        self.footnotes.as_mut().unwrap().reference_draft = false;
        let (id, present) = self.block_id(start);
        if !self.batch {
            output.done(
                Block {
                    id,
                    span: Span::new(start, line.next),
                    status: BlockStatus::Open,
                    data: BlockData::Footnote {
                        label_end: opener.label.end,
                        content: InlineContent::Plain(opener.body),
                    },
                },
                present,
                self.parent(),
            );
        }
        self.publish_footnote(source, opener.label, id, output)?;
        self.active = Some(Active::Footnote(Note {
            id,
            span: Span::new(start, line.next),
            label: opener.label,
            first: opener.body,
            continued: Vec::new(),
            pending_blank: None,
            present: !self.batch,
        }));
        Ok(())
    }

    pub(super) fn advance_footnote(
        &mut self,
        source: &str,
        mut note: Note,
        line: Line,
        output: &mut Output<'_>,
    ) -> Result<bool, ParseError> {
        if line.start <= note.span.start {
            emit_append(output, note.id, &mut note.first.end, line.end);
            note.span.end = line.next;
        } else if source[line.start..line.end]
            .trim_matches([' ', '\t'])
            .is_empty()
        {
            note.pending_blank
                .get_or_insert(Span::new(line.end, line.next));
        } else {
            let prefix = container::whitespace(
                &source.as_bytes()[line.start..line.end],
                line.column,
                line.padding,
                4,
            );
            if prefix.columns < 4 {
                self.finish_footnote(source, note, output)?;
                return Ok(false);
            }
            if note.continued.is_empty() {
                emit_append(output, note.id, &mut note.first.end, note.span.end);
                note.continued.push(note.first);
            }
            if let Some(blank) = note.pending_blank.take() {
                note.continued.push(blank);
                output.update(Change::AddTextFragment {
                    id: note.id,
                    span: blank,
                });
            }
            let span = Span::new(line.start + prefix.bytes, line.next);
            note.continued.push(span);
            output.update(Change::AddTextFragment { id: note.id, span });
            note.span.end = line.next;
        }
        self.active = Some(Active::Footnote(note));
        Ok(true)
    }

    pub(super) fn finish_footnote(
        &mut self,
        source: &str,
        note: Note,
        output: &mut Output<'_>,
    ) -> Result<(), ParseError> {
        let body = NoteBody {
            id: note.id,
            span: note.span,
            label: note.label,
            content: note.first,
            parts: (!note.continued.is_empty()).then(|| Arc::from(note.continued)),
        };
        self.emit_footnote_body(source, body, note.present, output)
    }

    pub(super) fn finish_footnote_body(
        &mut self,
        source: &str,
        body: NoteBody,
        output: &mut Output<'_>,
    ) -> Result<(), ParseError> {
        self.emit_footnote_body(source, body, true, output)
    }

    fn emit_footnote_body(
        &mut self,
        source: &str,
        body: NoteBody,
        present: bool,
        output: &mut Output<'_>,
    ) -> Result<(), ParseError> {
        self.unresolved.clear();
        self.clear_footnote_unresolved();
        let content = if let Some(parts) = &body.parts {
            if self.batch {
                self.batch_parts
                    .get_or_insert_with(std::collections::HashMap::new)
                    .insert(body.id, parts.clone());
                InlineContent::Plain(body.content)
            } else {
                self.parse_parts(source, parts, true)?
            }
        } else {
            self.inline(source, body.content)?
        };
        self.track_footnote_dependency(&Dependency::Footnote(body.clone()));
        if !self.unresolved.is_empty() {
            self.track_dependency(Dependency::Footnote(body.clone()));
        }
        output.done(
            Block {
                id: body.id,
                span: body.span,
                status: BlockStatus::Closed,
                data: BlockData::Footnote {
                    label_end: body.label.end,
                    content,
                },
            },
            present,
            self.parent(),
        );
        Ok(())
    }

    pub(super) fn resolve_footnote_nodes(
        &mut self,
        source: &str,
        nodes: &mut Vec<Inline>,
        track: bool,
    ) {
        if let Some(index) = self.footnotes.as_mut() {
            resolve_nodes(index, source, nodes, track);
        }
    }

    pub(super) fn clear_footnote_unresolved(&mut self) {
        if let Some(index) = self.footnotes.as_mut() {
            index.unresolved.clear();
        }
    }

    pub(super) fn has_unresolved_footnotes(&self) -> bool {
        self.footnotes
            .as_ref()
            .is_some_and(|index| !index.unresolved.is_empty())
    }

    pub(super) fn track_footnote_dependency(&mut self, dependency: &Dependency) {
        let Some(index) = self.footnotes.as_mut() else {
            return;
        };
        let key = dependency.key();
        index.unresolved.sort_unstable();
        index.unresolved.dedup();
        for label in index.unresolved.drain(..) {
            let waiters = index.waiting.entry(label).or_default();
            if let Err(position) = waiters.binary_search_by_key(&key, Dependency::key) {
                waiters.insert(position, dependency.clone());
            }
        }
    }

    fn publish_footnote(
        &mut self,
        source: &str,
        label: Span,
        id: BlockId,
        output: &mut Output<'_>,
    ) -> Result<(), ParseError> {
        let index = self.footnotes.as_mut().unwrap();
        let label = &source[Range::from(label)];
        if index.definitions.contains_key(label) {
            return Ok(());
        }
        index.definitions.insert(label.to_owned(), id);
        if !self.batch
            && let Some(waiters) = index.waiting.remove(label)
        {
            let mut previous = None;
            for dependency in waiters {
                let id = dependency.key().0;
                if previous != Some(id) {
                    self.metrics.invalidated_blocks += 1;
                    previous = Some(id);
                }
                match dependency {
                    Dependency::Leaf(leaf) => self.finish_leaf(source, leaf, true, output)?,
                    Dependency::Element(body) => {
                        self.emit_element_body(source, *body, true, output)?
                    }
                    Dependency::Cell { id, index, span } => {
                        let content = self.stream_cell(source, id, index, span)?;
                        output.update(Change::ReplaceTableCell { id, index, content });
                    }
                    Dependency::Footnote(body) => {
                        self.finish_footnote_body(source, body, output)?
                    }
                    Dependency::Summary(body) => self.finish_summary_body(source, body, output)?,
                }
            }
        }
        Ok(())
    }
}

fn resolve_nodes(index: &mut Index, source: &str, nodes: &mut Vec<Inline>, track: bool) {
    let mut replacements = Vec::new();
    for (position, node) in nodes.iter_mut().enumerate() {
        match &mut node.kind {
            InlineKind::Text(Text::Source(span)) => {
                let content = &source[Range::from(*span)];
                if let Some(found) = content.find("[^") {
                    let mut start = span.start + found;
                    let mut published = span.start;
                    let mut raw_start = node.span.start;
                    let mut parts = Vec::new();
                    while start < span.end {
                        if !(start == span.start && raw_start != span.start)
                            && let Some(note) = reference(source, start, span.end)
                        {
                            let label = &source[Range::from(note.label)];
                            if let Some(&definition) = index.definitions.get(label) {
                                if published < start {
                                    parts.push(Inline {
                                        span: Span::new(raw_start, start),
                                        kind: InlineKind::Text(Text::Source(Span::new(
                                            published, start,
                                        ))),
                                    });
                                }
                                parts.push(Inline {
                                    span: Span::new(start, note.end),
                                    kind: InlineKind::FootnoteReference {
                                        label: note.label,
                                        definition,
                                    },
                                });
                                published = note.end;
                                raw_start = published;
                            } else if track {
                                index.unresolved.push(label.to_owned());
                            }
                            start = note.end;
                        } else {
                            start += 2;
                        }
                        start = source[start..span.end]
                            .find("[^")
                            .map_or(span.end, |next| start + next);
                    }
                    if !parts.is_empty() {
                        if published < span.end {
                            parts.push(text(Span::new(published, span.end)));
                        }
                        replacements.push((position, parts));
                    }
                }
            }
            InlineKind::Emphasis(children)
            | InlineKind::Strong(children)
            | InlineKind::Strikethrough(children) => resolve_nodes(index, source, children, track),
            _ => {}
        }
    }
    if replacements.is_empty() {
        return;
    }
    let old = std::mem::take(nodes);
    let mut replacements = replacements.into_iter().peekable();
    for (position, node) in old.into_iter().enumerate() {
        if replacements
            .peek()
            .is_some_and(|(target, _)| *target == position)
        {
            nodes.extend(replacements.next().unwrap().1);
        } else {
            nodes.push(node);
        }
    }
}

fn text(span: Span) -> Inline {
    Inline {
        span,
        kind: InlineKind::Text(Text::Source(span)),
    }
}
