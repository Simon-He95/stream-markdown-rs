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
    use stream_markdown_rs::ParseError;

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
    parser.append("previous reply\n\n", &mut changes).unwrap();
    let before = changes.clone();

    assert!(matches!(
        parser.append("temporary text\n\n@broken!\n", &mut changes),
        Err(ParseError::Plugin(error)) if error.message.contains("executing WASM rule")
    ));
    assert_eq!(changes, before);
    assert!(matches!(
        parser.append("more", &mut changes),
        Err(ParseError::Finished)
    ));
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
