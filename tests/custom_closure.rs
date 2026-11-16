use stream_markdown_rs::{
    Block, BlockData, BlockStatus, Change, Inline, InlineContent, InlineElement, InlineKind,
    Options, Parser, parse,
};

fn options() -> Options {
    Options {
        custom_tags: vec!["custom-data".into(), "other-data".into()],
        markstream: true,
        ..Options::default()
    }
}

fn custom(block: &Block) -> Option<(&Inline, &InlineElement)> {
    let BlockData::Paragraph(InlineContent::Parsed(nodes)) = &block.data else {
        return None;
    };
    nodes.iter().find_map(|node| match &node.kind {
        InlineKind::CustomTag(element) => Some((node, element.as_ref())),
        _ => None,
    })
}

#[test]
fn batch_detached_closer_changes_only_inline_range_and_keeps_trailing_text_separate() {
    let source = "- prefix <custom-data>{\"a\":\"b\"}\n\n</custom-data> trailing";
    let document = parse(source, &options()).unwrap();
    let origin = document
        .nodes
        .iter()
        .find(|block| custom(block).is_some())
        .unwrap();
    let (node, element) = custom(origin).unwrap();
    assert_eq!(element.status, BlockStatus::Closed);
    assert_eq!(
        &source[element.body.start..element.body.end],
        "{\"a\":\"b\"}"
    );
    assert_eq!(
        &source[node.span.start..node.span.end],
        "<custom-data>{\"a\":\"b\"}\n\n</custom-data>"
    );
    assert_eq!(
        &source[origin.span.start..origin.span.end],
        "prefix <custom-data>{\"a\":\"b\"}"
    );
    assert!(document.parent(origin.id).is_some());
    let tail = document.nodes.last().unwrap();
    assert_eq!(&source[tail.span.start..tail.span.end], "trailing");
    assert!(document.parent(tail.id).is_none());
}

#[test]
fn split_detached_closer_updates_existing_node_without_reparsing_its_paragraph() {
    let mut parser = Parser::new(options());
    let mut changes = Vec::new();
    parser
        .append("- prefix <custom-data>{\"a\":\"b\"}\n\n", &mut changes)
        .unwrap();
    let mut paragraph = changes
        .iter()
        .rev()
        .find_map(|change| match change {
            Change::Insert { block, .. } | Change::Replace(block) if custom(block).is_some() => {
                Some(block.clone())
            }
            _ => None,
        })
        .unwrap();
    let (node, element) = custom(&paragraph).unwrap();
    assert_eq!(element.status, BlockStatus::Incomplete);
    let inline_start = node.span.start;
    let body = element.body;
    let paragraph_id = paragraph.id;
    let paragraph_span = paragraph.span;

    changes.clear();
    let before = parser.metrics().inline_bytes;
    for chunk in ["</", "custom-", "data"] {
        parser.append(chunk, &mut changes).unwrap();
        assert!(
            !changes
                .iter()
                .any(|change| matches!(change, Change::CloseInlineElement { .. }))
        );
    }
    parser.append(">", &mut changes).unwrap();
    assert_eq!(parser.metrics().inline_bytes, before);
    let close = changes
        .iter()
        .find_map(|change| match change {
            Change::CloseInlineElement { id, start, end } if *id == paragraph_id => {
                Some((*start, *end))
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(close.0, inline_start);
    let BlockData::Paragraph(content) = &mut paragraph.data else {
        unreachable!()
    };
    assert!(content.close_inline_element(close.0, close.1));
    let (node, element) = custom(&paragraph).unwrap();
    assert_eq!(element.status, BlockStatus::Closed);
    assert_eq!(element.body, body);
    assert_eq!(paragraph.span, paragraph_span);
    assert!(parser.source(node.span).ends_with("</custom-data>"));
    assert!(
        !changes
            .iter()
            .any(|change| matches!(change, Change::Replace(block) if block.id == paragraph_id))
    );
}

#[test]
fn unmatched_nonadjacent_and_code_closers_do_not_attach() {
    for tail in [
        "</other-data>",
        "Ordinary sibling.\n\n</custom-data>",
        "    </custom-data>",
        "```text\n</custom-data>\n```",
    ] {
        let source = format!("- prefix <custom-data>data\n\n{tail}");
        let document = parse(&source, &options()).unwrap();
        let (node, element) = document.nodes.iter().find_map(custom).unwrap();
        assert_eq!(element.status, BlockStatus::Incomplete, "{tail:?}");
        assert_eq!(
            &source[node.span.start..node.span.end],
            "<custom-data>data",
            "{tail:?}"
        );
    }
}

#[test]
fn late_inline_reference_updates_keep_a_previously_attached_closer() {
    let mut parser = Parser::new(options());
    let mut changes = Vec::new();
    parser
        .append(
            "- [before][id] <custom-data>data\n\n</custom-data>\n\n",
            &mut changes,
        )
        .unwrap();
    let paragraph = changes
        .iter()
        .find_map(|change| match change {
            Change::CloseInlineElement { id, .. } => Some(*id),
            _ => None,
        })
        .unwrap();
    changes.clear();
    parser.append("[id]: /url\n\n", &mut changes).unwrap();
    let block = changes
        .iter()
        .find_map(|change| match change {
            Change::Replace(block) if block.id == paragraph && custom(block).is_some() => {
                Some(block)
            }
            _ => None,
        })
        .unwrap();
    let (node, element) = custom(block).unwrap();
    assert_eq!(element.status, BlockStatus::Closed);
    assert!(parser.source(node.span).ends_with("</custom-data>"));
    let BlockData::Paragraph(InlineContent::Parsed(nodes)) = &block.data else {
        unreachable!()
    };
    assert!(
        nodes
            .iter()
            .any(|inline| matches!(inline.kind, InlineKind::Link(_)))
    );
}
