use stream_markdown_rs::{Options, parse, restore_history};

#[test]
fn history_restores_visible_messages_first_and_keeps_document_boundaries() {
    let messages = [
        "[id]: /first\n\n[earlier][id]",
        "> [visible][id]\n>\n> [id]: /visible",
        "| cell |\n| --- |\n| value |",
        "[last][id]",
    ];
    let options = Options::default();
    let mut restore = restore_history(&messages, 1..3, &options);
    let first = restore.by_ref().take(2).collect::<Vec<_>>();
    assert_eq!(
        first.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
        [1, 2]
    );
    let rest = restore.collect::<Vec<_>>();
    assert_eq!(
        rest.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
        [0, 3]
    );
    for (index, actual) in first.into_iter().chain(rest) {
        let actual = actual.unwrap();
        let expected = parse(messages[index], &options).unwrap();
        assert_eq!(actual.source, expected.source);
        assert_eq!(actual.nodes, expected.nodes);
        for node in &actual.nodes {
            assert_eq!(actual.parent(node.id), expected.parent(node.id));
        }
    }
}
