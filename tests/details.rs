use stream_markdown_rs::{
    BlockData, BlockStatus, Change, InlineContent, InlineKind, Options, Parser, Preview, parse,
};

fn options() -> Options {
    Options {
        markstream: true,
        ..Options::default()
    }
}

#[test]
fn html_container_closing_ranges_match_full_parse_at_every_split() {
    for source in [
        "<details>\r\n<summary>Summary</summary>\r\n\r\ntext\r\n</details> \t\r\nnext\r\n",
        "<section>\nintro\n\ntext\n</section>\t\nnext\n",
        "<details>\ntext\n</details>  ",
        "<details>\ntext\n</details> outside\n",
    ] {
        let expected = parse(source, &options()).unwrap().nodes.remove(0);
        assert_eq!(expected.status, BlockStatus::Closed);
        for split in source
            .char_indices()
            .map(|(index, _)| index)
            .chain([source.len()])
        {
            let mut parser = Parser::new(options());
            let mut changes = Vec::new();
            parser.append(&source[..split], &mut changes).unwrap();
            parser.append(&source[split..], &mut changes).unwrap();
            parser.finish(&mut changes).unwrap();
            let mut actual = None;
            for change in changes {
                match change {
                    Change::Insert { block, .. } | Change::Replace(block)
                        if block.id == expected.id =>
                    {
                        actual = Some(block)
                    }
                    Change::SetStatus { id, status, end } if id == expected.id => {
                        let block = actual.as_mut().expect("update follows the original insert");
                        block.status = status;
                        block.span.end = end;
                    }
                    _ => {}
                }
            }
            assert_eq!(
                actual.as_ref(),
                Some(&expected),
                "split {split}: {source:?}"
            );
        }
    }
}

#[test]
fn div_promotes_at_a_blank_line_and_keeps_typed_children_and_original_attributes() {
    let source = "<DIV role=note>\r\n**bold**\r\n\r\n- item\r\n\r\n</div>\r\n# outside\r\n";
    let document = parse(source, &options()).unwrap();
    let wrapper = &document.nodes[0];
    let BlockData::HtmlWrapper(opening) = wrapper.data else {
        panic!("{wrapper:?}");
    };
    assert_eq!(wrapper.status, BlockStatus::Closed);
    assert_eq!(
        opening.tag(source).attrs(source).next().unwrap().value,
        Some("note")
    );
    let children: Vec<_> = document
        .nodes
        .iter()
        .filter(|node| document.parent(node.id) == Some(wrapper.id))
        .collect();
    assert!(
        matches!(&children[0].data, BlockData::Paragraph(InlineContent::Parsed(nodes))
        if matches!(nodes[0].kind, InlineKind::Strong(_)))
    );
    assert!(matches!(children[1].data, BlockData::List { .. }));
    let outside = document.nodes.last().unwrap();
    assert!(matches!(outside.data, BlockData::Heading { .. }));
    assert_eq!(document.parent(outside.id), None);
    assert!(matches!(
        parse(source, &Options::default()).unwrap().nodes[0].data,
        BlockData::Html
    ));

    let compact = "<div>\n**literal**\n</div>\n\n";
    let document = parse(compact, &options()).unwrap();
    assert!(matches!(document.nodes[0].data, BlockData::Html));
}

#[test]
fn streaming_wrappers_keep_the_original_id_and_hold_a_split_closer() {
    let mut parser = Parser::new(options());
    let mut changes = Vec::new();
    parser.append("<section>\nintro\n", &mut changes).unwrap();
    let [Change::Insert { block, .. }, ..] = changes.as_slice() else {
        panic!("expected raw HTML: {changes:?}");
    };
    assert!(matches!(block.data, BlockData::Html));
    let id = block.id;
    changes.clear();
    parser.append("\n- item\n\n</sec", &mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(change, Change::Replace(block)
        if block.id == id && block.status == BlockStatus::Open && matches!(block.data, BlockData::HtmlWrapper(_)))));
    assert!(
        !changes
            .iter()
            .any(|change| matches!(change, Change::Replace(block)
        if block.id == id && block.status == BlockStatus::Closed))
    );
    changes.clear();
    parser.append("tion>outside", &mut changes).unwrap();
    assert!(
        changes
            .iter()
            .any(|change| matches!(change, Change::Replace(block)
        if block.id == id && block.status == BlockStatus::Closed))
    );
    parser.finish(&mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(change, Change::Insert { block, parent: None }
        if matches!(block.data, BlockData::Paragraph(InlineContent::Plain(span)) if parser.source(span) == "outside"))));
}

