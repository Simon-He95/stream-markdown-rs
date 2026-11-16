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
                        | BlockData::Footnote {
                            content: InlineContent::Plain(body),
                            ..
                        }
                        | BlockData::Heading {
                            content: InlineContent::Plain(body),
                            ..
                        }
                        | BlockData::Math {
                            body: stream_markdown_rs::Text::Source(body),
                        }
                        | BlockData::CodeBlock { body, .. }
                        | BlockData::CustomTag { body, .. } => body,
                        _ => panic!("unexpected text update: {block:?}"),
                    };
                    assert_eq!(span.start, body.end);
                    body.end = span.end;
                    block.span.end = span.end;
                }
                Change::ReplaceTableCell { id, index, content } => {
                    let block = self.blocks.get_mut(&id).unwrap();
                    let BlockData::Table(table) = &mut block.data else {
                        panic!("replaced a cell in non-table block {id:?}");
                    };
                    table.replace_cell(index, content);
                }
                Change::AddRow { id, end, cells } => {
                    let block = self.blocks.get_mut(&id).unwrap();
                    let BlockData::Table(table) = &mut block.data else {
                        panic!("unexpected row update: {block:?}");
                    };
                    table.add_row(cells);
                    block.span.end = end;
                }
                Change::SetStatus { id, status, end } => {
                    let block = self.blocks.get_mut(&id).unwrap();
                    block.status = status;
                    block.span.end = end;
                }
                Change::Preview { .. } | Change::PreviewInlines { .. } => {}
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
        change, Change::Replace(block) if block.id == id && matches!(block.data, BlockData::Table(_))
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

#[test]
fn late_reference_patches_only_affected_table_cells() {
    let options = Options::default();
    let table_source = "[head][id] | Keep\n--- | ---\n[body][id] | Stable\n\n";
    let definition = "[id]: /target\n\n";
    let mut parser = Parser::new(options.clone());
    let mut changes = Vec::new();
    let mut view = View::default();
    parser.append(table_source, &mut changes).unwrap();
    view.apply(&mut changes);
    let (&id, before) = view.blocks.first_key_value().unwrap();
    assert_eq!(before.status, BlockStatus::Closed);
    let BlockData::Table(table) = &before.data else {
        panic!("expected a table");
    };
    let untouched: Vec<_> = [
        table.header()[1].clone(),
        table.rows().next().unwrap()[1].clone(),
    ]
    .into();

    parser.append(definition, &mut changes).unwrap();
    assert_eq!(
        changes
            .iter()
            .map(|change| match change {
                Change::ReplaceTableCell {
                    id: changed,
                    index,
                    content: InlineContent::Parsed(nodes),
                } => {
                    assert_eq!(*changed, id);
                    assert!(matches!(nodes[0].kind, InlineKind::Link(_)));
                    *index
                }
                other => panic!("unexpected change: {other:?}"),
            })
            .collect::<Vec<_>>(),
        [0, 2]
    );
    assert_eq!(parser.metrics().invalidated_blocks, 1);
    view.apply(&mut changes);
    let BlockData::Table(table) = &view.blocks[&id].data else {
        panic!("expected a table");
    };
    assert_eq!(table.header()[1], untouched[0]);
    assert_eq!(table.rows().next().unwrap()[1], untouched[1]);

    let source = format!("{table_source}{definition}");
    let expected = parse(&source, &options).unwrap().nodes;
    assert_eq!(view.blocks.into_values().collect::<Vec<_>>(), expected);
}

