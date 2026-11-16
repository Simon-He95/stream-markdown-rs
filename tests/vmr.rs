use stream_markdown_rs::{BlockData, BlockStatus, Change, Options, Parser, Vmr, parse};

fn options() -> Options {
    Options {
        markstream: true,
        ..Options::default()
    }
}

#[test]
fn optional_vmr_preserves_parentage_and_protects_code_body() {
    let source = "Above\n:::viewcode:topo args {\"devId\":\"abc\",\"meta\":{\"args\":[1,2]}}\n\
        A paragraph.\n\n```ts\nconsole.log(1)\n:::\n```\n\n- list item\n:::\nBelow.";
    let document = parse(source, &options()).unwrap();
    let position = document
        .nodes
        .iter()
        .position(|block| matches!(block.data, BlockData::Vmr(_)))
        .unwrap();
    let container = &document.nodes[position];
    let BlockData::Vmr(vmr) = container.data else {
        unreachable!()
    };
    let params = vmr.parameters(source);
    assert_eq!(&source[params.name.start..params.name.end], "viewcode:topo");
    let args = params.args.unwrap();
    assert_eq!(&source[args.start..args.end], "args");
    let attributes = params.attributes.unwrap();
    assert_eq!(
        &source[attributes.start..attributes.end],
        "{\"devId\":\"abc\",\"meta\":{\"args\":[1,2]}}"
    );
    assert_eq!(container.status, BlockStatus::Closed);

    let children = document
        .nodes
        .iter()
        .filter(|block| document.parent(block.id) == Some(container.id))
        .collect::<Vec<_>>();
    assert_eq!(children.len(), 3);
    assert!(matches!(children[0].data, BlockData::Paragraph(_)));
    assert!(matches!(
        children[1].data,
        BlockData::CodeBlock { .. } | BlockData::SegmentedCode { .. }
    ));
    let code_source = &source[children[1].span.start..children[1].span.end];
    assert!(code_source.contains("\n:::\n"));
    assert!(matches!(children[2].data, BlockData::List { .. }));
    assert!(document.parent(document.nodes.last().unwrap().id).is_none());
    assert!(matches!(document.nodes[0].data, BlockData::Paragraph(_)));
    assert!(
        parse(source, &Options::default())
            .unwrap()
            .nodes
            .iter()
            .all(|block| !matches!(block.data, BlockData::Vmr(_)))
    );
}

#[test]
fn container_attributes_borrow_balanced_objects_even_with_quoted_braces() {
    let source = r#"::: viewcode:stream raw {"quoted":"}","nested":{"inside":"{"}}"#;
    let vmr = Vmr {
        header: stream_markdown_rs::Span::new(0, source.len()),
    };
    let params = vmr.parameters(source);
    assert_eq!(
        &source[params.name.start..params.name.end],
        "viewcode:stream"
    );
    let args = params.args.unwrap();
    assert_eq!(&source[args.start..args.end], "raw");
    let attrs = params.attributes.unwrap();
    assert_eq!(
        &source[attrs.start..attrs.end],
        r#"{"quoted":"}","nested":{"inside":"{"}}"#
    );

    let incomplete = "::: viewcode:stream {xxx:yyy";
    let vmr = Vmr {
        header: stream_markdown_rs::Span::new(0, incomplete.len()),
    };
    let params = vmr.parameters(incomplete);
    let args = params.args.unwrap();
    assert_eq!(&incomplete[args.start..args.end], "{xxx:yyy");
    assert!(params.attributes.is_none());
}

#[test]
fn streaming_body_keeps_parent_id_and_bare_closer_reports_closed_before_newline() {
    let mut parser = Parser::new(options());
    let mut changes = Vec::new();
    parser.append("::: viewcode:open", &mut changes).unwrap();
    let id = changes
        .iter()
        .find_map(|change| match change {
            Change::Insert { block, .. } if matches!(block.data, BlockData::Vmr(_)) => {
                assert_eq!(block.status, BlockStatus::Open);
                Some(block.id)
            }
            _ => None,
        })
        .unwrap();
    changes.clear();
    parser.append(" extra\n", &mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(change, Change::Replace(block) if block.id == id && matches!(block.data, BlockData::Vmr(_)))));
    changes.clear();
    parser.append("con", &mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(change,
        Change::Insert { parent: Some(parent), block } if *parent == id &&
            matches!(block.data, BlockData::Paragraph(_)) && parser.source(block.span) == "con"
    )));
    changes.clear();
    parser.append("tent\n::", &mut changes).unwrap();
    assert!(!changes.iter().any(|change| matches!(change,
        Change::SetStatus { id: updated, status: BlockStatus::Closed, .. } if *updated == id
    )));
    changes.clear();
    parser.append(":", &mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(change,
        Change::SetStatus { id: updated, status: BlockStatus::Closed, .. } if *updated == id
    )));
    changes.clear();
    parser.finish(&mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(change,
        Change::Replace(block) if block.id == id && block.status == BlockStatus::Closed
    )));
}

