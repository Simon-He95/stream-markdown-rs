use stream_markdown_rs::{BorrowedParser, Options, ParseError, Parser, Preview, SourceBuffer};

#[test]
fn borrowed_prefixes_emit_the_same_updates_as_owned_chunks() {
    let source = format!(
        "# Heading\r\n\r\n- 中文 `code`\n  - [label][late]\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\n<tool-call name=\"search\">内容</tool-call>\n\n$x+y$ and **draft**\n\n[late]: /docs\n\n```rust\n{}```\n",
        "let value = 42;\n".repeat(300)
    );
    let boundaries: Vec<_> = source
        .char_indices()
        .map(|(at, _)| at)
        .chain([source.len()])
        .collect();
    let extended = Options {
        preview: Preview::Ai,
        markstream: true,
        math: true,
        footnotes: true,
        custom_tags: vec!["tool-call".into()],
        ..Options::default()
    };
    for options in [Options::default(), extended] {
        for width in [1, 5, 19, 4096] {
            let mut owned = Parser::new(options.clone());
            let mut buffer = SourceBuffer::new(String::new());
            let other = SourceBuffer::new(String::new());
            let mut borrowed = BorrowedParser::new(&buffer, options.clone());
            let mut old_changes = Vec::new();
            let mut new_changes = Vec::new();
            for (packet, parts) in boundaries.windows(2).step_by(width).enumerate() {
                let start = parts[0];
                let next = ((packet + 1) * width).min(boundaries.len() - 1);
                let end = boundaries[next];
                owned.append(&source[start..end], &mut old_changes).unwrap();
                buffer.push_str(&source[start..end]);
                borrowed.append(&buffer, end, &mut new_changes).unwrap();
                assert_eq!(new_changes, old_changes, "width {width}, end {end}");
                assert_eq!(
                    borrowed.append(&buffer, 0, &mut new_changes),
                    Err(ParseError::SourceShrunk)
                );
                old_changes.clear();
                new_changes.clear();
            }
            assert_eq!(
                borrowed.append(&other, 0, &mut new_changes),
                Err(ParseError::DifferentSource)
            );
            assert_eq!(
                borrowed.append(&buffer, source.len() + 1, &mut new_changes),
                Err(ParseError::InvalidSourceEnd)
            );
            let inside_utf8 = source.find('中').unwrap() + 1;
            assert_eq!(
                borrowed.append(&buffer, inside_utf8, &mut new_changes),
                Err(ParseError::InvalidSourceEnd)
            );
            owned.finish(&mut old_changes).unwrap();
            borrowed
                .finish(&buffer, source.len(), &mut new_changes)
                .unwrap();
            assert_eq!(new_changes, old_changes, "finish width {width}");
            assert_eq!(borrowed.metrics(), owned.metrics());
        }
    }
}

#[test]
fn borrowed_finish_can_consume_the_remaining_visible_prefix_after_a_rejected_end() {
    let source = "# 节点\n\n正文";
    let mut buffer = SourceBuffer::new(source.to_owned());
    let mut borrowed = BorrowedParser::new(&buffer, Options::default());
    let mut owned = Parser::new(Options::default());
    let known_end = source.find('\n').unwrap() + 1;
    let mut new_changes = Vec::new();
    borrowed
        .append(&buffer, known_end, &mut new_changes)
        .unwrap();
    let before_failure = new_changes.clone();
    assert_eq!(
        borrowed.finish(&buffer, 0, &mut new_changes),
        Err(ParseError::SourceShrunk)
    );
    assert_eq!(new_changes, before_failure);
    borrowed
        .finish(&buffer, source.len(), &mut new_changes)
        .unwrap();
    assert_eq!(borrowed.metrics().input_bytes, source.len());
    assert_eq!(borrowed.metrics().scanned_bytes, source.len());

    let mut old_changes = Vec::new();
    owned
        .append(&source[..known_end], &mut old_changes)
        .unwrap();
    assert_eq!(before_failure, old_changes);
    assert_eq!(new_changes.len(), old_changes.len() + 1);
    assert!(
        matches!(new_changes.last(), Some(stream_markdown_rs::Change::Insert { block, .. })
        if block.status == stream_markdown_rs::BlockStatus::Closed)
    );

    buffer.push_str("\n");
    assert_eq!(
        borrowed.append(&buffer, buffer.as_str().len(), &mut new_changes),
        Err(ParseError::Finished)
    );
    assert_eq!(buffer.into_string(), format!("{source}\n"));
}
