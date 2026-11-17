use stream_markdown_plugin_host::{Limits, WasmPlugin};
use stream_markdown_rs::{
    BlockData, BlockStatus, Change, Options, Parser, Rule, Scope, parse_with_plugins,
};

#[test]
fn a_wasm_block_is_parseable_in_batch_and_across_chunks() {
    let wasm = wat::parse_str(include_str!("fixtures/delimited.wat")).unwrap();
    let module = WasmPlugin::load(
        &wasm,
        Rule {
            trigger: b'@',
            scope: Scope::Block,
            kind: 7,
        },
        Limits::default(),
    )
    .unwrap();
    let options = Options::default();
    let source = "@中文\n正文!\n\nordinary text\n";
    let batch = parse_with_plugins(
        source,
        &options,
        vec![Box::new(module.instantiate().unwrap())],
    )
    .unwrap();

    assert!(matches!(
        batch.nodes[0].data,
        BlockData::Extension { kind: 7 }
    ));
    assert_eq!(batch.nodes[0].status, BlockStatus::Closed);
    assert_eq!(
        &source[batch.nodes[0].span.start..batch.nodes[0].span.end],
        "@中文\n正文!"
    );

    let mut parser = Parser::new(options);
    parser.add_plugin(module.instantiate().unwrap()).unwrap();
    let mut changes = Vec::new();
    for chunk in ["@中", "文\n", "正", "文!\n\nordinary text", "\n"] {
        parser.append(chunk, &mut changes).unwrap();
    }
    parser.finish(&mut changes).unwrap();

    let mut extension = changes.iter().filter_map(|change| match change {
        Change::Insert { block, .. } | Change::Replace(block)
            if matches!(block.data, BlockData::Extension { kind: 7 }) =>
        {
            Some(block)
        }
        _ => None,
    });
    assert_eq!(extension.next_back(), Some(&batch.nodes[0]));
}

#[test]
fn a_wasm_inline_rule_respects_code_and_escape_boundaries() {
    use stream_markdown_rs::{InlineContent, InlineKind};

    let wasm = wat::parse_str(include_str!("fixtures/delimited.wat")).unwrap();
    let module = WasmPlugin::load(
        &wasm,
        Rule {
            trigger: b'@',
            scope: Scope::Inline,
            kind: 11,
        },
        Limits::default(),
    )
    .unwrap();
    let options = Options::default();
    let source = "before @中文! and `@literal!` then \\@escaped!\n";
    let batch = parse_with_plugins(
        source,
        &options,
        vec![Box::new(module.instantiate().unwrap())],
    )
    .unwrap();
    let BlockData::Paragraph(InlineContent::Parsed(inlines)) = &batch.nodes[0].data else {
        panic!("expected parsed paragraph content");
    };
    let matched: Vec<_> = inlines
        .iter()
        .filter_map(|inline| match inline.kind {
            InlineKind::Extension(11) => Some(&source[inline.span.start..inline.span.end]),
            _ => None,
        })
        .collect();
    assert_eq!(matched, ["@中文!"]);

    let mut parser = Parser::new(options);
    parser.add_plugin(module.instantiate().unwrap()).unwrap();
    let mut changes = Vec::new();
    for chunk in [
        "before @中",
        "文! and `@lit",
        "eral!` then \\",
        "@escaped!\n",
    ] {
        parser.append(chunk, &mut changes).unwrap();
    }
    parser.finish(&mut changes).unwrap();

    let streamed_content = changes.iter().rev().find_map(|change| match change {
        Change::Insert { block, .. } | Change::Replace(block) => match &block.data {
            BlockData::Paragraph(content) => Some(content),
            _ => None,
        },
        _ => None,
    });
    assert_eq!(
        streamed_content,
        match &batch.nodes[0].data {
            BlockData::Paragraph(content) => Some(content),
            _ => None,
        }
    );
}

