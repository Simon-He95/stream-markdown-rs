use std::{hint::black_box, time::Instant};

use stream_markdown_plugin_host::{Limits, WasmPlugin};
use stream_markdown_rs::{
    BlockData, Change, Options, Parser, Phase, PluginError, PluginRule, Rule, RuleInput,
    RuleResult, Scope,
};

const RULE: Rule = Rule {
    trigger: b'@',
    scope: Scope::Block,
    kind: 7,
};
const PACKETS: [&str; 2] = ["@中文\n", "more!\n"];
const ROUNDS: usize = 2000;

#[derive(Default)]
struct NativeDelimited {
    open: bool,
}

impl PluginRule for NativeDelimited {
    fn rule(&self) -> Rule {
        RULE
    }

    fn step(&mut self, input: RuleInput<'_>) -> Result<RuleResult, PluginError> {
        if input.phase == Phase::Probe {
            self.open = input.text.starts_with('@');
        }
        if !self.open {
            return Ok(RuleResult::Reject);
        }
        if let Some(end) = input.text.find('!') {
            self.open = false;
            return Ok(RuleResult::Accept { consumed: end + 1 });
        }
        Ok(RuleResult::NeedMore {
            consumed: input.text.len(),
        })
    }
}

fn append_pair(plugin: impl PluginRule + 'static) -> (u128, Vec<Change>) {
    let mut parser = Parser::new(Options::default());
    parser.add_plugin(plugin).unwrap();
    let mut changes = Vec::new();
    let start = Instant::now();
    for packet in PACKETS {
        parser.append(black_box(packet), &mut changes).unwrap();
    }
    let elapsed = start.elapsed().as_nanos();
    black_box(&changes);
    (elapsed, changes)
}

fn median(mut samples: Vec<u128>) -> u128 {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn main() {
    let bytes = wat::parse_str(include_str!("../tests/fixtures/delimited.wat")).unwrap();
    let module = WasmPlugin::load(&bytes, RULE, Limits::default()).unwrap();
    let (_, expected) = append_pair(NativeDelimited::default());
    assert!(expected.iter().any(|change| matches!(change,
        Change::Insert { block, .. } if matches!(block.data, BlockData::Extension { kind: 7 })
    )));
    assert_eq!(append_pair(module.instantiate().unwrap()).1, expected);

    let mut loads = Vec::with_capacity(ROUNDS);
    let mut instantiations = Vec::with_capacity(ROUNDS);
    let mut wasm_appends = Vec::with_capacity(ROUNDS);
    let mut native_appends = Vec::with_capacity(ROUNDS);
    for trial in 0..ROUNDS + 100 {
        let start = Instant::now();
        let module = WasmPlugin::load(black_box(&bytes), RULE, Limits::default()).unwrap();
        let load = start.elapsed().as_nanos();
        let start = Instant::now();
        let guest = module.instantiate().unwrap();
        let instantiate = start.elapsed().as_nanos();
        let (wasm_append, native_append) = if trial % 2 == 0 {
            (
                append_pair(guest).0,
                append_pair(NativeDelimited::default()).0,
            )
        } else {
            let native = append_pair(NativeDelimited::default()).0;
            (append_pair(guest).0, native)
        };
        if trial >= 100 {
            loads.push(load);
            instantiations.push(instantiate);
            wasm_appends.push(wasm_append);
            native_appends.push(native_append);
        }
    }

    println!("fixture_bytes={} rounds={ROUNDS}", bytes.len());
    println!("load_compile_median_ns={}", median(loads));
    println!("instantiate_median_ns={}", median(instantiations));
    println!("two_appends_with_wasm_median_ns={}", median(wasm_appends));
    println!(
        "two_appends_with_native_rule_median_ns={}",
        median(native_appends)
    );
}
