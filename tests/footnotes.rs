use stream_markdown_rs::{
    Block, BlockData, BlockStatus, Change, Inline, InlineContent, InlineKind, Options, Parser,
    parse,
};

fn options() -> Options {
    Options {
        footnotes: true,
        ..Options::default()
    }
}

fn published(change: &Change) -> Option<&Block> {
    match change {
        Change::Insert { block, .. } | Change::Replace(block) => Some(block),
        _ => None,
    }
}

fn inlines(block: &Block) -> &[Inline] {
    match &block.data {
        BlockData::Paragraph(InlineContent::Parsed(nodes))
        | BlockData::Footnote {
            content: InlineContent::Parsed(nodes),
            ..
        } => nodes,
        _ => &[],
    }
}

#[test]
fn an_ambiguous_definition_tail_is_hidden_until_the_note_has_content() {
    let completed = "[^1]: Note.";
    for suffix in ["[^", "[^1", "[^1]", "[^1]:", "[^1]: "] {
        let mut parser = Parser::new(options());
        let mut changes = Vec::new();
        parser.append("Reference[^1].\n\n", &mut changes).unwrap();
        let reference = changes.iter().filter_map(published).next_back().unwrap().id;
        changes.clear();
        parser.append(suffix, &mut changes).unwrap();
        assert!(changes.is_empty(), "{suffix:?}: {changes:?}");

        parser
            .append(&completed[suffix.len()..], &mut changes)
            .unwrap();
        let note = changes
            .iter()
            .filter_map(published)
            .find(|block| matches!(block.data, BlockData::Footnote { .. }))
            .unwrap();
        assert_eq!(note.status, BlockStatus::Open);
        let linked = changes
            .iter()
            .filter_map(published)
            .find(|block| block.id == reference)
            .unwrap();
        assert!(inlines(linked).iter().any(|node| {
            matches!(node.kind, InlineKind::FootnoteReference { definition, .. } if definition == note.id)
        }));

        let id = note.id;
        changes.clear();
        parser.finish(&mut changes).unwrap();
        let closed = changes
            .iter()
            .filter_map(published)
            .find(|block| block.id == id)
            .unwrap();
        assert_eq!(closed.status, BlockStatus::Closed);
    }
}

#[test]
fn a_final_unresolved_label_is_literal_but_an_empty_definition_resolves_it() {
    for label in ["1", "note_1", "note-with-dash", "引用"] {
        let source = format!("Reference[^{label}].\n\n[^{label}]");
        let document = parse(&source, &options()).unwrap();
        assert_eq!(document.nodes.len(), 2);
        assert!(
            document
                .nodes
                .iter()
                .all(|block| { !matches!(block.data, BlockData::Footnote { .. }) })
        );
        assert_eq!(
            &source[document.nodes[1].span.start..document.nodes[1].span.end],
            format!("[^{label}]")
        );

        let source = format!("Reference[^{label}].\n\n[^{label}]:");
        let document = parse(&source, &options()).unwrap();
        let note = document
            .nodes
            .iter()
            .find(|block| matches!(block.data, BlockData::Footnote { .. }))
            .unwrap();
        assert_eq!(note.status, BlockStatus::Closed);
        assert!(inlines(&document.nodes[0]).iter().any(|node| {
            matches!(node.kind, InlineKind::FootnoteReference { definition, .. } if definition == note.id)
        }));
    }
}

#[test]
fn an_existing_definition_resolves_a_bare_reference_before_the_line_ends() {
    let mut parser = Parser::new(options());
    let mut changes = Vec::new();
    parser
        .append("[^1]: Existing note.\n\n", &mut changes)
        .unwrap();
    let note = changes
        .iter()
        .filter_map(published)
        .find(|block| matches!(block.data, BlockData::Footnote { .. }))
        .unwrap()
        .id;
    changes.clear();
    parser.append("[^1]", &mut changes).unwrap();
    let reference = changes
        .iter()
        .filter_map(published)
        .find(|block| matches!(block.data, BlockData::Paragraph(_)))
        .unwrap();
    assert_eq!(reference.status, BlockStatus::Open);
    assert!(inlines(reference).iter().any(|inline| {
        matches!(inline.kind, InlineKind::FootnoteReference { definition, .. } if definition == note)
    }));
}

#[test]
fn late_footnotes_invalidate_only_leaves_waiting_on_the_exact_label() {
    let mut parser = Parser::new(options());
    let mut changes = Vec::new();
    parser
        .append(
            "Stable.\n\nWanted[^Key].\n\nUnrelated[^key].\n\n",
            &mut changes,
        )
        .unwrap();
    let dependent = changes
        .iter()
        .filter_map(published)
        .find(|block| parser.source(block.span) == "Wanted[^Key].")
        .unwrap()
        .id;
    assert_eq!(parser.metrics().invalidated_blocks, 0);

    changes.clear();
    parser
        .append("[^Key]: Defined **once**.\n\nLater text.", &mut changes)
        .unwrap();
    let updates = changes
        .iter()
        .filter_map(|change| match change {
            Change::Replace(block) if matches!(block.data, BlockData::Paragraph(_)) => {
                Some(block.id)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(updates, [dependent]);
    assert_eq!(parser.metrics().invalidated_blocks, 1);
}

#[test]
fn named_footnotes_respect_escapes_and_code_and_are_opt_in() {
    let source = "\\* real[^id], escaped \\[^id], `[^id]`.\n\n[^id]: Body\n\n    More *content*.";
    let no_extension = parse(source, &Options::default()).unwrap();
    assert!(
        no_extension
            .nodes
            .iter()
            .all(|block| { !matches!(block.data, BlockData::Footnote { .. }) })
    );

    let document = parse(source, &options()).unwrap();
    let references = inlines(&document.nodes[0])
        .iter()
        .filter(|node| matches!(node.kind, InlineKind::FootnoteReference { .. }))
        .count();
    assert_eq!(references, 1);
    let note = document
        .nodes
        .iter()
        .find(|block| matches!(block.data, BlockData::Footnote { .. }))
        .unwrap();
    assert!(
        inlines(note)
            .iter()
            .any(|inline| matches!(inline.kind, InlineKind::Emphasis(_)))
    );
}

#[test]
fn completed_reference_text_and_open_continuations_are_visible_during_streaming() {
    for source in [
        "[^unknown]\n",
        "Text [^unknown]",
        "[^unknown] followed by text",
    ] {
        let mut parser = Parser::new(options());
        let mut changes = Vec::new();
        parser.append(source, &mut changes).unwrap();
        let paragraph = changes
            .iter()
            .filter_map(published)
            .find(|block| matches!(block.data, BlockData::Paragraph(_)))
            .unwrap();
        assert!(parser.source(paragraph.span).contains("[^unknown]"));
    }

    let mut parser = Parser::new(options());
    let mut changes = Vec::new();
    parser
        .append("Note[^id].\n\n[^id]: First.\n\n", &mut changes)
        .unwrap();
    let note = changes
        .iter()
        .filter_map(published)
        .find(|block| matches!(block.data, BlockData::Footnote { .. }))
        .unwrap()
        .id;
    changes.clear();
    parser.append("    Second.\n", &mut changes).unwrap();
    assert!(changes.iter().any(|change| match change {
        Change::AddTextFragment { id, span } => *id == note && parser.source(*span) == "Second.\n",
        _ => false,
    }));
    assert!(
        changes
            .iter()
            .filter_map(published)
            .all(|block| block.id != note)
    );
}