#[test]
fn a_wasm_block_advances_across_multiple_bounded_windows_on_one_line() {
    let wasm = wat::parse_str(include_str!("fixtures/delimited.wat")).unwrap();
    let module = WasmPlugin::load(
        &wasm,
        Rule {
            trigger: b'@',
            scope: Scope::Block,
            kind: 7,
        },
        Limits {
            max_input_bytes: 4,
            ..Limits::default()
        },
    )
    .unwrap();
    let options = Options::default();
    let source = "@甲乙丙丁!\n";
    let document = parse_with_plugins(
        source,
        &options,
        vec![Box::new(module.instantiate().unwrap())],
    )
    .unwrap();
    let node = &document.nodes[0];
    assert!(matches!(node.data, BlockData::Extension { kind: 7 }));
    assert_eq!(&source[node.span.start..node.span.end], "@甲乙丙丁!");
}

#[test]
fn a_trapping_wasm_rule_discards_the_failed_append_and_stops_the_session() {
    use stream_markdown_rs::{BorrowedParser, ParseError, SourceBuffer};

    let wasm = wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (func (export "ms_abi_version") (result i32) (i32.const 1))
            (func (export "ms_reserve_input") (param i32) (result i32) (i32.const 0))
            (func (export "ms_step") (param i32 i32 i32) (result i64) (unreachable)))"#,
    )
    .unwrap();
    let module = WasmPlugin::load(
        &wasm,
        Rule {
            trigger: b'@',
            scope: Scope::Block,
            kind: 7,
        },
        Limits::default(),
    )
    .unwrap();
    let mut parser = Parser::new(Options::default());
    parser.add_plugin(module.instantiate().unwrap()).unwrap();
    let mut changes = Vec::new();
    let prior = "previous reply\n\n";
    let failed = "temporary text\n\n@broken!\n";
    parser.append(prior, &mut changes).unwrap();
    let before = changes.clone();

    assert!(matches!(
        parser.append(failed, &mut changes),
        Err(ParseError::Plugin(error)) if error.message.contains("executing WASM rule")
    ));
    assert_eq!(changes, before);
    assert!(matches!(
        parser.append("more", &mut changes),
        Err(ParseError::Finished)
    ));

    let mut source = SourceBuffer::new(prior.to_owned());
    let mut borrowed = BorrowedParser::new(&source, Options::default());
    borrowed.add_plugin(module.instantiate().unwrap()).unwrap();
    changes.clear();
    borrowed.append(&source, prior.len(), &mut changes).unwrap();
    assert_eq!(changes, before);

    source.push_str(failed);
    let end = source.as_str().len();
    assert!(matches!(
        borrowed.append(&source, end, &mut changes),
        Err(ParseError::Plugin(error)) if error.message.contains("executing WASM rule")
    ));
    assert_eq!(changes, before);
    assert_eq!(source.as_str(), format!("{prior}{failed}"));
    assert_eq!(borrowed.metrics().input_bytes, end);
    assert_eq!(
        borrowed.append(&source, end, &mut changes),
        Err(ParseError::Finished)
    );
}

#[test]
fn a_rule_cannot_keep_an_unconsumed_stream_window_open_forever() {
    use stream_markdown_rs::ParseError;

    let wasm = wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (func (export "ms_abi_version") (result i32) (i32.const 1))
            (func (export "ms_reserve_input") (param i32) (result i32) (i32.const 0))
            (func (export "ms_step") (param i32 i32 i32) (result i64)
                (i64.const 4294967296)))"#,
    )
    .unwrap();
    let module = WasmPlugin::load(
        &wasm,
        Rule {
            trigger: b'@',
            scope: Scope::Block,
            kind: 7,
        },
        Limits {
            max_input_bytes: 4,
            ..Limits::default()
        },
    )
    .unwrap();

    let mut parser = Parser::new(Options::default());
    parser.add_plugin(module.instantiate().unwrap()).unwrap();
    let mut changes = Vec::new();
    parser.append("@x\n", &mut changes).unwrap();
    parser.append("中", &mut changes).unwrap();

    assert!(matches!(
        parser.append("\n", &mut changes),
        Err(ParseError::Plugin(error)) if error.message.contains("bounded window")
    ));
}

