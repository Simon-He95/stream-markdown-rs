use std::collections::BTreeMap;

use stream_markdown_rs::{
    Block, BlockData, BlockId, BlockStatus, Change, InlineContent, InlineKind, Options, ParseError,
    Parser, Preview, PreviewHint, parse,
};

#[derive(Default)]
struct View {
    blocks: BTreeMap<BlockId, Block>,
}

impl View {
    fn apply(&mut self, changes: &mut Vec<Change>) {
        for change in changes.drain(..) {
            match change {
                Change::Insert {
                    block,
                    parent: None,
                } => {
                    assert!(self.blocks.insert(block.id, block).is_none());
                }
                Change::Replace(block) => {
                    assert!(self.blocks.insert(block.id, block).is_some());
                }
                Change::Remove { id } => {
                    assert!(self.blocks.remove(&id).is_some());
                }
                Change::AppendText { id, span } => {
                    let block = self.blocks.get_mut(&id).unwrap();
                    let body = match &mut block.data {
                        BlockData::Paragraph(InlineContent::Plain(body))
                        | BlockData::CodeBlock { body, .. }
                        | BlockData::CustomTag { body, .. } => body,
                        _ => panic!("unexpected text update: {block:?}"),
                    };
                    assert_eq!(span.start, body.end);
                    body.end = span.end;
                    block.span.end = span.end;
                }
                Change::AddRow { id, end, cells } => {
                    let block = self.blocks.get_mut(&id).unwrap();
                    let BlockData::Table { rows, .. } = &mut block.data else {
                        panic!("unexpected row update: {block:?}");
                    };
                    rows.push(cells);
                    block.span.end = end;
                }
                Change::SetStatus { id, status, end } => {
                    let block = self.blocks.get_mut(&id).unwrap();
                    block.status = status;
                    block.span.end = end;
                }
                Change::Preview { .. } => {}
                change => panic!("unexpected nested change: {change:?}"),
            }
        }
    }
}

#[test]
fn every_character_boundary_produces_the_same_final_view() {
    let options = Options {
        preview: Preview::Ai,
        custom_tags: vec!["tool-call".into()],
        ..Options::default()
    };
    let source = "中文 and **rich**\r\n\r\nTitle\n---\n\n```rust\nlet a = 1;\n```\n\nA | B\n| - | -: |\n| *Hi* | 5 |\n\n[old][id]\n\n[id]: /ok \"link\"\n\n<tool-call name=\"search\" title=\"a > b\">\nbody\n</tool-call>\n\n~~~\nstill open";
    let expected = parse(source, &options).unwrap().nodes;
    for (split, _) in source.char_indices() {
        let mut parser = Parser::new(options.clone());
        let mut changes = Vec::new();
        let mut view = View::default();
        for chunk in [&source[..split], &source[split..]] {
            parser.append(chunk, &mut changes).unwrap();
            view.apply(&mut changes);
        }
        parser.finish(&mut changes).unwrap();
        view.apply(&mut changes);
        assert_eq!(
            view.blocks.into_values().collect::<Vec<_>>(),
            expected,
            "split {split}"
        );
        assert_eq!(
            parser.metrics().scanned_bytes,
            source.len(),
            "split {split}"
        );
    }
}

#[test]
fn growing_paragraph_scans_each_input_byte_once() {
    let mut parser = Parser::new(Options::default());
    let mut changes = Vec::new();
    let chunk = "Growing content with 中文 and **emphasis**. ";
    let mut view = View::default();
    for _ in 0..1024 {
        parser.append(chunk, &mut changes).unwrap();
        assert_eq!(changes.len(), 1);
        view.apply(&mut changes);
    }
    let len = parser.source_text().len();
    assert_eq!(parser.metrics().scanned_bytes, len);
    assert_eq!(parser.metrics().inline_bytes, 0);
    parser.finish(&mut changes).unwrap();
    view.apply(&mut changes);
    assert_eq!(view.blocks.len(), 1);
    assert_eq!(parser.metrics().inline_bytes, len - 1);
}

