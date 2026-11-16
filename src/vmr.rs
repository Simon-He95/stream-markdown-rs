use super::{Active, Engine, Line, Output, admonition, container};
use crate::{
    BlockStatus, Change, Span,
    container::{Frame, Kind},
};
use std::ops::Range;

/// Source-backed opening metadata for a Markstream colon container.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Vmr {
    pub header: Span,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VmrParameters {
    pub name: Span,
    pub args: Option<Span>,
    pub attributes: Option<Span>,
}

impl Vmr {
    pub fn parameters(self, source: &str) -> VmrParameters {
        let text = &source[Range::from(self.header)];
        let marker = marker_count(text);
        let rest = text[marker..].trim_start();
        let name_start = self.header.end - rest.len();
        let name_end = name_start
            + rest
                .find(|ch: char| ch == '{' || ch.is_whitespace())
                .unwrap_or(rest.len());
        let name = Span::new(name_start, name_end);
        let payload = source[name_end..self.header.end].trim_start();
        let payload_start = self.header.end - payload.len();
        let payload = payload.trim_end();
        let payload_end = payload_start + payload.len();
        if let Some(attrs) = payload.find('{') {
            let start = payload_start + attrs;
            if let Some(width) = balanced_object(&payload[attrs..]) {
                let args = source[payload_start..start].trim_end();
                return VmrParameters {
                    name,
                    args: (!args.is_empty())
                        .then_some(Span::new(payload_start, payload_start + args.len())),
                    attributes: Some(Span::new(start, start + width)),
                };
            }
        }
        VmrParameters {
            name,
            args: (payload_start < payload_end).then_some(Span::new(payload_start, payload_end)),
            attributes: None,
        }
    }
}

fn marker_count(text: &str) -> usize {
    text.as_bytes()
        .iter()
        .take_while(|&&byte| byte == b':')
        .count()
}