#[test]
fn delayed_wasm_rejection_replays_native_block_boundaries() {
    use stream_markdown_rs::parse;

    let wasm = wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (func (export "ms_abi_version") (result i32) (i32.const 1))
            (func (export "ms_reserve_input") (param i32) (result i32) (i32.const 0))
            (func (export "ms_step") (param $phase i32) (param i32) (param $len i32) (result i64)
                (if (result i64) (i32.eqz (local.get $phase))
                    (then (i64.or (i64.const 4294967296) (i64.extend_i32_u (local.get $len))))
                    (else (i64.const 0)))))"#,
    )
    .unwrap();
    let module = WasmPlugin::load(
        &wasm,
        Rule {
            trigger: b'@',
            scope: Scope::Block,
            kind: 7,
        },
        Limits::default(),
    )
    .unwrap();
    let options = Options::default();
    let source = "@intro\r\n# Heading\r\n\r\nTail.";
    let expected = parse(source, &options).unwrap();
    let actual = parse_with_plugins(
        source,
        &options,
        vec![Box::new(module.instantiate().unwrap())],
    )
    .unwrap();
    assert_eq!(actual.nodes, expected.nodes);

    let mut parser = Parser::new(options);
    parser.add_plugin(module.instantiate().unwrap()).unwrap();
    let mut changes = Vec::new();
    parser.append("@intro\r", &mut changes).unwrap();
    parser.append("\n# Heading\r", &mut changes).unwrap();
    assert!(changes.is_empty());
    parser.append("\n\r\nTail.", &mut changes).unwrap();
    parser.finish(&mut changes).unwrap();
    let blocks = changes
        .iter()
        .filter_map(|change| match change {
            Change::Insert { block, .. } | Change::Replace(block)
                if block.status == BlockStatus::Closed =>
            {
                Some(block)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(blocks, expected.nodes.iter().collect::<Vec<_>>());
}

#[test]
fn finish_calls_the_wasm_eof_phase_and_commits_consumed_input() {
    let wasm = wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (func (export "ms_abi_version") (result i32) (i32.const 1))
            (func (export "ms_reserve_input") (param i32) (result i32) (i32.const 0))
            (func (export "ms_step") (param $phase i32) (param i32) (param $len i32) (result i64)
                (i64.or (i64.extend_i32_u (local.get $len))
                    (if (result i64) (i32.eq (local.get $phase) (i32.const 2))
                        (then (i64.const 8589934592))
                        (else (i64.const 4294967296))))))"#,
    )
    .unwrap();
    let module = WasmPlugin::load(
        &wasm,
        Rule {
            trigger: b'@',
            scope: Scope::Block,
            kind: 7,
        },
        Limits::default(),
    )
    .unwrap();
    let options = Options::default();
    let source = "@首行\r\n尾部";
    let document = parse_with_plugins(
        source,
        &options,
        vec![Box::new(module.instantiate().unwrap())],
    )
    .unwrap();
    assert_eq!(document.nodes.len(), 1);
    assert_eq!(document.nodes[0].status, BlockStatus::Closed);
    assert!(matches!(
        document.nodes[0].data,
        BlockData::Extension { kind: 7 }
    ));
    assert_eq!(document.nodes[0].span.end, source.len());

    let mut parser = Parser::new(options);
    parser.add_plugin(module.instantiate().unwrap()).unwrap();
    let mut changes = Vec::new();
    for chunk in ["@首行\r", "\n", "尾", "部"] {
        parser.append(chunk, &mut changes).unwrap();
    }
    assert!(changes.is_empty());
    parser.finish(&mut changes).unwrap();
    assert!(
        matches!(&changes[..], [Change::Insert { block, parent: None }] if *block == document.nodes[0])
    );

    let quoted = "> @first\n> continued\n\n# Outside\n";
    let document = parse_with_plugins(
        quoted,
        &Options::default(),
        vec![Box::new(module.instantiate().unwrap())],
    )
    .unwrap();
    assert_eq!(document.nodes.len(), 3);
    let [quote, extension, heading] = &document.nodes[..] else {
        unreachable!()
    };
    assert!(matches!(quote.data, BlockData::Quote));
    assert!(matches!(extension.data, BlockData::Extension { kind: 7 }));
    assert_eq!(document.parent(extension.id), Some(quote.id));
    assert_eq!(
        &quoted[extension.span.start..extension.span.end],
        "@first\n> continued\n"
    );
    assert!(matches!(heading.data, BlockData::Heading { level: 1, .. }));

    let mut parser = Parser::new(Options::default());
    parser.add_plugin(module.instantiate().unwrap()).unwrap();
    let mut changes = Vec::new();
    parser
        .append("> @first\n> continued\n", &mut changes)
        .unwrap();
    assert!(!changes.iter().any(|change| matches!(change,
        Change::Insert { block, .. } if matches!(block.data, BlockData::Extension { .. })
    )));
    parser.append("\n# Outside\n", &mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(change,
        Change::Insert { block, parent: Some(parent) } if block == extension && *parent == quote.id
    )));
    parser.finish(&mut changes).unwrap();
    assert!(changes.iter().any(|change| matches!(change,
        Change::Insert { block, parent: None } if block == heading
    )));
}

