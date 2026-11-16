use stream_markdown_plugin_host::{Limits, WasmPlugin};
use stream_markdown_rs::{Phase, PluginRule, Rule, RuleInput, RuleResult, Scope};

const DELIMITED: &str = include_str!("fixtures/delimited.wat");

fn descriptor(scope: Scope) -> Rule {
    Rule {
        trigger: b'@',
        scope,
        kind: 7,
    }
}

fn input(phase: Phase, text: &str) -> RuleInput<'_> {
    RuleInput { phase, text }
}

#[test]
fn a_compiled_module_runs_a_block_or_inline_rule() {
    let wasm = wat::parse_str(DELIMITED).unwrap();
    for scope in [Scope::Block, Scope::Inline] {
        let module = WasmPlugin::load(&wasm, descriptor(scope), Limits::default()).unwrap();
        let mut guest = module.instantiate().unwrap();
        assert_eq!(guest.rule().scope, scope);
        assert_eq!(guest.max_input_bytes(), Limits::default().max_input_bytes);

        assert!(matches!(
            guest.step(input(Phase::Probe, "@中文! tail")).unwrap(),
            RuleResult::Accept { consumed: 8 }
        ));
        assert!(matches!(
            guest.step(input(Phase::Probe, "ordinary text")).unwrap(),
            RuleResult::Reject
        ));
    }
}

#[test]
fn streaming_consumes_new_bytes_and_isolates_sessions() {
    let wasm = wat::parse_str(DELIMITED).unwrap();
    let module = WasmPlugin::load(&wasm, descriptor(Scope::Block), Limits::default()).unwrap();
    let mut a = module.instantiate().unwrap();
    let mut b = module.instantiate().unwrap();

    assert!(matches!(
        a.step(input(Phase::Probe, "@中")).unwrap(),
        RuleResult::NeedMore { consumed: 4 }
    ));
    assert!(matches!(
        b.step(input(Phase::Advance, "!")).unwrap(),
        RuleResult::Reject
    ));
    assert!(matches!(
        a.step(input(Phase::Advance, "文! extra")).unwrap(),
        RuleResult::Accept { consumed: 4 }
    ));
    assert!(matches!(
        b.step(input(Phase::Probe, "@open")).unwrap(),
        RuleResult::NeedMore { consumed: 5 }
    ));
    assert!(matches!(
        b.step(input(Phase::Finish, "")).unwrap(),
        RuleResult::NeedMore { consumed: 0 }
    ));
}

#[test]
fn loading_rejects_host_imports_and_limits_module_bytes() {
    let imported = wat::parse_str(r#"(module (import "host" "fs" (func)))"#).unwrap();
    let result = WasmPlugin::load(&imported, descriptor(Scope::Inline), Limits::default());
    assert!(matches!(result, Err(error) if error.message.contains("cannot import")));

    let limits = Limits {
        max_module_bytes: imported.len() - 1,
        ..Limits::default()
    };
    let result = WasmPlugin::load(&imported, descriptor(Scope::Inline), limits);
    assert!(matches!(result, Err(error) if error.message.contains("byte limit")));
}

#[test]
fn instantiation_enforces_version_memory_and_buffer_limits() {
    let wrong_version = wat::parse_str(DELIMITED.replace("i32.const 1)", "i32.const 9)")).unwrap();
    let plugin =
        WasmPlugin::load(&wrong_version, descriptor(Scope::Block), Limits::default()).unwrap();
    assert!(matches!(plugin.instantiate(), Err(error) if error.message.contains("ABI version 9")));

    let oversized =
        wat::parse_str(DELIMITED.replace("(export \"memory\") 1", "(export \"memory\") 33"))
            .unwrap();
    let plugin = WasmPlugin::load(&oversized, descriptor(Scope::Block), Limits::default()).unwrap();
    assert!(plugin.instantiate().is_err());

    let bad_buffer = wat::parse_str(DELIMITED.replace(
        "(param i32) (result i32)\n    i32.const 0)",
        "(param i32) (result i32)\n    i32.const 65500)",
    ))
    .unwrap();
    let plugin =
        WasmPlugin::load(&bad_buffer, descriptor(Scope::Block), Limits::default()).unwrap();
    assert!(
        matches!(plugin.instantiate(), Err(error) if error.message.contains("outside linear memory"))
    );
}

#[test]
fn a_rule_cannot_return_an_out_of_range_or_non_utf8_boundary() {
    let arbitrary_length = r#"
        (func (export "ms_step") (param i32 i32 i32) (result i64)
            (i64.const 8589934594)))"#;
    let base = DELIMITED
        .split("(func (export \"ms_step\")")
        .next()
        .unwrap();
    let wasm = wat::parse_str(format!("{base}{arbitrary_length}")).unwrap();
    let plugin = WasmPlugin::load(&wasm, descriptor(Scope::Inline), Limits::default()).unwrap();
    for text in ["@", "@中"] {
        let mut rule = plugin.instantiate().unwrap();
        assert!(matches!(
            rule.step(input(Phase::Probe, text)),
            Err(error) if error.message.contains("invalid UTF-8 boundary")
        ));
        assert!(matches!(
            rule.step(input(Phase::Probe, "@a")),
            Err(error) if error.message.contains("instance has failed")
        ));
    }
}

#[test]
fn a_rule_cannot_execute_beyond_its_fuel_limit() {
    let endless_step = r#"
        (func (export "ms_step") (param i32 i32 i32) (result i64)
            (loop $forever (br $forever)) (i64.const 0)))"#;
    let base = DELIMITED
        .split("(func (export \"ms_step\")")
        .next()
        .unwrap();
    let wasm = wat::parse_str(format!("{base}{endless_step}")).unwrap();
    let limits = Limits {
        fuel_per_call: 100,
        ..Limits::default()
    };
    let plugin = WasmPlugin::load(&wasm, descriptor(Scope::Block), limits).unwrap();
    let mut rule = plugin.instantiate().unwrap();
    assert!(matches!(
        rule.step(input(Phase::Probe, "@")),
        Err(error) if error.message.contains("fuel")
    ));
}