#[test]
fn a_bare_tail_closer_can_turn_into_a_nested_opener_without_replacing_outer_id() {
    let mut parser = Parser::new(options());
    let mut changes = Vec::new();
    parser.append("::: outer\n:::", &mut changes).unwrap();
    let outer = changes
        .iter()
        .find_map(|change| match change {
            Change::Insert { block, .. } if matches!(block.data, BlockData::Vmr(_)) => {
                Some(block.id)
            }
            _ => None,
        })
        .unwrap();
    changes.clear();

    parser.append("inner\nbody\n:::\n", &mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(change,
        Change::SetStatus { id, status: BlockStatus::Open, .. } if *id == outer
    )));
    let inner = changes
        .iter()
        .find_map(|change| match change {
            Change::Insert {
                parent: Some(parent),
                block,
            } if *parent == outer && matches!(block.data, BlockData::Vmr(_)) => Some(block.id),
            _ => None,
        })
        .unwrap();
    assert!(changes.iter().any(|change| matches!(change,
        Change::Replace(block) if block.id == inner && block.status == BlockStatus::Closed
    )));
    changes.clear();
    parser.finish(&mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(change,
        Change::Replace(block) if block.id == outer && block.status == BlockStatus::Incomplete
    )));
}

#[test]
fn split_opening_marker_stays_pending_until_the_name_arrives() {
    let mut parser = Parser::new(options());
    let mut changes = Vec::new();
    for chunk in [":", ":", ":", " "] {
        parser.append(chunk, &mut changes).unwrap();
        assert!(changes.is_empty());
    }
    parser.append("viewcode", &mut changes).unwrap();
    assert!(matches!(
        &changes[..],
        [Change::Insert { parent: None, block }] if matches!(block.data, BlockData::Vmr(_))
    ));
}

#[test]
fn streaming_bare_closer_ends_vmr_even_after_a_list_child() {
    let mut parser = Parser::new(options());
    let mut changes = Vec::new();
    parser.append("::: outer\n- item\n", &mut changes).unwrap();
    let container = changes
        .iter()
        .find_map(|change| match change {
            Change::Insert { block, .. } if matches!(block.data, BlockData::Vmr(_)) => {
                Some(block.id)
            }
            _ => None,
        })
        .unwrap();
    changes.clear();
    parser.append(":::", &mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(change,
        Change::SetStatus { id, status: BlockStatus::Closed, .. } if *id == container
    )));
}

#[test]
fn a_new_vmr_after_an_unindented_list_is_its_sibling() {
    let source = "::: outer\n- item\n::: inner\ncontent\n:::\n:::\n";
    let document = parse(source, &options()).unwrap();
    let outer = document
        .nodes
        .iter()
        .find(|block| matches!(block.data, BlockData::Vmr(_)))
        .unwrap();
    let children = document
        .nodes
        .iter()
        .filter(|block| document.parent(block.id) == Some(outer.id))
        .collect::<Vec<_>>();
    assert_eq!(children.len(), 2);
    assert!(matches!(children[0].data, BlockData::List { .. }));
    assert!(matches!(children[1].data, BlockData::Vmr(_)));
}

#[test]
fn an_unindented_vmr_closer_ends_a_footnote_child() {
    let source = "::: outer\nA[^1].\n\n[^1]: note\n:::\n";
    let document = parse(
        source,
        &Options {
            footnotes: true,
            ..options()
        },
    )
    .unwrap();
    let outer = &document.nodes[0];
    assert!(matches!(outer.data, BlockData::Vmr(_)));
    assert_eq!(outer.status, BlockStatus::Closed);
    let note = document
        .nodes
        .iter()
        .find(|block| matches!(block.data, BlockData::Footnote { .. }))
        .unwrap();
    assert_eq!(document.parent(note.id), Some(outer.id));
    assert_eq!(note.status, BlockStatus::Closed);
}