#[test]
fn late_reference_changes_only_the_dependent_closed_block() {
    let mut parser = Parser::new(Options::default());
    let mut changes = Vec::new();
    parser
        .append("Early [link][ref].\n\nUnaffected.\n\n", &mut changes)
        .unwrap();
    let mut view = View::default();
    view.apply(&mut changes);
    let id = view.blocks.first_key_value().unwrap().0.to_owned();
    parser
        .append("[ref]: https://example.com\n\n", &mut changes)
        .unwrap();
    assert_eq!(changes.len(), 1);
    let Change::Replace(block) = &changes[0] else {
        panic!("expected linked replacement");
    };
    assert_eq!(block.id, id);
    assert_eq!(block.status, BlockStatus::Closed);
    let BlockData::Paragraph(InlineContent::Parsed(inlines)) = &block.data else {
        panic!("expected paragraph");
    };
    assert!(
        inlines
            .iter()
            .any(|inline| matches!(inline.kind, InlineKind::Link(_)))
    );
    assert_eq!(parser.metrics().invalidated_blocks, 1);
}

#[test]
fn table_preview_requires_a_delimiter_and_keeps_the_block_id() {
    let mut parser = Parser::new(Options {
        preview: Preview::Ai,
        ..Options::default()
    });
    let mut changes = Vec::new();
    parser.append("A | B\n", &mut changes).unwrap();
    let Change::Insert { block, .. } = &changes[0] else {
        panic!("expected paragraph");
    };
    let id = block.id;
    changes.clear();
    parser.append("|", &mut changes).unwrap();
    assert!(changes.is_empty());
    parser.append("-", &mut changes).unwrap();
    assert_eq!(
        changes,
        [Change::Preview {
            id,
            hint: Some(PreviewHint::Table)
        }]
    );
    changes.clear();
    parser.append("| - |\n", &mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(
        change, Change::Replace(block) if block.id == id && matches!(block.data, BlockData::Table { .. })
    )));
}

#[test]
fn finish_closes_fences_once_and_preserves_the_body() {
    let mut parser = Parser::new(Options::default());
    let mut changes = Vec::new();
    let mut view = View::default();
    for chunk in ["``", "`rust\n", "let ", "a = 5;"] {
        parser.append(chunk, &mut changes).unwrap();
        view.apply(&mut changes);
    }
    parser.finish(&mut changes).unwrap();
    view.apply(&mut changes);
    let block = view.blocks.values().next().unwrap();
    assert_eq!(block.status, BlockStatus::Incomplete);
    let BlockData::CodeBlock { body, .. } = block.data else {
        panic!("expected fence");
    };
    assert_eq!(parser.source(body), "let a = 5;");
    parser.finish(&mut changes).unwrap();
    assert!(changes.is_empty());
    assert_eq!(
        parser.append("later", &mut changes),
        Err(ParseError::Finished)
    );
}

#[test]
fn registered_tag_attributes_borrow_the_original_source() {
    let options = Options {
        custom_tags: vec!["tool-call".into()],
        ..Options::default()
    };
    let source = "<tool-call name=\"search\" title=\"a > b\" empty=\"\" />\n";
    let document = parse(source, &options).unwrap();
    let block = &document.nodes[0];
    let BlockData::CustomTag { tag, .. } = block.data else {
        panic!("expected custom element");
    };
    assert_eq!(tag.tag_name(source), "tool-call");
    assert_eq!(
        tag.attrs(source)
            .map(|attr| (attr.name, attr.value))
            .collect::<Vec<_>>(),
        [
            ("name", Some("search")),
            ("title", Some("a > b")),
            ("empty", Some(""))
        ]
    );
    assert_eq!(block.status, BlockStatus::Closed);
}