fn balanced_object(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote = None;
    let mut escape = false;
    for (position, byte) in text.bytes().enumerate() {
        if let Some(active) = quote {
            if escape {
                escape = false;
            } else if byte == b'\\' {
                escape = true;
            } else if byte == active {
                quote = None;
            }
        } else {
            match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(position + 1);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn header_start(source: &str, line: Line) -> Option<usize> {
    let leading = container::whitespace(
        &source.as_bytes()[line.start..line.end],
        line.column,
        line.padding,
        4,
    );
    (leading.columns < 4).then_some(line.start + leading.bytes)
}

fn opening(source: &str, line: Line) -> Option<Vmr> {
    let start = header_start(source, line)?;
    let head = &source[start..line.end];
    let count = marker_count(head);
    if count < 3 {
        return None;
    }
    let name = head[count..].trim_start();
    (!name.is_empty() && !name.starts_with('{')).then_some(Vmr {
        header: Span::new(start, line.end),
    })
}

impl Engine {
    pub(super) fn vmr_opener(&self, source: &str, line: Line) -> bool {
        opening(source, line).is_some()
    }

    pub(super) fn vmr_closer(&self, source: &str, line: Line, frame: Frame) -> bool {
        if line.lazy {
            return false;
        }
        let header_end = match frame.kind {
            Kind::Vmr { header_end, .. }
            | Kind::Admonition {
                header_end,
                quoted: false,
                ..
            } => header_end,
            _ => return false,
        };
        let Some(start) = header_start(source, line) else {
            return false;
        };
        let head = source[start..line.end].trim_end_matches([' ', '\t']);
        let count = marker_count(head);
        count >= 3 && count == head.len() && count >= marker_count(&source[frame.start..header_end])
    }

    pub(super) fn vmr_leaf_protected(&self) -> bool {
        self.running.is_some()
            || matches!(
                self.active,
                Some(Active::Fence(_) | Active::Html(_) | Active::Element(_) | Active::Math(_))
            )
    }

    pub(super) fn close_vmr(
        &mut self,
        source: &str,
        line: Line,
        index: usize,
        output: &mut Output<'_>,
    ) -> Result<(), crate::ParseError> {
        self.stop_leaf(source, output)?;
        let frame = &mut self.containers[index];
        if let Kind::Vmr {
            closed,
            preview_closed,
            ..
        }
        | Kind::Admonition {
            closed,
            preview_closed,
            quoted: false,
            ..
        } = &mut frame.kind
        {
            *closed = true;
            *preview_closed = false;
        }
        frame.end = line.next;
        self.close_containers(index, output);
        self.extend_containers(line.next);
        Ok(())
    }

    pub(super) fn open_vmr_line(
        &mut self,
        source: &str,
        line: Line,
        output: &mut Output<'_>,
    ) -> Result<bool, crate::ParseError> {
        if self.vmr_leaf_protected() {
            return Ok(false);
        }
        self.restore_vmr_preview(output);
        let Some(vmr) = opening(source, line) else {
            return Ok(false);
        };
        let kind = if let Some(kind) = admonition::colon_kind(source, vmr.header) {
            Kind::Admonition {
                header_end: vmr.header.end,
                kind,
                quoted: false,
                closed: false,
                preview_closed: false,
            }
        } else {
            Kind::Vmr {
                header_end: vmr.header.end,
                closed: false,
                preview_closed: false,
            }
        };
        if let Some(frame) = self.containers.last_mut()
            && frame.start == vmr.header.start
            && let Kind::Vmr { header_end, .. }
            | Kind::Admonition {
                header_end,
                quoted: false,
                ..
            } = frame.kind
        {
            if header_end != vmr.header.end || frame.end != line.next {
                frame.kind = kind;
                frame.end = line.next;
                output.close_container(
                    frame.batch_index,
                    super::container_block(*frame, BlockStatus::Open),
                );
            }
            return Ok(true);
        }

        self.stop_leaf(source, output)?;
        if let Some(draft) = self.draft.take() {
            output.update(Change::Remove { id: draft.id });
        }
        self.extend_containers(line.next);
        self.open_container(kind, vmr.header.start, line.next, output);
        Ok(true)
    }

    pub(super) fn partial_vmr(
        &mut self,
        source: &str,
        output: &mut Output<'_>,
    ) -> Result<bool, crate::ParseError> {
        if self.line_start >= self.scan || self.vmr_leaf_protected() {
            return Ok(false);
        }
        let line = Line::new(self.line_start, self.scan, self.scan);
        let Some(start) = header_start(source, line) else {
            return Ok(false);
        };
        let text = &source[start..line.end];
        let direct = self
            .containers
            .iter()
            .take_while(|frame| frame.kind.is_colon_container())
            .count();
        if let Some(index) = direct.checked_sub(1)
            && let Some(frame) = self.containers.get(index).copied()
            && frame.start < self.line_start
        {
            if self.vmr_closer(source, line, frame) {
                let (Kind::Vmr { preview_closed, .. }
                | Kind::Admonition {
                    preview_closed,
                    quoted: false,
                    ..
                }) = &mut self.containers[index].kind
                else {
                    unreachable!();
                };
                if !*preview_closed {
                    *preview_closed = true;
                    output.update(Change::SetStatus {
                        id: frame.id,
                        status: BlockStatus::Closed,
                        end: line.end,
                    });
                }
                return Ok(true);
            }
            self.restore_vmr_preview(output);
        }
        if direct != self.containers.len() {
            return Ok(false);
        }
        let tail = text.trim_end_matches([' ', '\t']);
        if !tail.is_empty() && marker_count(tail) == tail.len() {
            return Ok(true);
        }
        self.open_vmr_line(source, line, output)
    }

    fn restore_vmr_preview(&mut self, output: &mut Output<'_>) {
        if let Some(frame) = self
            .containers
            .iter_mut()
            .take_while(|frame| frame.kind.is_colon_container())
            .last()
            && let Kind::Vmr { preview_closed, .. }
            | Kind::Admonition {
                preview_closed,
                quoted: false,
                ..
            } = &mut frame.kind
            && *preview_closed
        {
            *preview_closed = false;
            output.update(Change::SetStatus {
                id: frame.id,
                status: BlockStatus::Open,
                end: frame.end,
            });
        }
    }
}
