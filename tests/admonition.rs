use stream_markdown_rs::{
    AdmonitionKind, BlockData, BlockStatus, Change, InlineContent, InlineKind, Options, Parser,
    parse,
};

fn options() -> Options {
    Options {
        markstream: true,
        ..Options::default()
    }
}

#[test]
fn colon_admonition_keeps_native_inline_parsing_and_titles() {
    let source = "::: note A title\n\\*literal *real*, \\[x\\] and \\$5\\$ with $y$\n:::\n";
    let document = parse(
        source,
        &Options {
            math: true,
            ..options()
        },
    )
    .unwrap();
    let outer = &document.nodes[0];
    let BlockData::Admonition(note) = outer.data else {
        panic!("{outer:?}")
    };
    assert_eq!(note.kind, AdmonitionKind::Note);
    assert!(!note.quoted);
    assert_eq!(outer.status, BlockStatus::Closed);
    let title = note.title(source).unwrap();
    assert_eq!(&source[title.start..title.end], "A title");

    let body = &document.nodes[1];
    assert_eq!(document.parent(body.id), Some(outer.id));
    let BlockData::Paragraph(InlineContent::Parsed(nodes)) = &body.data else {
        panic!("{body:?}")
    };
    let text = nodes
        .iter()
        .filter_map(|node| match &node.kind {
            InlineKind::Text(text) => Some(text.as_str(source)),
            _ => None,
        })
        .collect::<String>();
    assert!(text.contains("*literal"));
    assert!(text.contains("[x] and $5$"));
    assert_eq!(
        nodes
            .iter()
            .filter(|node| matches!(node.kind, InlineKind::Emphasis(_)))
            .count(),
        1
    );
    assert_eq!(
        nodes
            .iter()
            .filter(|node| matches!(node.kind, InlineKind::Math { .. }))
            .count(),
        1
    );
}

#[test]
fn first_quote_marker_is_typed_but_unrecognized_names_and_default_stay_ordinary() {
    let source = "> [!WARNING] Observe\n> Body.\n\nOutside.\n";
    let standard = parse(source, &Options::default()).unwrap();
    assert!(matches!(standard.nodes[0].data, BlockData::Quote));

    let document = parse(source, &options()).unwrap();
    let outer = &document.nodes[0];
    let BlockData::Admonition(admonition) = outer.data else {
        panic!("{outer:?}")
    };
    assert_eq!(admonition.kind, AdmonitionKind::Warning);
    assert!(admonition.quoted);
    assert_eq!(outer.status, BlockStatus::Closed);
    let title = admonition.title(source).unwrap();
    assert_eq!(&source[title.start..title.end], "Observe");
    assert_eq!(document.parent(document.nodes[1].id), Some(outer.id));
    assert!(document.parent(document.nodes.last().unwrap().id).is_none());

    let unrecognized = parse("> [!UNKNOWN]\n> body\n", &options()).unwrap();
    assert!(matches!(unrecognized.nodes[0].data, BlockData::Quote));
    for name in ["note-test", "NOTE", "important"] {
        let source = format!("::: {name}\nbody\n:::\n");
        let vmr = parse(&source, &options()).unwrap();
        assert!(matches!(vmr.nodes[0].data, BlockData::Vmr(_)));
    }
}

#[test]
fn streaming_colon_header_changes_kind_without_replacing_the_container_id() {
    let mut parser = Parser::new(options());
    let mut changes = Vec::new();
    parser.append("::: not", &mut changes).unwrap();
    let id = changes
        .iter()
        .find_map(|change| match change {
            Change::Insert { block, .. } if matches!(block.data, BlockData::Vmr(_)) => {
                Some(block.id)
            }
            _ => None,
        })
        .unwrap();
    changes.clear();
    parser.append("e\ncontent\n:::", &mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(change, Change::Replace(block)
        if block.id == id && matches!(block.data, BlockData::Admonition(value) if value.kind == AdmonitionKind::Note)
    )));
    assert!(changes.iter().any(|change| matches!(change,
        Change::Insert { parent: Some(parent), block }
        if *parent == id && matches!(block.data, BlockData::Paragraph(_))
    )));
    assert!(changes.iter().any(|change| matches!(change,
        Change::SetStatus { id: updated, status: BlockStatus::Closed, .. }
        if *updated == id
    )));
}

#[test]
fn only_a_colon_admonition_requires_an_explicit_close() {
    let colon = parse("::: info\nbody", &options()).unwrap();
    assert!(matches!(colon.nodes[0].data, BlockData::Admonition(_)));
    assert_eq!(colon.nodes[0].status, BlockStatus::Incomplete);

    let quote = parse("> [!TIP]\n> body", &options()).unwrap();
    assert!(matches!(quote.nodes[0].data, BlockData::Admonition(_)));
    assert_eq!(quote.nodes[0].status, BlockStatus::Closed);
}
