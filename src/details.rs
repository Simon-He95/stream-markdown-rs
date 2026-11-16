use super::{Active, Dependency, Engine, Html, Line, Output, container, tag};
use crate::{Block, BlockData, BlockId, BlockStatus, Change, InlineContent, ParseError, Span, Tag};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtmlOpening {
    pub span: Span,
}

impl HtmlOpening {
    pub fn tag(self, source: &str) -> Tag {
        tag::opening(source, self.span.start, self.span.end)
            .expect("HtmlOpening contains a validated tag")
            .tag
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Name {
    Details,
    Summary,
    Div,
    Section,
}

impl Name {
    fn opening(self) -> &'static [u8] {
        match self {
            Self::Details => b"<details",
            Self::Summary => b"<summary",
            Self::Div => b"<div",
            Self::Section => b"<section",
        }
    }

    fn closing(self) -> &'static [u8] {
        match self {
            Self::Details => b"</details",
            Self::Summary => b"</summary",
            Self::Div => b"</div",
            Self::Section => b"</section",
        }
    }

    pub(super) fn data(self, opening: HtmlOpening) -> BlockData {
        if self == Self::Details {
            BlockData::Details(opening)
        } else {
            BlockData::HtmlWrapper(opening)
        }
    }
}

enum EndTag {
    Absent,
    Pending,
    Complete(usize),
}

pub(super) enum HtmlProgress {
    Scanning(usize),
    ClosingLine(BlockId),
    // The closing tag ended, and the physical line still has content.
    Tail(usize),
}

impl HtmlProgress {
    pub(super) fn has_tail(&self) -> bool {
        matches!(self, Self::Tail(_))
    }
}

fn match_end(source: &str, start: usize, end: usize, name: Name) -> EndTag {
    let candidate = &source.as_bytes()[start..end];
    let mark = name.closing();
    let common = mark.len().min(candidate.len());
    if !candidate[..common].eq_ignore_ascii_case(&mark[..common]) {
        return EndTag::Absent;
    }
    if common < mark.len() {
        return EndTag::Pending;
    }
    let mut offset = start + mark.len();
    while offset < end && matches!(source.as_bytes()[offset], b' ' | b'\t') {
        offset += 1;
    }
    match source.as_bytes().get(offset).filter(|_| offset < end) {
        None => EndTag::Pending,
        Some(b'>') => EndTag::Complete(offset + 1),
        _ => EndTag::Absent,
    }
}

fn find_end(
    source: &str,
    mut start: usize,
    end: usize,
    names: &[Name],
) -> (usize, Option<(Name, Span)>) {
    while let Some(relative) = memchr::memchr(b'<', &source.as_bytes()[start..end]) {
        let position = start + relative;
        for &name in names {
            match match_end(source, position, end, name) {
                EndTag::Complete(stop) => {
                    return (position, Some((name, Span::new(position, stop))));
                }
                EndTag::Pending => return (position, None),
                EndTag::Absent => {}
            }
        }
        let tail = &source[position..end];
        if let Some(kind) = tag::LiteralKind::at_start(tail.as_bytes()) {
            let Some(close) =
                kind.find_end(source.as_bytes(), position + kind.opening().len(), end)
            else {
                return (position, None);
            };
            start = close;
        } else if tag::LiteralKind::pending(tail.as_bytes()) {
            return (position, None);
        } else if let Some(opening) = tag::opening(source, position, end) {
            start = opening.end;
        } else {
            start = position + 1;
        }
    }
    (end, None)
}

fn tag_start(source: &str, line: Line) -> Option<usize> {
    let leading = container::whitespace(
        &source.as_bytes()[line.start..line.end],
        line.column,
        line.padding,
        4,
    );
    (leading.columns < 4).then_some(line.start + leading.bytes)
}

fn start_name(source: &str, start: usize, end: usize, in_details: bool) -> Option<Name> {
    let bytes = &source.as_bytes()[start..end];
    [Name::Details, Name::Summary].into_iter().find(|name| {
        if *name == Name::Summary && !in_details {
            return false;
        }
        let mark = name.opening();
        bytes.len() > mark.len()
            && bytes[..mark.len()].eq_ignore_ascii_case(mark)
            && matches!(bytes[mark.len()], b' ' | b'\t' | b'/' | b'>')
    })
}

