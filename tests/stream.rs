use std::collections::BTreeMap;

use stream_markdown_rs::{
    Block, BlockData, BlockId, BlockStatus, Change, Inline, InlineContent, InlineKind, Options,
    ParseError, Parser, Preview, PreviewHint, Text, parse,
};

#[derive(Default)]
struct View {
    blocks: BTreeMap<BlockId, Block>,
}

impl View {
    fn apply(&mut self, changes: &mut Vec<Change>) {
        for change in changes.drain(..) {
            match change {
                Change::Insert { block, .. } => {
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
                        | BlockData::CodeBlock { body, .. } => body,
                        BlockData::CustomTag(element) => &mut element.body,
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
                Change::CloseInlineElement { id, start, end } => {
                    let block = self.blocks.get_mut(&id).unwrap();
                    let content = match &mut block.data {
                        BlockData::Paragraph(content) | BlockData::Heading { content, .. } => {
                            content
                        }
                        _ => panic!("invalid inline closer target: {id:?}"),
                    };
                    assert!(content.close_inline_element(start, end));
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
fn quote_preview_requires_body_and_keeps_its_block_and_leaf_ids() {
    let options = Options {
        preview: Preview::Ai,
        markstream: true,
        ..Options::default()
    };
    let mut parser = Parser::new(options.clone());
    let mut changes = Vec::new();
    let mut view = View::default();
    parser
        .append("Introduction\r\n\r\n>", &mut changes)
        .unwrap();
    assert!(changes.iter().all(|change| !matches!(
        change,
        Change::Insert { block, .. } if matches!(block.data, BlockData::Quote)
    )));
    view.apply(&mut changes);
    parser.append(" ", &mut changes).unwrap();
    assert!(changes.is_empty());
    let body = "ℹ️ We now build on top of [`markdown-it-ts`](https://www.npmjs.com/package/mar";
    parser.append(body, &mut changes).unwrap();
    let [
        Change::Insert {
            block: quote,
            parent: None,
        },
        Change::Insert {
            block: paragraph,
            parent: Some(parent),
        },
        Change::PreviewInlines { id, nodes },
    ] = changes.as_slice()
    else {
        panic!("expected a quote, its draft paragraph and inlines: {changes:?}");
    };
    assert!(matches!(quote.data, BlockData::Quote));
    assert_eq!(*parent, quote.id);
    assert_eq!(*id, paragraph.id);
    assert_eq!(parser.source(paragraph.span), body);
    assert!(
        nodes
            .iter()
            .any(|node| matches!(&node.kind, InlineKind::LinkPreview(_)))
    );
    let (quote_id, paragraph_id) = (quote.id, paragraph.id);
    view.apply(&mut changes);

    for chunk in ["kdown-it-ts)", "\r", "\n"] {
        parser.append(chunk, &mut changes).unwrap();
        assert!(
            changes
                .iter()
                .all(|change| !matches!(change, Change::Insert { .. }))
        );
        view.apply(&mut changes);
    }
    parser.finish(&mut changes).unwrap();
    view.apply(&mut changes);
    assert!(view.blocks.contains_key(&quote_id));
    assert!(view.blocks.contains_key(&paragraph_id));
    assert_eq!(
        view.blocks.into_values().collect::<Vec<_>>(),
        parse(parser.source_text(), &options).unwrap().nodes
    );
}

#[test]
fn quote_preview_does_not_preempt_ambiguous_or_active_containers() {
    for (before, prefix) in [
        ("", ">"),
        ("", "  >  "),
        ("", ">\tcode"),
        ("", ">     code"),
        ("", "> > nested"),
        ("", "> [!NOTE]"),
        ("", "> [^1]: note"),
        ("", "> :::warning"),
        ("", "> $$"),
        ("", "> \\["),
        ("- item\nlazy continuation\n", "> Next"),
        ("- item\n\n", "  > Nested after a list"),
        ("- [x] task\n\n", "> Following a task list"),
    ] {
        let mut parser = Parser::new(Options {
            preview: Preview::Ai,
            markstream: true,
            math: true,
            ..Options::default()
        });
        let mut changes = Vec::new();
        parser.append(before, &mut changes).unwrap();
        changes.clear();
        parser.append(prefix, &mut changes).unwrap();
        assert!(
            !changes.iter().any(|change| matches!(
                change,
                Change::Insert { block, .. } if matches!(block.data, BlockData::Quote)
            )),
            "{before:?} + {prefix:?}: {changes:?}"
        );
    }
}

#[test]
fn quote_preview_after_dormant_list_has_packet_final_parity() {
    let options = Options {
        preview: Preview::Ai,
        markstream: true,
        ..Options::default()
    };
    let mut parser = Parser::new(options.clone());
    let mut changes = Vec::new();
    parser.append("- item\r\n\r\n", &mut changes).unwrap();
    changes.clear();
    parser.append(">", &mut changes).unwrap();
    assert!(changes.is_empty());
    parser.append(" 文本", &mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(
        change,
        Change::Insert { block, parent: None } if matches!(block.data, BlockData::Quote)
    )));

    for source in [
        "- item\r\n\r\n> 文本 [link](https://example.org)\r\n\r\nAfter",
        "  > 中文 and `code`\r\n\r\nNext",
        "- item\nlazy continuation\n> Next",
        "> [!NOTE]\n> message",
        "> > nested quote",
    ] {
        let expected = parse(source, &options).unwrap().nodes;
        for (split, _) in source.char_indices() {
            let mut parser = Parser::new(options.clone());
            let mut view = View::default();
            let mut changes = Vec::new();
            for chunk in [&source[..split], &source[split..]] {
                parser.append(chunk, &mut changes).unwrap();
                view.apply(&mut changes);
            }
            parser.finish(&mut changes).unwrap();
            view.apply(&mut changes);
            assert_eq!(
                view.blocks.into_values().collect::<Vec<_>>(),
                expected,
                "{source:?} split {split}"
            );
        }
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
    let [
        Change::Preview {
            id: changed,
            hint: Some(PreviewHint::Table { table, .. }),
        },
    ] = changes.as_slice()
    else {
        panic!("expected a typed table preview");
    };
    assert_eq!(*changed, id);
    assert_eq!(table.header().len(), 2);
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
    let BlockData::CustomTag(element) = &block.data else {
        panic!("expected custom element");
    };
    assert_eq!(element.tag.tag_name(source), "tool-call");
    assert_eq!(
        element
            .tag
            .attrs(source)
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
        hint: Some(PreviewHint::Table { .. }),
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
    let BlockData::CustomTag(element) = &block.data else {
        panic!("expected custom element");
    };
    assert_eq!(element.tag.tag_name(parser.source_text()), "tool-call");
    assert_eq!(
        element
            .tag
            .attrs(parser.source_text())
            .last()
            .unwrap()
            .value,
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
fn multiline_registered_openers_keep_original_attributes_at_every_split() {
    let options = Options {
        custom_tags: vec!["tool-call".into()],
        ..Options::default()
    };
    for source in [
        "<tool-call\r\n  name='search'\r\n  title=\"甲 >\r\n乙\">body</tool-call>\r\n\r\n# Next",
        "<tool-call\n  name='search'\n  title=\"甲 >\n乙\" />",
    ] {
        let expected = parse(source, &options).unwrap().nodes;
        assert_eq!(
            expected.len(),
            if source.ends_with("# Next") { 2 } else { 1 }
        );
        let BlockData::CustomTag(element) = &expected[0].data else {
            panic!("expected a registered tag: {expected:?}");
        };
        let attributes: Vec<_> = element
            .tag
            .attrs(source)
            .map(|attr| (attr.name, attr.value))
            .collect();
        let title = if source.contains('\r') {
            "甲 >\r\n乙"
        } else {
            "甲 >\n乙"
        };
        assert_eq!(
            attributes,
            [("name", Some("search")), ("title", Some(title))]
        );
        assert_eq!(expected[0].status, BlockStatus::Closed);

        for split in source
            .char_indices()
            .map(|(offset, _)| offset)
            .chain([source.len()])
        {
            let mut parser = Parser::new(options.clone());
            let mut changes = Vec::new();
            let mut view = View::default();
            parser.append(&source[..split], &mut changes).unwrap();
            if split < source.find("乙\"").unwrap() + "乙\"".len() + 1 {
                assert!(
                    changes.is_empty(),
                    "published bare start at split {split}: {changes:?}"
                );
            }
            view.apply(&mut changes);
            parser.append(&source[split..], &mut changes).unwrap();
            view.apply(&mut changes);
            parser.finish(&mut changes).unwrap();
            view.apply(&mut changes);
            assert_eq!(
                view.blocks.into_values().collect::<Vec<_>>(),
                expected,
                "split {split}: {source:?}"
            );
            assert_eq!(parser.metrics().scanned_bytes, source.len());
        }
    }
}

#[test]
fn registered_tag_literals_do_not_close_the_body_in_batch_or_streaming() {
    for body in [
        "\r\n<!-- fake </tool-call>\r\n还在注释中 -->\r\nactual body\r\n",
        "<!-- fake </tool-call>\ncontinued -->actual body",
        "<!-- fake </tool-call> -->actual body",
        "\r\n<![CDATA[fake </tool-call>\r\n还在原文中 ]]>\r\nactual body\r\n",
        "<![CDATA[fake </tool-call>\ncontinued ]]>actual body",
        "<![CDATA[fake </tool-call> ]]>actual body",
    ] {
        let source = format!("<tool-call>{body}</tool-call>\n\n# Next");
        let options = Options {
            custom_tags: vec!["tool-call".into()],
            ..Options::default()
        };
        let expected = parse(&source, &options).unwrap().nodes;
        assert_eq!(expected.len(), 2, "{expected:?}");
        let BlockData::CustomTag(element) = &expected[0].data else {
            panic!("expected a registered block");
        };
        assert_eq!(&source[element.body.start..element.body.end], body);

        for split in source
            .char_indices()
            .map(|(offset, _)| offset)
            .chain([source.len()])
        {
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
        }
    }
}

#[test]
fn multiline_opening_attributes_do_not_make_a_same_line_custom_body_block_level() {
    let options = Options {
        markstream: true,
        preview: Preview::Ai,
        custom_tags: vec!["tool-call".into()],
        ..Options::default()
    };
    for source in [
        "<tool-call\r\n name=\"lookup\">正文</tool-call>后续内容",
        "<tool-call\r\n name=\"lookup\">正文</tool-call>后续内容\n",
    ] {
        let expected = parse(source, &options).unwrap().nodes;
        let BlockData::Paragraph(InlineContent::Parsed(nodes)) = &expected[0].data else {
            panic!("expected a paragraph with an inline custom tag");
        };
        assert!(matches!(&nodes[0].kind, InlineKind::CustomTag(element)
            if element.tag.tag_name(source) == "tool-call"
                && element.tag.attrs(source).next().unwrap().value == Some("lookup")));
        assert!(
            matches!(&nodes[1].kind, InlineKind::Text(text) if text.as_str(source) == "后续内容")
        );

        for split in source
            .char_indices()
            .map(|(offset, _)| offset)
            .chain([source.len()])
        {
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
        }
    }
}

#[test]
fn segmented_html_publishes_only_completed_source_lines() {
    let mut parser = Parser::new(Options {
        markstream: true,
        preview: Preview::Ai,
        ..Options::default()
    });
    let mut changes = Vec::new();
    parser.append("<details>\n<div>\n", &mut changes).unwrap();
    let id = changes
        .iter()
        .find_map(|change| match change {
            Change::Insert { block, .. } if matches!(block.data, BlockData::SegmentedHtml(_)) => {
                Some(block.id)
            }
            _ => None,
        })
        .expect("expected raw HTML under details");
    changes.clear();
    parser.append("一行", &mut changes).unwrap();
    assert!(!changes.iter().any(|change| matches!(change,
        Change::AppendText { id: updated, .. } if *updated == id)));
    changes.clear();
    parser.append("\n", &mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(change,
        Change::AddRawLine { id: updated, line, .. } if *updated == id && parser.source(line.span) == "一行\n")));
}

#[test]
fn split_crlf_blank_line_does_not_close_a_list_early() {
    let mut parser = Parser::new(Options::default());
    let mut changes = Vec::new();
    parser.append("- item\n\n", &mut changes).unwrap();
    let id = changes
        .iter()
        .find_map(|change| match change {
            Change::Insert {
                block,
                parent: None,
            } if matches!(block.data, BlockData::List { .. }) => Some(block.id),
            _ => None,
        })
        .expect("expected an open list");
    changes.clear();
    parser.append("\r", &mut changes).unwrap();
    assert!(!changes.iter().any(|change| matches!(change,
        Change::Replace(block) if block.id == id && block.status == BlockStatus::Closed)));
    parser.append("\nnext\n", &mut changes).unwrap();
    parser.finish(&mut changes).unwrap();
    let span = parse(parser.source_text(), &Options::default())
        .unwrap()
        .nodes[0]
        .span;
    assert!(changes.iter().any(|change| matches!(change,
        Change::Replace(block) if block.id == id && block.status == BlockStatus::Closed && block.span == span)));
}

#[test]
fn rejected_or_incomplete_multiline_openers_replay_the_buffer_once() {
    let options = Options {
        custom_tags: vec!["tool-call".into()],
        ..Options::default()
    };
    let prefix = "<tool-call\n  title='unfinished\n\n# Heading\n";
    for tail in ["", "' invalid==value>\n"] {
        let mut parser = Parser::new(options.clone());
        let mut changes = Vec::new();
        parser.append(prefix, &mut changes).unwrap();
        assert!(changes.is_empty());
        parser.append(tail, &mut changes).unwrap();
        parser.finish(&mut changes).unwrap();
        let source = parser.source_text();
        let expected = parse(source, &Options::default()).unwrap().nodes;
        let mut view = View::default();
        view.apply(&mut changes);
        assert_eq!(view.blocks.into_values().collect::<Vec<_>>(), expected);
        assert!(
            expected
                .iter()
                .any(|node| matches!(node.data, BlockData::Heading { .. }))
        );
        assert!(parser.metrics().scanned_bytes <= 2 * source.len());
    }
}

#[test]
fn late_definitions_backpatch_custom_children_without_rebuilding_unrelated_blocks() {
    let options = Options {
        custom_tags: vec!["tool-call".into()],
        footnotes: true,
        ..Options::default()
    };
    let mut parser = Parser::new(options.clone());
    let mut changes = Vec::new();
    let mut view = View::default();
    parser
        .append(
            "<tool-call>\n[label][ref] and [^note]\n</tool-call>\n\nStable\n\n",
            &mut changes,
        )
        .unwrap();
    view.apply(&mut changes);
    let id = view
        .blocks
        .values()
        .find(|block| matches!(block.data, BlockData::CustomTag(_)))
        .unwrap()
        .id;
    let stable = view
        .blocks
        .values()
        .find(|block| matches!(block.data, BlockData::Paragraph(_)))
        .unwrap()
        .clone();

    for definition in ["[ref]: /destination\n\n", "[^note]: note body\n\n"] {
        parser.append(definition, &mut changes).unwrap();
        assert!(
            changes
                .iter()
                .any(|change| matches!(change, Change::Replace(block) if block.id == id))
        );
        assert!(
            !changes
                .iter()
                .any(|change| matches!(change, Change::Replace(block) if block.id == stable.id))
        );
        view.apply(&mut changes);
    }
    parser.finish(&mut changes).unwrap();
    view.apply(&mut changes);
    let expected = parse(parser.source_text(), &options).unwrap().nodes;
    assert_eq!(view.blocks.into_values().collect::<Vec<_>>(), expected);
    assert_eq!(parser.metrics().invalidated_blocks, 2);
}

#[test]
fn rejected_multiline_opener_does_not_consume_the_next_registered_start() {
    let options = Options {
        custom_tags: vec!["tool-call".into()],
        ..Options::default()
    };
    let source =
        "<tool-call malformed==\n<tool-call\r\n  name=\"lookup\">内容</tool-call>\n\n# Next";
    let expected = parse(source, &options).unwrap().nodes;
    assert_eq!(expected.len(), 3);
    let BlockData::CustomTag(element) = &expected[1].data else {
        panic!("expected the second opener to start a typed block: {expected:?}");
    };
    assert_eq!(
        element.tag.attrs(source).next().unwrap().value,
        Some("lookup")
    );

    for split in source
        .char_indices()
        .map(|(offset, _)| offset)
        .chain([source.len()])
    {
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
        assert!(parser.metrics().scanned_bytes <= 2 * source.len());
    }
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
    let BlockData::CustomTag(element) = &view.blocks[&id].data else {
        panic!("expected tag")
    };
    assert_eq!(parser.source(element.body), "分析 <em>中</e");

    parser.append("m></thi", &mut changes).unwrap();
    view.apply(&mut changes);
    let BlockData::CustomTag(element) = &view.blocks[&id].data else {
        panic!("expected tag")
    };
    assert_eq!(parser.source(element.body), "分析 <em>中</em>");
    parser.append("nking>\n", &mut changes).unwrap();
    view.apply(&mut changes);
    assert_eq!(view.blocks[&id].status, BlockStatus::Closed);
}

#[test]
fn a_custom_tag_closes_in_the_same_chunk_without_waiting_for_newline() {
    let options = Options {
        custom_tags: vec!["thinking".into()],
        ..Options::default()
    };
    let mut parser = Parser::new(options.clone());
    let mut view = View::default();
    let mut changes = Vec::new();
    for chunk in [
        "<thinking>before <think",
        "ing title=\"literal </thinking>",
        "\">nested</think",
        "ing> remaining</think",
    ] {
        parser.append(chunk, &mut changes).unwrap();
        view.apply(&mut changes);
        assert!(
            view.blocks
                .values()
                .all(|block| block.status == BlockStatus::Open)
        );
    }
    parser.append("ing> trailing", &mut changes).unwrap();
    view.apply(&mut changes);
    let root = view.blocks.values().next().unwrap();
    assert_eq!(root.status, BlockStatus::Closed);
    let BlockData::CustomTag(element) = &root.data else {
        panic!("expected a tag")
    };
    assert_eq!(
        parser.source(element.body),
        "before <thinking title=\"literal </thinking>\">nested</thinking> remaining"
    );
    parser.finish(&mut changes).unwrap();
    view.apply(&mut changes);
    let full = parse(parser.source_text(), &options).unwrap();
    assert_eq!(view.blocks.into_values().collect::<Vec<_>>(), full.nodes);
}

#[test]
fn finished_session_releases_plugins_and_retains_source_metrics() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use stream_markdown_rs::{PluginError, PluginRule, Rule, RuleInput, RuleResult, Scope};

    struct DropRule(Arc<AtomicUsize>);

    impl PluginRule for DropRule {
        fn rule(&self) -> Rule {
            Rule {
                trigger: b'@',
                scope: Scope::Block,
                kind: 1,
            }
        }

        fn step(&mut self, _: RuleInput<'_>) -> Result<RuleResult, PluginError> {
            Ok(RuleResult::Reject)
        }
    }

    impl Drop for DropRule {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    let released = Arc::new(AtomicUsize::new(0));
    let mut parser = Parser::new(Options::default());
    parser.add_plugin(DropRule(released.clone())).unwrap();
    let mut changes = Vec::new();
    parser.append("text", &mut changes).unwrap();
    assert_eq!(released.load(Ordering::Relaxed), 0);
    parser.finish(&mut changes).unwrap();
    assert_eq!(released.load(Ordering::Relaxed), 1);
    assert_eq!(parser.source_text(), "text");
    assert_eq!(parser.metrics().scanned_bytes, 4);
    parser.finish(&mut changes).unwrap();
    assert_eq!(released.load(Ordering::Relaxed), 1);
}

#[test]
fn a_fence_releases_a_partial_line_when_it_cannot_close_the_block() {
    let mut parser = Parser::new(Options::default());
    let mut changes = Vec::new();
    let mut view = View::default();
    parser.append("```txt\n", &mut changes).unwrap();
    view.apply(&mut changes);
    let id = *view.blocks.keys().next().unwrap();
    parser.append("`", &mut changes).unwrap();
    assert!(changes.is_empty());
    parser.append("value", &mut changes).unwrap();
    assert!(
        matches!(changes.as_slice(), [Change::AppendText { id: changed, span }] if *changed == id && parser.source(*span) == "`value")
    );
    view.apply(&mut changes);
    parser.append("\n    ```", &mut changes).unwrap();
    view.apply(&mut changes);
    assert_eq!(view.blocks[&id].status, BlockStatus::Open);
    let BlockData::CodeBlock { body, .. } = view.blocks[&id].data else {
        panic!("expected code")
    };
    assert_eq!(parser.source(body), "`value\n    ```");
}

#[test]
fn markstream_projects_a_root_inline_prefix_once_it_cannot_be_a_block_marker() {
    let options = Options {
        markstream: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    let mut parser = Parser::new(options);
    let mut changes = Vec::new();
    parser.append("**", &mut changes).unwrap();
    assert!(changes.is_empty());
    parser
        .append("[百度](https://baidu.com/)提", &mut changes)
        .unwrap();
    assert!(
        matches!(changes.first(), Some(Change::Insert { block, .. }) if matches!(block.data, BlockData::Paragraph(_)))
    );
    assert!(
        matches!(changes.last(), Some(Change::PreviewInlines { nodes, .. }) if
        nodes.iter().any(|node| matches!(&node.kind, InlineKind::Strong(children) if children.iter().any(|child| matches!(child.kind, InlineKind::Link(_))))))
    );
}

#[test]
fn an_incomplete_table_row_is_previewed_then_committed_once() {
    let mut parser = Parser::new(Options {
        preview: Preview::Ai,
        ..Options::default()
    });
    let mut changes = Vec::new();
    parser
        .append("| A | B |\n| --- | --- |\n", &mut changes)
        .unwrap();
    changes.clear();
    parser.append("| ", &mut changes).unwrap();
    assert!(
        matches!(changes.as_slice(), [Change::Preview { hint: Some(PreviewHint::TableRow(cells)), .. }]
        if cells.len() == 2 && cells.iter().all(|cell| matches!(cell, InlineContent::Plain(span) if parser.source(*span).is_empty())))
    );
    changes.clear();
    parser.append("1 | ", &mut changes).unwrap();
    let [
        Change::Preview {
            id,
            hint: Some(PreviewHint::TableRow(cells)),
        },
    ] = changes.as_slice()
    else {
        panic!("expected a provisional row")
    };
    let id = *id;
    let [InlineContent::Plain(first), InlineContent::Plain(second)] = cells.as_slice() else {
        panic!("expected two cells")
    };
    assert_eq!(parser.source(*first), "1");
    assert_eq!(parser.source(*second), "");
    changes.clear();
    parser.append("2 |\n", &mut changes).unwrap();
    assert!(matches!(changes.as_slice(),
        [Change::Preview { id: cleared, hint: None }, Change::AddRow { id: committed, .. }] if *cleared == id && *committed == id));
}

#[test]
fn markstream_previews_inline_code_in_an_incomplete_table_row() {
    let mut parser = Parser::new(Options {
        markstream: true,
        preview: Preview::Ai,
        ..Options::default()
    });
    let mut changes = Vec::new();
    parser
        .append("| A | B |\n| --- | --- |\n", &mut changes)
        .unwrap();
    changes.clear();
    parser.append("| `a\\|b", &mut changes).unwrap();
    let [
        Change::Preview {
            id,
            hint: Some(PreviewHint::TableRow(cells)),
        },
    ] = changes.as_slice()
    else {
        panic!("expected a provisional row")
    };
    let id = *id;
    assert!(matches!(&cells[0], InlineContent::Parsed(nodes)
        if matches!(nodes.as_slice(), [Inline { kind: InlineKind::CodePreview(Text::Decoded(text)), .. }] if text.as_ref() == "a|b")));
    changes.clear();
    parser.append("` | done |\n", &mut changes).unwrap();
    assert!(
        matches!(changes.as_slice(), [Change::Preview { id: cleared, hint: None }, Change::AddRow { id: committed, cells, .. }]
        if *cleared == id && *committed == id && matches!(&cells[0], InlineContent::Parsed(nodes)
            if matches!(nodes.as_slice(), [Inline { kind: InlineKind::Code(Text::Decoded(text)), .. }] if text.as_ref() == "a|b")))
    );
}

#[test]
fn a_partial_ordered_item_keeps_its_list_and_inline_ids_on_line_completion() {
    let options = Options {
        markstream: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    let mut parser = Parser::new(options.clone());
    let mut changes = Vec::new();
    parser.append("1.  ", &mut changes).unwrap();
    assert!(changes.is_empty());
    parser.append("**标题** `one`", &mut changes).unwrap();
    let inserts = changes
        .iter()
        .filter_map(|change| match change {
            Change::Insert { parent, block } => Some((*parent, block)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [
        (None, list),
        (Some(item_parent), item),
        (Some(body_parent), body),
    ] = inserts.as_slice()
    else {
        panic!("expected an open list, item and paragraph")
    };
    assert_eq!(*item_parent, list.id);
    assert_eq!(*body_parent, item.id);
    let body_id = body.id;
    assert!(changes.iter().any(
        |change| matches!(change, Change::PreviewInlines { id, nodes }
        if *id == body_id && matches!(nodes[0].kind, InlineKind::Strong(_)))
    ));
    changes.clear();
    parser.append("\n", &mut changes).unwrap();
    assert!(
        changes
            .iter()
            .all(|change| !matches!(change, Change::Insert { .. }))
    );
    parser.finish(&mut changes).unwrap();
    let expected = parse(parser.source_text(), &options).unwrap();
    assert!(
        changes
            .iter()
            .any(|change| matches!(change, Change::Replace(block) if *block == expected.nodes[2]))
    );
}

#[test]
fn indented_list_continuation_is_visible_before_its_newline() {
    let options = Options {
        markstream: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    let mut parser = Parser::new(options);
    let mut changes = Vec::new();
    parser.append("- **标题**\n   前缀", &mut changes).unwrap();
    let Some(Change::PreviewInlines { nodes, .. }) = changes.last() else {
        panic!("expected a preview spanning the indented continuation");
    };
    assert!(
        nodes
            .iter()
            .any(|node| matches!(node.kind, InlineKind::Strong(_)))
    );
    assert!(nodes.iter().any(|node| matches!(&node.kind,
        InlineKind::Text(text) if text.as_str(parser.source_text()) == "前缀")));
    changes.clear();
    parser.append("继续\n", &mut changes).unwrap();
    assert!(
        changes
            .iter()
            .any(|change| matches!(change, Change::AddTextFragment { .. }))
    );
}

#[test]
fn markstream_html_drafts_keep_source_and_ids_at_same_line_closure() {
    let options = Options {
        markstream: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    let mut parser = Parser::new(options.clone());
    let mut changes = Vec::new();
    let mut view = View::default();
    parser.append("<div", &mut changes).unwrap();
    assert!(changes.is_empty());
    parser.append(">hello", &mut changes).unwrap();
    view.apply(&mut changes);
    let root = view.blocks.values().next().unwrap();
    let id = root.id;
    assert_eq!(root.status, BlockStatus::Open);
    assert!(matches!(root.data, BlockData::Html));
    parser.append("</div>", &mut changes).unwrap();
    view.apply(&mut changes);
    let root = &view.blocks[&id];
    assert_eq!(root.status, BlockStatus::Closed);
    assert_eq!(parser.source(root.span), "<div>hello</div>");
    parser.finish(&mut changes).unwrap();
    view.apply(&mut changes);
    assert_eq!(
        view.blocks.into_values().collect::<Vec<_>>(),
        parse(parser.source_text(), &options).unwrap().nodes
    );
}

#[test]
fn registered_inline_custom_tags_keep_raw_payload_and_parsed_children() {
    let source = "before <custom_data title=\"x>y\">{\"title\":\"**Report**\"}</custom_data> after";
    let options = Options {
        custom_tags: vec!["custom_data".into()],
        markstream: true,
        ..Options::default()
    };
    let document = parse(source, &options).unwrap();
    let BlockData::Paragraph(InlineContent::Parsed(nodes)) = &document.nodes[0].data else {
        panic!("expected parsed paragraph");
    };
    let InlineKind::CustomTag(element) = &nodes[1].kind else {
        panic!("expected registered inline tag");
    };
    assert_eq!(element.tag.tag_name(source), "custom_data");
    assert_eq!(element.tag.attrs(source).next().unwrap().value, Some("x>y"));
    assert_eq!(
        &source[element.body.start..element.body.end],
        "{\"title\":\"**Report**\"}"
    );
    assert!(
        element
            .children
            .iter()
            .any(|node| matches!(node.kind, InlineKind::Strong(_)))
    );
}

#[test]
fn a_markstream_break_promotes_the_following_nonempty_custom_element() {
    let options = Options {
        custom_tags: vec!["usrhead".into(), "aihead".into(), "thinking".into()],
        markstream: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    let first = "<usrhead></usrhead><br>前缀<aihead></aihead><thinking>分析";
    let rest = "\n\n### 推理\n</thinking>";
    let source = format!("{first}{rest}");
    let full = parse(&source, &options).unwrap();
    assert_eq!(full.nodes.len(), 2);
    let BlockData::Paragraph(InlineContent::Parsed(prefix)) = &full.nodes[0].data else {
        panic!("expected the preceding inline paragraph");
    };
    assert!(
        prefix
            .iter()
            .any(|node| matches!(node.kind, InlineKind::HardBreak))
    );
    let BlockData::CustomTag(element) = &full.nodes[1].data else {
        panic!("expected one promoted custom element");
    };
    assert!(element.promoted);
    assert_eq!(
        &source[element.body.start..element.body.end],
        "分析\n\n### 推理\n"
    );

    let mut parser = Parser::new(options);
    let mut changes = Vec::new();
    let mut view = View::default();
    parser.append(first, &mut changes).unwrap();
    view.apply(&mut changes);
    assert_eq!(view.blocks.len(), 2);
    assert_eq!(
        view.blocks.values().last().unwrap().status,
        BlockStatus::Open
    );
    parser.append(rest, &mut changes).unwrap();
    parser.finish(&mut changes).unwrap();
    view.apply(&mut changes);
    assert_eq!(view.blocks.into_values().collect::<Vec<_>>(), full.nodes);
}

#[test]
fn a_partial_reference_definition_previews_only_waiting_text() {
    let options = Options {
        markstream: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    let mut parser = Parser::new(options);
    let mut view = View::default();
    let mut changes = Vec::new();
    parser
        .append("请看 [1][source]\n\nUnaffected\n\n", &mut changes)
        .unwrap();
    view.apply(&mut changes);
    let id = *view.blocks.keys().next().unwrap();
    parser
        .append("[source]: https://example.com", &mut changes)
        .unwrap();
    assert!(
        matches!(changes.as_slice(), [Change::PreviewInlines { id: updated, nodes }]
        if *updated == id && nodes.iter().any(|node| matches!(node.kind, InlineKind::Link(_))))
    );
    assert_eq!(parser.metrics().invalidated_blocks, 0);
    changes.clear();
    parser.append("\n\n", &mut changes).unwrap();
    assert_eq!(parser.metrics().invalidated_blocks, 1);
    assert!(
        changes
            .iter()
            .any(|change| matches!(change, Change::Replace(block) if block.id == id))
    );
}

#[test]
fn a_bracket_prefix_is_literal_until_the_reference_definition_has_a_destination() {
    let options = Options {
        markstream: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    for ending in [" /target", " /target\n\n"] {
        let mut parser = Parser::new(options.clone());
        let mut changes = Vec::new();
        let mut view = View::default();
        parser.append("[read][来源]\n\n[来", &mut changes).unwrap();
        let Some(Change::Insert {
            block,
            parent: None,
        }) = changes.last()
        else {
            panic!("expected the literal bracket prefix: {changes:?}");
        };
        let id = block.id;
        assert_eq!(block.status, BlockStatus::Open);
        assert!(
            matches!(block.data, BlockData::Paragraph(InlineContent::Plain(span))
            if parser.source(span) == "[来")
        );
        view.apply(&mut changes);

        parser.append("源]:", &mut changes).unwrap();
        view.apply(&mut changes);
        assert_eq!(parser.source(view.blocks[&id].span), "[来源]:");
        parser.append(ending, &mut changes).unwrap();
        assert!(changes.contains(&Change::Remove { id }));
        view.apply(&mut changes);
        parser.finish(&mut changes).unwrap();
        view.apply(&mut changes);

        assert_eq!(
            view.blocks.into_values().collect::<Vec<_>>(),
            parse(parser.source_text(), &options).unwrap().nodes
        );
    }
}

#[test]
fn a_numeric_prefix_is_visible_once_it_cannot_be_an_ordered_list_marker() {
    let options = Options {
        markstream: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    let mut parser = Parser::new(options.clone());
    let mut changes = Vec::new();
    let mut view = View::default();
    parser.append("- earlier\n\n", &mut changes).unwrap();
    view.apply(&mut changes);
    for chunk in ["2", "."] {
        parser.append(chunk, &mut changes).unwrap();
        assert!(changes.is_empty());
    }
    parser.append("0 ships", &mut changes).unwrap();
    let Some(Change::Insert {
        parent: None,
        block,
    }) = changes.last()
    else {
        panic!("expected a root paragraph: {changes:?}");
    };
    assert_eq!(block.status, BlockStatus::Open);
    assert!(
        matches!(block.data, BlockData::Paragraph(InlineContent::Plain(span))
        if parser.source(span) == "2.0 ships")
    );
    view.apply(&mut changes);
    parser.finish(&mut changes).unwrap();
    view.apply(&mut changes);
    assert_eq!(
        view.blocks.into_values().collect::<Vec<_>>(),
        parse(parser.source_text(), &options).unwrap().nodes
    );

    let mut parser = Parser::new(options);
    parser.append("2.", &mut changes).unwrap();
    assert!(changes.is_empty());
    parser.append(" item", &mut changes).unwrap();
    assert!(
        matches!(changes.first(), Some(Change::Insert { block, parent: None })
        if matches!(block.data, BlockData::List { ordered: true, start: 2, .. }))
    );
}

#[test]
fn a_heading_or_inline_starter_after_a_list_is_previewed_at_the_root() {
    let options = Options {
        markstream: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    let prefix = "- earlier\n\n";
    for (starter, content) in [
        ("## ", "Heading"),
        ("`", "code` draft"),
        ("**", "strong** draft"),
    ] {
        let mut parser = Parser::new(options.clone());
        let mut changes = Vec::new();
        let mut view = View::default();
        parser.append(prefix, &mut changes).unwrap();
        view.apply(&mut changes);
        parser.append(starter, &mut changes).unwrap();
        assert!(
            !changes
                .iter()
                .any(|change| matches!(change, Change::Insert { .. }))
        );
        view.apply(&mut changes);
        parser.append(content, &mut changes).unwrap();
        let block = changes
            .iter()
            .find_map(|change| match change {
                Change::Insert {
                    parent: None,
                    block,
                } => Some(block),
                _ => None,
            })
            .unwrap_or_else(|| panic!("expected a root draft for {starter}{content}: {changes:?}"));
        let id = block.id;
        assert_eq!(block.span.start, prefix.len());
        assert_eq!(block.status, BlockStatus::Open);
        assert_eq!(
            matches!(block.data, BlockData::Heading { .. }),
            starter.starts_with('#')
        );
        view.apply(&mut changes);
        parser.append("\n\n", &mut changes).unwrap();
        view.apply(&mut changes);
        parser.finish(&mut changes).unwrap();
        view.apply(&mut changes);
        let expected = parse(parser.source_text(), &options).unwrap();
        assert_eq!(expected.parent(id), None);
        assert_eq!(
            view.blocks.into_values().collect::<Vec<_>>(),
            expected.nodes
        );
    }
}

#[test]
fn markstream_fence_previews_the_info_and_keeps_the_id_at_the_newline() {
    let options = Options {
        markstream: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    let mut parser = Parser::new(options);
    let mut changes = Vec::new();
    parser.append("~~~dif", &mut changes).unwrap();
    let [Change::Insert { block, .. }] = changes.as_slice() else {
        panic!("expected preview")
    };
    let id = block.id;
    let BlockData::CodeBlock { body, .. } = block.data else {
        panic!("expected a fence")
    };
    assert_eq!(parser.source(block.data.code_info().unwrap()), "dif");
    assert_eq!(parser.source(body), "");
    changes.clear();
    parser.append("f\n+x\n", &mut changes).unwrap();
    assert!(matches!(changes.as_slice(),
        [Change::Replace(block), Change::AppendText { id: updated, .. }] if block.id == id && *updated == id));
}

#[test]
fn an_outdented_fence_after_a_list_is_previewed_at_the_root() {
    let options = Options {
        markstream: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    for suffix in ["s\nbody\n```\n", "` invalid\n"] {
        let mut parser = Parser::new(options.clone());
        let mut changes = Vec::new();
        let mut view = View::default();
        parser
            .append("1. first\n2. second\n\n", &mut changes)
            .unwrap();
        view.apply(&mut changes);
        parser.append("```j", &mut changes).unwrap();
        let Some(Change::Insert {
            block,
            parent: None,
        }) = changes.last()
        else {
            panic!("expected the outdented fence at the root: {changes:?}");
        };
        let id = block.id;
        assert_eq!(block.status, BlockStatus::Open);
        assert_eq!(parser.source(block.data.code_info().unwrap()), "j");
        view.apply(&mut changes);
        parser.append(suffix, &mut changes).unwrap();
        view.apply(&mut changes);
        parser.finish(&mut changes).unwrap();
        view.apply(&mut changes);
        let expected = parse(parser.source_text(), &options).unwrap();
        assert_eq!(expected.parent(id), None);
        assert_eq!(&view.blocks[&id], expected.nodes.last().unwrap());
        assert_eq!(
            view.blocks.into_values().collect::<Vec<_>>(),
            expected.nodes
        );
    }
}

#[test]
fn invalid_backtick_info_changes_a_fence_preview_into_the_same_paragraph() {
    let options = Options {
        markstream: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    let mut parser = Parser::new(options.clone());
    let mut changes = Vec::new();
    let mut view = View::default();
    parser.append("```ts", &mut changes).unwrap();
    view.apply(&mut changes);
    let id = *view.blocks.keys().next().unwrap();
    parser.append(" `", &mut changes).unwrap();
    view.apply(&mut changes);
    assert!(matches!(view.blocks[&id].data, BlockData::Paragraph(_)));
    parser.finish(&mut changes).unwrap();
    view.apply(&mut changes);
    assert_eq!(
        view.blocks.into_values().collect::<Vec<_>>(),
        parse(parser.source_text(), &options).unwrap().nodes
    );
}

#[test]
fn locally_closed_paragraph_keeps_pending_math_until_document_finish() {
    let options = Options {
        markstream: true,
        math: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    let mut parser = Parser::new(options);
    let mut changes = Vec::new();
    parser.append("before $x +\n\nnext", &mut changes).unwrap();
    let id = changes
        .iter()
        .find_map(|change| match change {
            Change::Insert { block, .. } | Change::Replace(block)
                if block.status == BlockStatus::Closed =>
            {
                Some(block.id)
            }
            _ => None,
        })
        .expect("the first paragraph must close");
    assert!(changes.iter().any(|change| matches!(change, Change::PreviewInlines { id: changed, nodes }
        if *changed == id && nodes.iter().any(|node| matches!(node.kind, InlineKind::Math { loading: true, .. })))));
    changes.clear();
    parser.finish(&mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(change,
        Change::PreviewInlines { id: changed, nodes } if *changed == id && nodes.is_empty())));
}

#[test]
fn resolved_reference_also_removes_its_old_pending_math_overlay() {
    let mut parser = Parser::new(Options {
        markstream: true,
        math: true,
        preview: Preview::Ai,
        ..Options::default()
    });
    let mut changes = Vec::new();
    parser
        .append("Read [link][key $x +]\n\n", &mut changes)
        .unwrap();
    let id = changes
        .iter()
        .find_map(|change| match change {
            Change::PreviewInlines { id, nodes }
                if nodes
                    .iter()
                    .any(|node| matches!(node.kind, InlineKind::Math { loading: true, .. })) =>
            {
                Some(*id)
            }
            _ => None,
        })
        .expect("the unresolved reference label has a pending math preview");
    changes.clear();
    parser
        .append("[key $x +]: /target\n\n", &mut changes)
        .unwrap();
    assert!(changes.iter().any(
        |change| matches!(change, Change::Replace(block) if block.id == id
        && matches!(&block.data, BlockData::Paragraph(InlineContent::Parsed(nodes))
            if nodes.iter().any(|node| matches!(node.kind, InlineKind::Link(_)))))
    ));
    changes.clear();
    parser.finish(&mut changes).unwrap();
    assert!(
        changes.is_empty(),
        "the replacement already cleared the preview: {changes:?}"
    );
}

#[test]
fn append_coalesces_only_insertions_from_its_own_batch() {
    let mut parser = Parser::new(Options::default());
    let mut changes = Vec::new();
    parser.append("first\n\nsecond\n\n", &mut changes).unwrap();
    assert!(matches!(changes.as_slice(),
        [Change::Insert { block: first, .. }, Change::Insert { block: second, .. }]
        if first.status == BlockStatus::Closed && second.status == BlockStatus::Closed));
    changes.clear();
    parser.append("third", &mut changes).unwrap();
    let [Change::Insert { block, .. }] = changes.as_slice() else {
        panic!("expected the open paragraph")
    };
    let id = block.id;
    parser.append("\n\n", &mut changes).unwrap();
    assert!(matches!(changes.as_slice(),
        [Change::Insert { block, .. }, Change::Replace(closed)]
        if block.id == id && block.status == BlockStatus::Open && closed.id == id && closed.status == BlockStatus::Closed));
}

#[test]
fn a_partial_sibling_item_reuses_the_list_and_publishes_its_first_paragraph() {
    let options = Options {
        markstream: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    let mut parser = Parser::new(options);
    let mut changes = Vec::new();
    parser
        .append("  1. first\n  2. second\n", &mut changes)
        .unwrap();
    let id = changes
        .iter()
        .find_map(|change| match change {
            Change::Insert { block, .. } if matches!(block.data, BlockData::List { .. }) => {
                Some(block.id)
            }
            _ => None,
        })
        .expect("a list was emitted");
    changes.clear();
    parser.append("  3. thi", &mut changes).unwrap();
    let item = changes
        .iter()
        .find_map(|change| match change {
            Change::Insert {
                parent: Some(parent),
                block,
            } if *parent == id && matches!(block.data, BlockData::ListItem { .. }) => {
                Some(block.id)
            }
            _ => None,
        })
        .expect("the sibling item reused the existing list");
    assert!(changes.iter().any(|change| matches!(change,
        Change::Insert { parent: Some(parent), block } if *parent == item
            && matches!(block.data, BlockData::Paragraph(InlineContent::Plain(span)) if parser.source(span) == "thi"))));
    changes.clear();
    parser.append("rd\n", &mut changes).unwrap();
    assert!(!changes.iter().any(|change| matches!(change,
        Change::Insert { block, .. } if matches!(block.data, BlockData::ListItem { .. }))));
}

#[test]
fn bracket_led_list_text_is_previewed_after_the_task_prefix_is_ruled_out() {
    let options = Options {
        markstream: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    let mut parser = Parser::new(options.clone());
    let mut changes = Vec::new();
    let mut view = View::default();
    parser.append("- First\n- [", &mut changes).unwrap();
    view.apply(&mut changes);
    parser.append("Usage", &mut changes).unwrap();
    let item = changes
        .iter()
        .find_map(|change| match change {
            Change::Insert { block, .. }
                if matches!(block.data, BlockData::ListItem { task: None }) =>
            {
                Some(block.id)
            }
            _ => None,
        })
        .expect("expected the second item before its newline");
    assert!(changes.iter().any(|change| matches!(change,
        Change::Insert { parent: Some(parent), block } if *parent == item
            && matches!(block.data, BlockData::Paragraph(InlineContent::Plain(span)) if parser.source(span) == "[Usage"))));
    view.apply(&mut changes);
    parser.append("](/guide)\n", &mut changes).unwrap();
    view.apply(&mut changes);
    parser.finish(&mut changes).unwrap();
    view.apply(&mut changes);
    assert_eq!(
        view.blocks.into_values().collect::<Vec<_>>(),
        parse(parser.source_text(), &options).unwrap().nodes
    );

    for (prefix, checked) in [("- [x", true), ("- [X", true), ("- [ ", false)] {
        let mut parser = Parser::new(options.clone());
        parser.append(prefix, &mut changes).unwrap();
        parser.append("]", &mut changes).unwrap();
        assert!(changes.is_empty());
        parser.append(" task\n", &mut changes).unwrap();
        assert!(changes.iter().any(|change| matches!(change,
            Change::Insert { block, .. } if matches!(block.data, BlockData::ListItem { task: Some(task) } if task == checked))));
        changes.clear();
    }
}

#[test]
fn a_provisional_html_closer_keeps_raw_bytes_separate_from_visible_content() {
    let options = Options {
        markstream: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    let mut parser = Parser::new(options);
    let mut changes = Vec::new();
    parser.append("<div>内容</d", &mut changes).unwrap();
    let [
        Change::Insert { block, .. },
        Change::Preview {
            id,
            hint: Some(PreviewHint::HtmlContent(content)),
        },
    ] = changes.as_slice()
    else {
        panic!("expected raw HTML and a held closer")
    };
    let id = *id;
    assert_eq!(block.id, id);
    assert_eq!(parser.source(block.span), "<div>内容</d");
    assert_eq!(parser.source(*content), "<div>内容");
    changes.clear();
    parser.append("iv>", &mut changes).unwrap();
    assert!(
        matches!(changes.as_slice(), [Change::Replace(block)] if block.id == id && block.status == BlockStatus::Closed)
    );
}

#[test]
fn incomplete_image_link_is_previewed_without_a_destination() {
    let options = Options {
        markstream: true,
        preview: Preview::Ai,
        ..Options::default()
    };
    let mut parser = Parser::new(options);
    let mut changes = Vec::new();
    parser
        .append("[![badge](https://example.com/bad", &mut changes)
        .unwrap();
    assert!(
        matches!(changes.last(), Some(Change::PreviewInlines { nodes, .. }) if
        matches!(&nodes[0].kind, InlineKind::LinkPreview(link)
            if link.destination.as_str(parser.source_text()).is_empty()
               && matches!(link.children[0].kind, InlineKind::ImagePreview(_))))
    );
}

#[test]
fn fence_info_keeps_absence_and_exact_nonempty_ranges() {
    use std::ops::Range;

    let source = "``` \t\nplain\n```\n\n> ```  中文 \t\n> fenced\n> ```\n";
    let document = stream_markdown_rs::parse(source, &Options::default()).unwrap();
    assert!(matches!(
        document.nodes[0].data,
        BlockData::CodeBlock { .. }
    ));
    assert_eq!(document.nodes[0].data.code_info(), None);
    let segmented = document
        .nodes
        .iter()
        .find(|block| matches!(block.data, BlockData::SegmentedCode(_)))
        .unwrap();
    assert_eq!(
        &source[Range::from(segmented.data.code_info().unwrap())],
        "中文"
    );
}