#[test]
fn details_groups_native_children_and_preserves_attributes_and_closing_newline() {
    let source = "<details open style=\"color: gray\">\nlead\n<summary>**sum** _x_</summary>\ntext\n<div>inner</div>\n\npara\n</details>\n\n# Next\n";
    let document = parse(source, &options()).unwrap();
    let outer = &document.nodes[0];
    let BlockData::Details(opening) = outer.data else {
        panic!("{outer:?}")
    };
    assert_eq!(outer.status, BlockStatus::Closed);
    assert!(source[outer.span.start..outer.span.end].ends_with("</details>\n"));
    let tag = opening.tag(source);
    let attrs = tag
        .attrs(source)
        .map(|attr| (attr.name, attr.value))
        .collect::<Vec<_>>();
    assert_eq!(attrs, [("open", None), ("style", Some("color: gray"))]);

    let children = document
        .nodes
        .iter()
        .filter(|block| document.parent(block.id) == Some(outer.id))
        .collect::<Vec<_>>();
    assert_eq!(children.len(), 5);
    assert!(matches!(children[0].data, BlockData::Paragraph(_)));
    let BlockData::Summary {
        content: InlineContent::Parsed(inlines),
        ..
    } = &children[1].data
    else {
        panic!("{:?}", children[1]);
    };
    assert!(matches!(inlines[0].kind, InlineKind::Strong(_)));
    assert!(matches!(inlines[2].kind, InlineKind::Emphasis(_)));
    assert!(matches!(children[2].data, BlockData::Paragraph(_)));
    assert!(matches!(
        children[3].data,
        BlockData::Html | BlockData::SegmentedHtml(_)
    ));
    assert!(matches!(children[4].data, BlockData::Paragraph(_)));
    let trailing = document.nodes.last().unwrap();
    assert!(matches!(trailing.data, BlockData::Heading { .. }));
    assert!(document.parent(trailing.id).is_none());
}

#[test]
fn same_line_summary_and_footer_do_not_absorb_following_text() {
    let source = "<DETAILS open> <Summary>Sum</SUMMARY> body </DETAILS> tail\n";
    let document = parse(source, &options()).unwrap();
    let outer = &document.nodes[0];
    assert!(matches!(outer.data, BlockData::Details(_)));
    assert_eq!(outer.status, BlockStatus::Closed);
    assert_eq!(
        &source[outer.span.start..outer.span.end],
        "<DETAILS open> <Summary>Sum</SUMMARY> body </DETAILS>"
    );
    let children = document
        .nodes
        .iter()
        .filter(|block| document.parent(block.id) == Some(outer.id))
        .collect::<Vec<_>>();
    assert_eq!(children.len(), 2);
    assert!(matches!(children[0].data, BlockData::Summary { .. }));
    assert!(matches!(children[1].data, BlockData::Paragraph(_)));
    let trailing = document.nodes.last().unwrap();
    assert!(matches!(trailing.data, BlockData::Paragraph(_)));
    assert_eq!(&source[trailing.span.start..trailing.span.end], "tail");
    assert!(document.parent(trailing.id).is_none());
}

#[test]
fn source_owned_fences_shield_details_markers_and_nested_details_keep_parentage() {
    let source = "<details>\n<summary>outer</summary>\n<details>\n<summary>inner</summary>\n\n```text\n</details>\n```\n\ninner body\n</details>\n\nouter body\n</details>\n";
    let document = parse(source, &options()).unwrap();
    let containers = document
        .nodes
        .iter()
        .filter(|block| matches!(block.data, BlockData::Details(_)))
        .collect::<Vec<_>>();
    assert_eq!(containers.len(), 2);
    assert!(
        containers
            .iter()
            .all(|block| block.status == BlockStatus::Closed)
    );
    assert_eq!(document.parent(containers[1].id), Some(containers[0].id));
    let code = document
        .nodes
        .iter()
        .find(|block| {
            matches!(
                block.data,
                BlockData::CodeBlock { .. } | BlockData::SegmentedCode(_)
            )
        })
        .unwrap();
    assert_eq!(document.parent(code.id), Some(containers[1].id));
    assert!(source[code.span.start..code.span.end].contains("\n</details>\n"));

    let plain = parse(source, &Options::default()).unwrap();
    assert!(plain.nodes.iter().all(|block| !matches!(
        block.data,
        BlockData::Details(_) | BlockData::Summary { .. }
    )));
}

#[test]
fn stream_keeps_details_and_summary_ids_across_split_open_and_close_tags() {
    let mut parser = Parser::new(options());
    let mut changes = Vec::new();
    parser.append("<det", &mut changes).unwrap();
    assert!(changes.is_empty());
    parser.append("ails title=\"x > ", &mut changes).unwrap();
    assert!(changes.is_empty());
    parser.append("y\">\n<sum", &mut changes).unwrap();
    let outer = changes
        .iter()
        .find_map(|change| match change {
            Change::Insert { block, .. } if matches!(block.data, BlockData::Details(_)) => {
                assert_eq!(block.status, BlockStatus::Open);
                Some(block.id)
            }
            _ => None,
        })
        .unwrap();
    changes.clear();
    parser
        .append("mary>**sum** _x_</SUM", &mut changes)
        .unwrap();
    let summary = changes
        .iter()
        .find_map(|change| match change {
            Change::Insert {
                block,
                parent: Some(parent),
            } if *parent == outer && matches!(block.data, BlockData::Summary { .. }) => {
                assert_eq!(block.status, BlockStatus::Open);
                Some(block.id)
            }
            _ => None,
        })
        .unwrap();
    changes.clear();
    parser
        .append("MARY>\nbody\n</detail", &mut changes)
        .unwrap();
    assert!(changes.iter().any(|change| matches!(change, Change::Replace(block) if block.id == summary && block.status == BlockStatus::Closed)));
    assert!(!changes.iter().any(|change| matches!(change, Change::Replace(block) if block.id == outer && block.status == BlockStatus::Closed)));
    changes.clear();
    parser.append("s>", &mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(change, Change::Replace(block) if block.id == outer && block.status == BlockStatus::Closed)));
}

