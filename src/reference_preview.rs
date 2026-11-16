use super::{Active, Dependency, Engine, Output, reference_prefix, unindent};
use crate::{BlockId, Change, Preview, Span, inline};

pub(super) struct ReferencePreview {
    span: Span,
    blocks: Vec<BlockId>,
}

impl Engine {
    pub(super) fn preview_reference(&mut self, source: &str, output: &mut Output<'_>) -> bool {
        if !self.options.markstream
            || self.options.preview != Preview::Ai
            || !self.containers.is_empty()
            || self.running.is_some()
            || self.has_block_plugin(b'[')
        {
            self.clear_reference_preview(output);
            return false;
        }
        let start = match &self.active {
            None => self.scan - unindent(&source[self.line_start..self.scan]).len(),
            Some(Active::Paragraph(paragraph))
                if !paragraph.visible && paragraph.parts.is_empty() =>
            {
                paragraph.span.start
            }
            _ => {
                self.clear_reference_preview(output);
                return false;
            }
        };
        let span = Span::new(start, self.scan);
        if self
            .preview_state
            .as_ref()
            .and_then(|state| state.reference.as_ref())
            .is_some_and(|preview| preview.span == span)
        {
            return true;
        }
        self.clear_reference_preview(output);
        const LIMIT: usize = 1024;
        let raw = &source[start..span.end];
        if raw.len() > LIMIT
            || !raw.starts_with('[')
            || (self.options.footnotes && raw.starts_with("[^"))
        {
            return false;
        }
        let Some((label, reference, next)) = reference_prefix(source, start, span.end) else {
            return false;
        };
        if !source[next..span.end].trim().is_empty() {
            return false;
        }
        if let Some(draft) = self.draft.filter(|draft| draft.start == start) {
            self.draft = None;
            self.clear_inline_preview(draft.id, output);
            output.update(Change::Remove { id: draft.id });
        }
        let label = inline::normalize_label(label);
        if self.refs.get(label.as_ref()).is_some() {
            return true;
        }
        let Some(waiters) = self
            .waiting
            .as_ref()
            .and_then(|waiting| waiting.get(label.as_ref()))
        else {
            return true;
        };
        let mut budget = LIMIT;
        let mut targets = Vec::new();
        for dependency in waiters {
            if let Dependency::Leaf(leaf) = dependency {
                let bytes = leaf.content.end - leaf.content.start;
                let Some(remaining) = budget.checked_sub(bytes) else {
                    return true;
                };
                budget = remaining;
                targets.push(leaf.clone());
            }
        }
        if targets.is_empty() {
            return true;
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
        self.preview_state
            .get_or_insert_with(Box::default)
            .reference = Some(ReferencePreview { span, blocks });
        true
    }

    pub(super) fn clear_reference_preview(&mut self, output: &mut Output<'_>) {
        if let Some(preview) = self
            .preview_state
            .as_mut()
            .and_then(|state| state.reference.take())
        {
            for id in preview.blocks {
                output.update(Change::PreviewInlines {
                    id,
                    nodes: Vec::new(),
                });
            }
        }
    }
}