fn wrapper_remains_open(source: &str, mut at: usize, end: usize, name: Name) -> bool {
    let bytes = source.as_bytes();
    let mut depth = 1usize;
    while let Some(relative) = memchr::memchr(b'<', &bytes[at..end]) {
        let position = at + relative;
        if let Some(kind) = tag::LiteralKind::at_start(&bytes[position..end]) {
            let Some(close) = kind.find_end(bytes, position + kind.opening().len(), end) else {
                return false;
            };
            at = close;
        } else if let EndTag::Complete(close) = match_end(source, position, end, name) {
            depth -= 1;
            if depth == 0 {
                return false;
            }
            at = close;
        } else if let Some(opener) = tag::opening(source, position, end) {
            let inner = opener.tag.tag_name(source).as_bytes();
            depth += usize::from(
                !opener.self_closing && inner.eq_ignore_ascii_case(&name.opening()[1..]),
            );
            at = opener.end;
            if let Some(raw) = tag::RawTextTag::from_name(inner) {
                let Some(close) = raw.find_end(bytes, at, end) else {
                    return false;
                };
                at = close;
            }
        } else {
            at = position + 1;
        }
    }
    true
}

#[derive(Clone)]
pub(super) struct SummaryBody {
    pub(super) id: BlockId,
    opening: HtmlOpening,
    span: Span,
    body: Span,
    parts: Option<Arc<[Span]>>,
    status: BlockStatus,
}

pub(super) struct Summary {
    id: BlockId,
    opening: HtmlOpening,
    body: Span,
    parts: Vec<Span>,
    checked: usize,
}

impl Engine {
    pub(super) fn resume_html_line_ending(
        &mut self,
        source: &str,
        final_input: bool,
        output: &mut Output<'_>,
    ) {
        let HtmlProgress::ClosingLine(id) = self.html_progress else {
            return;
        };
        let bytes = source.as_bytes();
        let mut at = self.scan;
        while matches!(bytes.get(at), Some(b' ' | b'\t')) {
            at += 1;
        }
        if !final_input && (at == bytes.len() || bytes.get(at..) == Some(b"\r")) {
            return;
        }
        let end = if at == bytes.len() {
            Some(at)
        } else {
            super::line_break(bytes, at, bytes.len())
        };
        self.html_progress = if end.is_some() {
            HtmlProgress::Scanning(self.line_start)
        } else {
            HtmlProgress::Tail(self.line_start)
        };
        if let Some(end) = end
            && end > self.line_start
        {
            self.metrics.scanned_bytes += end - self.scan;
            self.scan = end;
            self.line_start = end;
            self.extend_containers(end);
            output.update(Change::SetStatus {
                id,
                status: BlockStatus::Closed,
                end,
            });
        }
    }

    #[inline]
    pub(crate) fn publish_html_container_progress(
        &self,
        previous_end: usize,
        out: &mut Vec<Change>,
    ) {
        if self.options.markstream {
            for frame in &self.containers {
                if let container::Kind::Html { opening_end, .. } = frame.kind
                    && frame.end > previous_end.max(opening_end)
                {
                    out.push(Change::SetStatus {
                        id: frame.id,
                        status: BlockStatus::Open,
                        end: frame.end,
                    });
                }
            }
        }
    }

    fn details_index(&self) -> Option<usize> {
        self.containers.iter().rposition(|frame| {
            matches!(
                frame.kind,
                container::Kind::Html {
                    tag: Name::Details,
                    ..
                }
            )
        })
    }