#[test]
fn late_reference_only_updates_the_summary_waiting_on_its_definition() {
    let mut parser = Parser::new(options());
    let mut changes = Vec::new();
    parser.append("Before.\n\n<details>\n<summary>See [label][id]</summary>\n</details>\n\nUnrelated.\n\n", &mut changes).unwrap();
    let summary = changes
        .iter()
        .find_map(|change| match change {
            Change::Insert { block, .. } if matches!(block.data, BlockData::Summary { .. }) => {
                Some(block.id)
            }
            _ => None,
        })
        .unwrap();
    changes.clear();
    parser.append("[id]: /target\n\n", &mut changes).unwrap();
    let links = changes
        .iter()
        .find_map(|change| match change {
            Change::Replace(block) if block.id == summary => match &block.data {
                BlockData::Summary {
                    content: InlineContent::Parsed(nodes),
                    ..
                } => Some(nodes),
                _ => None,
            },
            _ => None,
        })
        .unwrap();
    assert!(
        links
            .iter()
            .any(|node| matches!(node.kind, InlineKind::Link(_)))
    );
    assert_eq!(parser.metrics().invalidated_blocks, 1);
}

#[test]
fn streamed_details_exposes_list_items_before_their_newlines() {
    let mut parser = Parser::new(Options {
        preview: Preview::Ai,
        ..options()
    });
    let mut changes = Vec::new();
    parser
        .append("<details>\n<summary>Summary</summary>\n\n", &mut changes)
        .unwrap();
    let Change::Insert { block: outer, .. } = &changes[0] else {
        panic!("expected details")
    };
    let outer = outer.id;
    changes.clear();
    parser.append("- fi", &mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(change,
        Change::SetStatus { id, status: BlockStatus::Open, end }
            if *id == outer && *end == parser.source_text().len())));
    let list = changes
        .iter()
        .find_map(|change| match change {
            Change::Insert { parent, block }
                if *parent == Some(outer) && matches!(block.data, BlockData::List { .. }) =>
            {
                Some(block.id)
            }
            _ => None,
        })
        .expect("expected the list inside details");
    changes.clear();
    parser.append("rst\n- se", &mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(change,
        Change::Insert { parent, block } if *parent == Some(list)
            && matches!(block.data, BlockData::ListItem { .. }))));
    assert!(changes.iter().any(|change| matches!(change,
        Change::Insert { block, .. }
            if matches!(block.data, BlockData::Paragraph(InlineContent::Plain(span)) if parser.source(span) == "se"))));
    let confirmed_end = parser.source_text().len() + "cond\n\n".len();
    changes.clear();
    parser.append("cond\n\n</det", &mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(change,
        Change::SetStatus { id, status: BlockStatus::Open, end }
            if *id == outer && *end == confirmed_end)));
}

#[test]
fn missing_closers_keep_source_boundaries_and_finish_incomplete() {
    let source = "<details><summary>**partial**";
    let document = parse(source, &options()).unwrap();
    assert_eq!(document.nodes.len(), 2);
    assert!(
        document
            .nodes
            .iter()
            .all(|block| block.status == BlockStatus::Incomplete)
    );
    assert!(matches!(document.nodes[0].data, BlockData::Details(_)));
    assert!(matches!(
        document.nodes[1].data,
        BlockData::Summary {
            content: InlineContent::Parsed(_),
            ..
        }
    ));
    assert_eq!(document.nodes[0].span.end, source.len());
    assert_eq!(document.nodes[1].span.end, source.len());
}

#[test]
fn summary_opening_keeps_exact_source_with_a_container_prefix() {
    use std::ops::Range;

    let source = "> <details>\n>   <summary data-x='值'>Heading</summary>\n> </details>\n";
    let document = parse(source, &options()).unwrap();
    let opening = document
        .nodes
        .iter()
        .find_map(|block| block.summary_opening())
        .unwrap();
    assert_eq!(&source[Range::from(opening.span)], "<summary data-x='值'>");
    assert_eq!(
        opening.tag(source).attrs(source).next().unwrap().value,
        Some("值")
    );
}
