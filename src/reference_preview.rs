use super::{Active, Dependency, Engine, Output, reference_prefix};
use crate::{BlockId, Change, Preview, Span, inline};

pub(super) struct ReferencePreview {
    span: Span,
    blocks: Vec<BlockId>,
}

impl Engine {
    pub(super) fn preview_reference(&mut self, source: &str, output: &mut Output<'_>) {
        if !self.options.markstream
            || self.options.preview != Preview::Ai
            || !self.containers.is_empty()
            || self.running.is_some()
        {
            self.clear_reference_preview(output);
            return;
        }
        let start = match &self.active {
            None => self.line_start,
            Some(Active::Paragraph(paragraph))
                if !paragraph.visible && paragraph.parts.is_empty() =>
            {
                paragraph.span.start
            }
            _ => {
                self.clear_reference_preview(output);
                return;
            }
        };
        let span = Span::new(start, self.scan);
        if self
            .reference_preview
            .as_ref()
            .is_some_and(|preview| preview.span == span)
        {
            return;
        }
        self.clear_reference_preview(output);
        const LIMIT: usize = 1024;
        if span.end - span.start > LIMIT || source.as_bytes().get(start) != Some(&b'[') {
            return;
        }
        let Some((label, reference, next)) = reference_prefix(source, start, span.end) else {
            return;
        };
        if !source[next..span.end].trim().is_empty() {
            return;
        }
        let label = inline::normalize_label(label);
        if self.refs.get(label.as_ref()).is_some() {
            return;
        }
        let Some(waiters) = self
            .waiting
            .as_ref()
            .and_then(|waiting| waiting.get(label.as_ref()))
        else {
            return;
        };
        let mut budget = LIMIT;
        let mut targets = Vec::new();
        for dependency in waiters {
            if let Dependency::Leaf(leaf) = dependency {
                let bytes = leaf.content.end - leaf.content.start;
                let Some(remaining) = budget.checked_sub(bytes) else {
                    return;
                };
                budget = remaining;
                targets.push(leaf.clone());
            }
        }
        if targets.is_empty() {
            return;
        }
        self.refs.insert(label.clone(), reference);
        let mut blocks = Vec::with_capacity(targets.len());
        for leaf in targets {
            let nodes = if let Some(parts) = leaf.parts {
                inline::parse_fragments_preview(source, &parts, &self.options, &self.refs)
            } else {
                inline::parse_preview(source, leaf.content, &self.options, &self.refs)
            };
            blocks.push(leaf.id);
            output.update(Change::PreviewInlines { id: leaf.id, nodes });
        }
        self.refs.remove(label.as_ref());
        self.reference_preview = Some(ReferencePreview { span, blocks });
    }

    pub(super) fn clear_reference_preview(&mut self, output: &mut Output<'_>) {
        if let Some(preview) = self.reference_preview.take() {
            for id in preview.blocks {
                output.update(Change::PreviewInlines {
                    id,
                    nodes: Vec::new(),
                });
            }
        }
    }
}
