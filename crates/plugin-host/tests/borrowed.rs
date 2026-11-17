use stream_markdown_plugin_host::{Limits, WasmPlugin};
use stream_markdown_rs::{
    BlockData, BorrowedParser, Change, Options, Parser, Rule, Scope, SourceBuffer,
};

#[test]
fn runtime_plugin_deltas_match_when_the_tui_owns_source() {
    let bytes = wat::parse_str(include_str!("fixtures/delimited.wat")).unwrap();
    let rule = Rule {
        trigger: b'@',
        scope: Scope::Block,
        kind: 7,
    };
    let module = WasmPlugin::load(&bytes, rule, Limits::default()).unwrap();
    let mut source = SourceBuffer::new(String::new());
    let mut owned = Parser::new(Options::default());
    let mut borrowed = BorrowedParser::new(&source, Options::default());
    owned.add_plugin(module.instantiate().unwrap()).unwrap();
    borrowed.add_plugin(module.instantiate().unwrap()).unwrap();
    let mut a = Vec::new();
    let mut b = Vec::new();
    for packet in ["@中文\n", "second", "!\n"] {
        source.push_str(packet);
        owned.append(packet, &mut a).unwrap();
        borrowed
            .append(&source, source.as_str().len(), &mut b)
            .unwrap();
        assert_eq!(a, b);
    }
    assert!(
        a.iter()
            .any(|change| matches!(change, Change::Insert { block, .. }
        if matches!(block.data, BlockData::Extension { kind: 7 })))
    );
    owned.finish(&mut a).unwrap();
    borrowed
        .finish(&source, source.as_str().len(), &mut b)
        .unwrap();
    assert_eq!(a, b);
}