#[test]
fn wasm_inline_plugins_descend_formatting_and_explicit_link_labels() {
    use stream_markdown_rs::{InlineContent, InlineKind};

    let wasm = wat::parse_str(include_str!("fixtures/delimited.wat")).unwrap();
    let module = WasmPlugin::load(
        &wasm,
        Rule {
            trigger: b'@',
            scope: Scope::Inline,
            kind: 11,
        },
        Limits::default(),
    )
    .unwrap();
    let options = Options::default();
    let source = "**`@code!` \\@escaped! @中文!** [@label!](https://example.com/@dest!) <https://example.com/@auto!> ![@alt!](/x)\n";
    let batch = parse_with_plugins(
        source,
        &options,
        vec![Box::new(module.instantiate().unwrap())],
    )
    .unwrap();
    let BlockData::Paragraph(InlineContent::Parsed(nodes)) = &batch.nodes[0].data else {
        panic!("expected parsed paragraph");
    };
    let InlineKind::Strong(styled) = &nodes[0].kind else {
        panic!("expected a strong run");
    };
    assert!(matches!(&styled[0].kind, InlineKind::Code(code) if code.as_str(source) == "@code!"));
    let escaped = styled
        .iter()
        .find(|node| {
            matches!(&node.kind,
                InlineKind::Text(text) if text.as_str(source).contains("@escaped!")
            )
        })
        .unwrap();
    assert!(source[escaped.span.start..escaped.span.end].starts_with("\\@escaped!"));
    let nested = styled
        .iter()
        .find(|node| matches!(node.kind, InlineKind::Extension(11)))
        .unwrap();
    assert_eq!(&source[nested.span.start..nested.span.end], "@中文!");

    let links = nodes
        .iter()
        .filter_map(|node| match &node.kind {
            InlineKind::Link(link) => Some(link.as_ref()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(links.len(), 2);
    assert_eq!(
        links[0].destination.as_str(source),
        "https://example.com/@dest!"
    );
    assert!(matches!(
        &links[0].children[0].kind,
        InlineKind::Extension(11)
    ));
    assert!(matches!(&links[1].children[0].kind, InlineKind::Text(_)));
    let image = nodes
        .iter()
        .find_map(|node| match &node.kind {
            InlineKind::Image(image) => Some(image),
            _ => None,
        })
        .unwrap();
    assert!(
        matches!(&image.children[0].kind, InlineKind::Text(text) if text.as_str(source) == "@alt!")
    );

    let mut parser = Parser::new(options);
    parser.add_plugin(module.instantiate().unwrap()).unwrap();
    let mut changes = Vec::new();
    for (start, ch) in source.char_indices() {
        parser
            .append(&source[start..start + ch.len_utf8()], &mut changes)
            .unwrap();
    }
    parser.finish(&mut changes).unwrap();
    let streamed = changes.iter().rev().find_map(|change| match change {
        Change::Insert { block, .. } | Change::Replace(block)
            if block.status == BlockStatus::Closed =>
        {
            Some(block)
        }
        _ => None,
    });
    assert_eq!(streamed, Some(&batch.nodes[0]));
}