#[test]
fn flat_table_preserves_padding_and_ignores_excess_cells() {
    let source = "| A | B |\n| --- | --- |\n| one |\n| two | three | ignored |\n";
    let document = parse(source, &Options::default()).unwrap();
    let BlockData::Table(table) = &document.nodes[0].data else {
        panic!("expected a table");
    };
    let labels = |row: &[InlineContent]| {
        row.iter()
            .map(|cell| match cell {
                InlineContent::Plain(span) => &source[span.start..span.end],
                other => panic!("expected plain cell: {other:?}"),
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(labels(table.header()), ["A", "B"]);
    assert_eq!(
        table.rows().map(labels).collect::<Vec<_>>(),
        [["one", ""], ["two", "three"]]
    );
}

#[test]
fn table_preview_is_withdrawn_when_a_separator_exceeds_the_header() {
    let mut parser = Parser::new(Options {
        preview: Preview::Ai,
        ..Options::default()
    });
    let mut changes = Vec::new();
    parser.append("A | B\n| -", &mut changes).unwrap();
    let Some(Change::Preview {
        id,
        hint: Some(PreviewHint::Table),
    }) = changes.last()
    else {
        panic!("expected a table preview");
    };
    let id = *id;
    changes.clear();
    parser.append(" | - | -", &mut changes).unwrap();
    assert_eq!(changes, [Change::Preview { id, hint: None }]);
    changes.clear();
    parser.append("----", &mut changes).unwrap();
    assert!(changes.is_empty());
}

#[test]
fn registered_opening_is_emitted_without_waiting_for_a_newline() {
    let options = Options {
        custom_tags: vec!["tool-call".into()],
        ..Options::default()
    };
    let mut parser = Parser::new(options.clone());
    let mut changes = Vec::new();
    parser
        .append("<tool-call name=\"search\" title=\"a >", &mut changes)
        .unwrap();
    assert!(changes.is_empty());
    parser.append(" b\">text", &mut changes).unwrap();
    let Change::Insert { block, .. } = &changes[0] else {
        panic!("expected the complete opening tag");
    };
    assert_eq!(block.status, BlockStatus::Open);
    let BlockData::CustomTag { tag, .. } = block.data else {
        panic!("expected custom element");
    };
    assert_eq!(tag.tag_name(parser.source_text()), "tool-call");
    assert_eq!(
        tag.attrs(parser.source_text()).last().unwrap().value,
        Some("a > b")
    );
    let mut view = View::default();
    view.apply(&mut changes);
    parser.append("</tool-call>", &mut changes).unwrap();
    view.apply(&mut changes);
    parser.finish(&mut changes).unwrap();
    view.apply(&mut changes);
    let expected = parse(parser.source_text(), &options).unwrap().nodes;
    assert_eq!(view.blocks.into_values().collect::<Vec<_>>(), expected);
}

#[test]
fn streaming_fence_does_not_publish_a_partial_indented_closer() {
    let mut parser = Parser::new(Options::default());
    let mut changes = Vec::new();
    let mut view = View::default();
    for chunk in [
        "````text\r\n",
        "one\r",
        "\ntwo",
        "\r\n  ",
        "``",
        "`` ",
        "\r",
        "\n",
    ] {
        parser.append(chunk, &mut changes).unwrap();
        view.apply(&mut changes);
    }
    let block = view.blocks.values().next().unwrap();
    let BlockData::CodeBlock { body, .. } = block.data else {
        panic!("expected a code block");
    };
    assert_eq!(parser.source(body), "one\r\ntwo\r\n");
    assert_eq!(block.status, BlockStatus::Closed);
    assert_eq!(parser.metrics().scanned_bytes, parser.source_text().len());
}

#[test]
fn heading_is_emitted_before_newline_and_closed_at_the_same_id() {
    let mut parser = Parser::new(Options::default());
    let mut changes = Vec::new();
    let mut view = View::default();
    parser.append("##", &mut changes).unwrap();
    assert!(changes.is_empty());
    parser.append(" Hel", &mut changes).unwrap();
    let [Change::Insert { block, .. }] = changes.as_slice() else {
        panic!("expected an open heading");
    };
    let id = block.id;
    assert_eq!(block.status, BlockStatus::Open);
    assert!(matches!(block.data, BlockData::Heading { level: 2, .. }));
    view.apply(&mut changes);
    parser.append("lo", &mut changes).unwrap();
    assert!(
        matches!(changes.as_slice(), [Change::AppendText { id: updated, .. }] if *updated == id)
    );
    view.apply(&mut changes);
    parser.append(" ##\n", &mut changes).unwrap();
    assert!(
        matches!(changes.as_slice(), [Change::Replace(block)] if block.id == id && block.status == BlockStatus::Closed)
    );
    view.apply(&mut changes);
    let BlockData::Heading {
        content: InlineContent::Plain(span),
        ..
    } = view.blocks[&id].data
    else {
        panic!("expected a plain heading");
    };
    assert_eq!(parser.source(span), "Hello");
}

#[test]
fn partial_table_header_is_visible_as_text_until_the_delimiter() {
    let options = Options {
        preview: Preview::Ai,
        ..Options::default()
    };
    let mut parser = Parser::new(options);
    let mut changes = Vec::new();
    let mut view = View::default();
    parser.append("| 中文 |", &mut changes).unwrap();
    let [Change::Insert { block, .. }] = changes.as_slice() else {
        panic!("expected a plain open paragraph");
    };
    let id = block.id;
    assert!(matches!(
        block.data,
        BlockData::Paragraph(InlineContent::Plain(_))
    ));
    view.apply(&mut changes);
    parser.append(" B |\n| - | - |\n", &mut changes).unwrap();
    view.apply(&mut changes);
    let BlockData::Table(table) = &view.blocks[&id].data else {
        panic!("expected the delimiter to confirm the table");
    };
    assert_eq!(table.header().len(), 2);
}

#[test]
fn display_math_is_opt_in_and_does_not_consume_footnote_labels() {
    let source = "$$\nx+1\n$$\n\n\\[y^2\\]\n\n[\n^1\n]\n\n[^1]";
    let default = parse(source, &Options::default()).unwrap();
    assert!(
        default
            .nodes
            .iter()
            .all(|block| !matches!(block.data, BlockData::Math { .. }))
    );
    let options = Options {
        math: true,
        ..Options::default()
    };
    let document = parse(source, &options).unwrap();
    let bodies = document
        .nodes
        .iter()
        .filter_map(|block| match &block.data {
            BlockData::Math { body } => Some(body.as_str(source)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(bodies, ["x+1", "y^2", "^1"]);
    assert!(matches!(
        document.nodes.last().unwrap().data,
        BlockData::Paragraph(_)
    ));
}

#[test]
fn display_math_streams_on_evidence_and_falls_back_at_end_of_input() {
    let options = Options {
        math: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    let mut parser = Parser::new(options.clone());
    let mut changes = Vec::new();
    let mut view = View::default();
    parser.append("$$\n", &mut changes).unwrap();
    assert!(changes.is_empty());
    parser.append("x + 1\n", &mut changes).unwrap();
    let [Change::Insert { block, .. }] = changes.as_slice() else {
        panic!("expected an open display block");
    };
    let id = block.id;
    assert_eq!(block.status, BlockStatus::Open);
    view.apply(&mut changes);
    parser.finish(&mut changes).unwrap();
    assert!(
        matches!(changes.as_slice(), [Change::Replace(block)] if block.id == id &&
        block.status == BlockStatus::Closed && matches!(block.data, BlockData::Paragraph(_)))
    );
    view.apply(&mut changes);
    let expected = parse(parser.source_text(), &options).unwrap().nodes;
    assert_eq!(view.blocks.into_values().collect::<Vec<_>>(), expected);
}

#[test]
fn confirmed_display_math_preserves_inline_markup_in_later_blocks() {
    let options = Options {
        math: true,
        ..Options::default()
    };
    let source = "before\n$$a$$ **after**\n";
    let document = parse(source, &options).unwrap();
    assert_eq!(document.nodes.len(), 3);
    assert!(matches!(document.nodes[1].data, BlockData::Math { .. }));
    assert!(matches!(
        document.nodes[2].data,
        BlockData::Paragraph(InlineContent::Parsed(_))
    ));
}

#[test]
fn display_math_handles_a_closing_marker_split_without_newline() {
    let options = Options {
        math: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    let mut parser = Parser::new(options.clone());
    let mut changes = Vec::new();
    let mut view = View::default();
    parser.append("$", &mut changes).unwrap();
    assert!(changes.is_empty());
    parser.append("$x+1$", &mut changes).unwrap();
    let [Change::Insert { block, .. }] = changes.as_slice() else {
        panic!("expected open math")
    };
    let id = block.id;
    let BlockData::Math { body } = &block.data else {
        panic!("expected open math")
    };
    assert_eq!(body.as_str(parser.source_text()), "x+1");
    view.apply(&mut changes);
    parser.append("$ trailing", &mut changes).unwrap();
    assert!(
        matches!(&changes[0], Change::Replace(block) if block.id == id && block.status == BlockStatus::Closed)
    );
    view.apply(&mut changes);
    parser.finish(&mut changes).unwrap();
    view.apply(&mut changes);
    let expected = parse(parser.source_text(), &options).unwrap().nodes;
    assert_eq!(view.blocks.into_values().collect::<Vec<_>>(), expected);
}

#[test]
fn inline_math_preview_is_temporary_and_bounded() {
    let options = Options {
        math: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    let mut parser = Parser::new(options);
    let mut changes = Vec::new();
    parser.append("result: $x+1", &mut changes).unwrap();
    assert!(changes.iter().any(
        |change| matches!(change, Change::PreviewInlines { nodes, .. }
        if nodes.iter().any(|node| matches!(node.kind, InlineKind::Math { loading: true, .. })))
    ));
    changes.clear();
    parser.append(&"x".repeat(1024), &mut changes).unwrap();
    assert!(
        matches!(changes.last(), Some(Change::PreviewInlines { nodes, .. }) if nodes.is_empty())
    );
    changes.clear();
    parser.finish(&mut changes).unwrap();
    assert!(
        changes
            .iter()
            .all(|change| !matches!(change, Change::PreviewInlines { .. }))
    );
}

#[test]
fn a_partial_unindented_header_can_follow_a_completed_list() {
    let options = Options {
        preview: Preview::Ai,
        ..Options::default()
    };
    let mut parser = Parser::new(options);
    let mut changes = Vec::new();
    parser
        .append("- 第一项\n  - nested 🙂\n- 第二项\n\n", &mut changes)
        .unwrap();
    changes.clear();
    parser.append("| 列 A |", &mut changes).unwrap();
    assert!(matches!(changes.last(),
        Some(Change::Insert { parent: None, block }) if block.status == BlockStatus::Open
            && matches!(block.data, BlockData::Paragraph(InlineContent::Plain(_)))));
}

#[test]
fn custom_tag_body_streams_nested_markup_until_its_own_closer() {
    let options = Options {
        custom_tags: vec!["thinking".into()],
        ..Options::default()
    };
    let mut parser = Parser::new(options);
    let mut changes = Vec::new();
    let mut view = View::default();
    parser
        .append("<thinking>分析 <em>中</e", &mut changes)
        .unwrap();
    view.apply(&mut changes);
    let id = *view.blocks.keys().next().unwrap();
    let BlockData::CustomTag { body, .. } = view.blocks[&id].data else {
        panic!("expected tag")
    };
    assert_eq!(parser.source(body), "分析 <em>中</e");

    parser.append("m></thi", &mut changes).unwrap();
    view.apply(&mut changes);
    let BlockData::CustomTag { body, .. } = view.blocks[&id].data else {
        panic!("expected tag")
    };
    assert_eq!(parser.source(body), "分析 <em>中</em>");
    parser.append("nking>\n", &mut changes).unwrap();
    view.apply(&mut changes);
    assert_eq!(view.blocks[&id].status, BlockStatus::Closed);
}