    fn html_container(&self) -> Option<(usize, Name)> {
        self.containers
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, frame)| match frame.kind {
                container::Kind::Html { tag, .. } => Some((index, tag)),
                _ => None,
            })
    }

    pub(super) fn promote_html_wrapper(
        &mut self,
        source: &str,
        html: &Html,
        blank: Line,
        output: &mut Output<'_>,
    ) -> Result<bool, ParseError> {
        let bytes = source.as_bytes();
        let Some(relative) = memchr::memchr2(b'\r', b'\n', &bytes[html.span.start..html.span.end])
        else {
            return Ok(false);
        };
        let end = html.span.start + relative;
        let mut next = super::line_break(bytes, end, html.span.end).unwrap();
        let start = super::unindent(&source[html.span.start..end]);
        let start = end - start.len();
        let Some(opener) = tag::opening(source, start, end).filter(|open| {
            !open.self_closing && source[open.end..end].trim_matches([' ', '\t']).is_empty()
        }) else {
            return Ok(false);
        };
        let name = opener.tag.tag_name(source).as_bytes();
        let Some(tag) = [Name::Div, Name::Section]
            .into_iter()
            .find(|tag| name.eq_ignore_ascii_case(&tag.opening()[1..]))
        else {
            return Ok(false);
        };
        if !wrapper_remains_open(source, next, html.span.end, tag) {
            return Ok(false);
        }
        let frame = container::Frame {
            id: html.id,
            output_index: output.container_index(self.options.markstream),
            start,
            end: html.span.end,
            kind: container::Kind::Html {
                opening_end: opener.end,
                closed: false,
                tag,
            },
        };
        output.done(
            super::container_block(frame, BlockStatus::Open),
            !self.batch,
            self.parent(),
        );
        self.containers.push(frame);
        // The first blank line confirms a wrapper. Parse its buffered body once as Markdown.
        self.metrics.scanned_bytes += html.span.end - next;
        while next < html.span.end {
            let end = memchr::memchr2(b'\r', b'\n', &bytes[next..html.span.end])
                .map_or(html.span.end, |relative| next + relative);
            let after = super::line_break(bytes, end, html.span.end).unwrap_or(end);
            self.container_line(source, Line::new(next, end, after), output)?;
            next = after;
        }
        self.container_line(source, blank, output)?;
        Ok(true)
    }

    pub(super) fn details_opener(&self, source: &str, line: Line) -> bool {
        tag_start(source, line).is_some_and(|start| {
            start_name(source, start, line.end, self.details_index().is_some()).is_some()
                && tag::opening(source, start, line.end).is_some()
        })
    }

    pub(super) fn details_closer(
        &self,
        source: &str,
        line: Line,
        frame: container::Frame,
    ) -> Option<usize> {
        let container::Kind::Html { tag, .. } = frame.kind else {
            return None;
        };
        if line.lazy {
            return None;
        }
        match match_end(source, tag_start(source, line)?, line.end, tag) {
            EndTag::Complete(end) => Some(end),
            EndTag::Pending | EndTag::Absent => None,
        }
    }

    pub(super) fn details_leaf_protected(&self) -> bool {
        self.running.is_some()
            || matches!(
                self.active,
                Some(Active::Fence(_) | Active::Element(_) | Active::Html(_) | Active::Math(_))
            )
    }

    pub(super) fn close_details(
        &mut self,
        source: &str,
        line: Line,
        index: usize,
        close_end: usize,
        output: &mut Output<'_>,
    ) -> Result<(), ParseError> {
        self.stop_leaf(source, output)?;
        let tail = !source[close_end..line.end]
            .trim_matches([' ', '\t'])
            .is_empty();
        let end = if tail { close_end } else { line.next };
        let frame = &mut self.containers[index];
        if let container::Kind::Html { closed, .. } = &mut frame.kind {
            *closed = true;
        }
        frame.end = end;
        if !self.finalizing && line.next == line.end && close_end <= self.scan {
            self.html_progress = if source.as_bytes()[close_end..self.scan]
                .iter()
                .all(|&byte| matches!(byte, b' ' | b'\t'))
            {
                HtmlProgress::ClosingLine(frame.id)
            } else {
                HtmlProgress::Tail(close_end)
            };
        }
        self.close_containers(index, output);
        self.extend_containers(end);
        if tail {
            self.line(
                source,
                Line {
                    start: close_end,
                    ..line
                },
                output,
            )?;
        }
        Ok(())
    }

    pub(super) fn open_details_line(
        &mut self,
        source: &str,
        line: Line,
        output: &mut Output<'_>,
    ) -> Result<bool, ParseError> {
        if self.details_leaf_protected() || matches!(self.active, Some(Active::Summary(_))) {
            return Ok(false);
        }
        let current = self.details_index();
        if let Some(start) = tag_start(source, line)
            && let Some(name) = start_name(source, start, line.end, current.is_some())
            && let Some(tag) = tag::opening(source, start, line.end)
        {
            self.stop_leaf(source, output)?;
            if let Some(draft) = self.draft.take() {
                output.update(Change::Remove { id: draft.id });
            }
            self.extend_containers(tag.end);
            let opening = HtmlOpening {
                span: Span::new(start, tag.end),
            };
            let remainder = Line {
                start: tag.end,
                ..line
            };

            if !tag.self_closing && name == Name::Details {
                self.open_container(
                    container::Kind::Html {
                        opening_end: tag.end,
                        closed: false,
                        tag: Name::Details,
                    },
                    start,
                    tag.end,
                    output,
                );
            } else {
                let id = self.id();
                let body = Span::new(tag.end, tag.end);
                if tag.self_closing {
                    let data = if name == Name::Details {
                        BlockData::Details(opening)
                    } else {
                        BlockData::Summary {
                            opening_end: opening.span.end,
                            content: InlineContent::Plain(body),
                        }
                    };
                    output.done(
                        Block {
                            id,
                            span: opening.span,
                            status: BlockStatus::Closed,
                            data,
                        },
                        false,
                        self.parent(),
                    );
                } else {
                    output.open(
                        Block {
                            id,
                            span: opening.span,
                            status: BlockStatus::Open,
                            data: BlockData::Summary {
                                opening_end: opening.span.end,
                                content: InlineContent::Plain(body),
                            },
                        },
                        self.parent(),
                    );
                    self.advance_summary(
                        source,
                        Summary {
                            id,
                            opening,
                            body,
                            parts: Vec::new(),
                            checked: tag.end,
                        },
                        remainder,
                        output,
                    )?;
                    return Ok(true);
                }
            }
            if !source[tag.end..line.end]
                .trim_matches([' ', '\t'])
                .is_empty()
            {
                self.line(source, remainder, output)?;
            }
            self.extend_containers(line.next);
            return Ok(true);
        }

        if let Some((index, tag)) = self.html_container()
            && let (_, Some((_, close))) = find_end(source, line.start, line.end, &[tag])
        {
            if !source[line.start..close.start]
                .trim_matches([' ', '\t'])
                .is_empty()
            {
                self.line(
                    source,
                    Line {
                        end: close.start,
                        next: close.start,
                        ..line
                    },
                    output,
                )?;
            }
            self.close_details(source, line, index, close.end, output)?;
            return Ok(true);
        }
        Ok(false)
    }

    pub(super) fn advance_summary(
        &mut self,
        source: &str,
        mut summary: Summary,
        line: Line,
        output: &mut Output<'_>,
    ) -> Result<(), ParseError> {
        let (_, close) = find_end(
            source,
            summary.checked.max(line.start),
            line.end,
            &[Name::Details, Name::Summary],
        );
        if let Some((name, close)) = close {
            self.update_summary(&mut summary, line.start, close.start, output);
            let end = if name == Name::Summary {
                close.end
            } else {
                close.start
            };
            self.close_summary(
                source,
                summary,
                end,
                if name == Name::Summary {
                    BlockStatus::Closed
                } else {
                    BlockStatus::Incomplete
                },
                output,
            )?;
            if !source[end..line.end].trim_matches([' ', '\t']).is_empty() {
                self.line(source, Line { start: end, ..line }, output)?;
            }
        } else {
            self.update_summary(&mut summary, line.start, line.next, output);
            summary.checked = line.next;
            self.active = Some(Active::Summary(summary));
        }
        self.extend_containers(line.next);
        Ok(())
    }

    pub(super) fn finish_summary(
        &mut self,
        source: &str,
        summary: Summary,
        output: &mut Output<'_>,
    ) -> Result<(), ParseError> {
        let end = summary.body.end;
        self.close_summary(source, summary, end, BlockStatus::Incomplete, output)
    }

    pub(super) fn partial_details(
        &mut self,
        source: &str,
        output: &mut Output<'_>,
    ) -> Result<bool, ParseError> {
        if self.details_leaf_protected() {
            return Ok(false);
        }
        let transparent = |frame: &container::Frame| {
            matches!(
                frame.kind,
                container::Kind::Html { .. } | container::Kind::Vmr { .. }
            )
        };
        if !self.containers.iter().all(transparent) {
            let Some((index, tag)) = self.html_container() else {
                return Ok(false);
            };
            if !self.containers[..index].iter().all(transparent)
                || !self.containers[index + 1..].iter().all(|frame| {
                    matches!(
                        frame.kind,
                        container::Kind::List { .. } | container::Kind::Item { .. }
                    )
                })
            {
                return Ok(false);
            }
            let line = Line::new(self.line_start, self.scan, self.scan);
            if let Some(start) = tag_start(source, line)
                && let EndTag::Complete(end) = match_end(source, start, line.end, tag)
            {
                self.container_line(source, Line::new(self.line_start, end, end), output)?;
                self.line_start = end;
            }
            return Ok(false);
        }

        while self.line_start < self.scan {
            let checked = match self.html_progress {
                HtmlProgress::Scanning(checked) | HtmlProgress::Tail(checked) => checked,
                HtmlProgress::ClosingLine(_) => return Ok(false),
            };
            let line = Line::new(self.line_start, self.scan, self.scan);
            if matches!(self.active, Some(Active::Summary(_))) {
                let Some(Active::Summary(mut summary)) = self.active.take() else {
                    unreachable!();
                };
                let (through, close) = find_end(
                    source,
                    summary.checked.max(line.start),
                    line.end,
                    &[Name::Details, Name::Summary],
                );
                self.update_summary(&mut summary, line.start, through, output);
                if let Some((name, close)) = close {
                    let end = if name == Name::Summary {
                        close.end
                    } else {
                        close.start
                    };
                    self.close_summary(
                        source,
                        summary,
                        end,
                        if name == Name::Summary {
                            BlockStatus::Closed
                        } else {
                            BlockStatus::Incomplete
                        },
                        output,
                    )?;
                    self.line_start = end;
                    continue;
                }
                summary.checked = through;
                self.active = Some(Active::Summary(summary));
                self.extend_containers(through);
                return Ok(true);
            }
            // An ordinary active leaf remains owned by the enclosing block parser.
            let start = tag_start(source, line).unwrap_or(line.start);
            let in_details = self.details_index().is_some();
            if let Some(name) = start_name(source, start, line.end, in_details) {
                if self
                    .tag_probe
                    .as_ref()
                    .is_none_or(|probe| probe.start != start)
                {
                    self.tag_probe =
                        Some(tag::OpeningProbe::new(start, start + name.opening().len()));
                }
                let probe = self.tag_probe.as_mut().unwrap();
                let Some(end) = probe.update(source, line.end) else {
                    return Ok(true);
                };
                if tag::opening(source, start, end).is_none() {
                    probe.reject();
                    return Ok(false);
                }
                self.tag_probe = None;
                self.line(source, Line::new(self.line_start, end, end), output)?;
                self.line_start = end;
            } else if let Some((_, tag)) = self.html_container() {
                let from = checked.max(line.start).min(line.end);
                let (through, close) = find_end(source, from, line.end, &[tag]);
                self.html_progress = if self.html_progress.has_tail() {
                    HtmlProgress::Tail(through)
                } else {
                    HtmlProgress::Scanning(through)
                };
                let Some((_, close)) = close else {
                    return Ok(false);
                };
                // Let the physical line consume any container prefix before this closer.
                if !self.html_progress.has_tail()
                    && container::opening_at(
                        &source[line.start..close.start],
                        line.column,
                        line.padding,
                    )
                    .is_some()
                {
                    return Ok(false);
                }
                self.line(
                    source,
                    Line::new(self.line_start, close.end, close.end),
                    output,
                )?;
                self.line_start = close.end;
            } else {
                return Ok(false);
            }
        }
        Ok(false)
    }

    fn update_summary(
        &self,
        summary: &mut Summary,
        start: usize,
        end: usize,
        output: &mut Output<'_>,
    ) {
        let start = start.max(summary.body.end);
        if end <= start {
            return;
        }
        let append = start == summary.body.end;
        if append {
            if let Some(last) = summary.parts.last_mut() {
                last.end = end;
            }
            output.append_text(summary.id, Span::new(start, end));
        } else {
            if summary.parts.is_empty() {
                summary.parts.push(summary.body);
            }
            let part = Span::new(start, end);
            summary.parts.push(part);
            output.update(Change::AddTextFragment {
                id: summary.id,
                span: part,
            });
        }
        summary.body.end = end;
    }

    fn close_summary(
        &mut self,
        source: &str,
        summary: Summary,
        end: usize,
        status: BlockStatus,
        output: &mut Output<'_>,
    ) -> Result<(), ParseError> {
        self.emit_summary(
            source,
            SummaryBody {
                id: summary.id,
                opening: summary.opening,
                span: Span::new(summary.opening.span.start, end),
                body: summary.body,
                parts: (!summary.parts.is_empty()).then(|| Arc::from(summary.parts)),
                status,
            },
            !self.batch,
            output,
        )
    }

    pub(super) fn finish_summary_body(
        &mut self,
        source: &str,
        body: SummaryBody,
        output: &mut Output<'_>,
    ) -> Result<(), ParseError> {
        self.emit_summary(source, body, true, output)
    }

    fn emit_summary(
        &mut self,
        source: &str,
        body: SummaryBody,
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
                InlineContent::Plain(body.body)
            } else {
                self.parse_parts(source, parts, true)?
            }
        } else {
            self.inline(source, body.body)?
        };
        if self.options.footnotes {
            self.track_footnote_dependency(&Dependency::Summary(body.clone()));
        }
        if !self.unresolved.is_empty() {
            self.track_dependency(Dependency::Summary(body.clone()));
        }
        output.done(
            Block {
                id: body.id,
                span: body.span,
                status: body.status,
                data: BlockData::Summary {
                    opening_end: body.opening.span.end,
                    content,
                },
            },
            present,
            self.parent(),
        );
        Ok(())
    }
}
