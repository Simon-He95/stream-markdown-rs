use stream_markdown_plugin_host::{Limits, WasmPlugin};
use stream_markdown_rs::{
    Block, BlockData, Change, InlineContent, InlineKind, Options, ParseError, Parser, Phase,
    PluginError, PluginField, PluginFieldValue, PluginRule, Rule, RuleInput, RuleResult, Scope,
    Span, parse_with_plugins,
};

const TYPED: &str = include_str!("fixtures/typed.wat");

const FIELDS: [PluginField; 3] = [
    PluginField {
        id: 5,
        value: PluginFieldValue::Text { start: 1, end: 4 },
    },
    PluginField {
        id: 7,
        value: PluginFieldValue::Integer(-42),
    },
    PluginField {
        id: 9,
        value: PluginFieldValue::Bool(true),
    },
];

fn rule(scope: Scope) -> Rule {
    Rule {
        trigger: b'@',
        scope,
        kind: 33,
    }
}

struct NativeRule {
    rule: Rule,
    fields: [PluginField; 3],
}

impl PluginRule for NativeRule {
    fn rule(&self) -> Rule {
        self.rule
    }

    fn step(&mut self, input: RuleInput<'_>) -> Result<RuleResult, PluginError> {
        if input.phase == Phase::Probe && !input.text.starts_with('@') {
            return Ok(RuleResult::Reject);
        }
        if let Some(end) = input.text.find('!') {
            Ok(RuleResult::AcceptWithFields {
                consumed: end + 1,
                fields: Box::new(self.fields),
            })
        } else {
            Ok(RuleResult::NeedMore {
                consumed: input.text.len(),
            })
        }
    }
}

#[test]
fn native_and_wasm_fields_are_preserved_in_batch_and_streaming_blocks_and_inlines() {
    let wasm = wat::parse_str(TYPED).unwrap();
    for scope in [Scope::Block, Scope::Inline] {
        let descriptor = rule(scope);
        let module = WasmPlugin::load(&wasm, descriptor, Limits::default()).unwrap();
        let chunks: &[&str] = match scope {
            Scope::Block => &["@中\n", "文!\n"],
            Scope::Inline => &["before @中", "文! after\n"],
        };
        let source = chunks.concat();
        for dynamic in [false, true] {
            let options = Options::default();
            let plugin = || -> Box<dyn PluginRule> {
                if dynamic {
                    Box::new(module.instantiate().unwrap())
                } else {
                    Box::new(NativeRule {
                        rule: descriptor,
                        fields: FIELDS,
                    })
                }
            };
            let document = parse_with_plugins(&source, &options, vec![plugin()]).unwrap();
            let expected = document.nodes.iter().find_map(typed_extension).unwrap();
            let expected_match = match scope {
                Scope::Block => "@中\n文!",
                Scope::Inline => "@中文!",
            };
            assert_eq!(&source[expected.0.start..expected.0.end], expected_match);
            assert_eq!(expected.1, FIELDS);
            assert_eq!(expected.1[0].value.as_text(expected_match), Some("中"));

            let mut parser = Parser::new(options);
            if dynamic {
                parser.add_plugin(module.instantiate().unwrap()).unwrap();
            } else {
                parser
                    .add_plugin(NativeRule {
                        rule: descriptor,
                        fields: FIELDS,
                    })
                    .unwrap();
            }
            let mut changes = Vec::new();
            for chunk in chunks {
                parser.append(chunk, &mut changes).unwrap();
            }
            parser.finish(&mut changes).unwrap();
            let streamed = changes.iter().rev().find_map(|change| match change {
                Change::Insert { block, .. } | Change::Replace(block) => typed_extension(block),
                _ => None,
            });
            assert_eq!(streamed, Some(expected));
        }
    }
}

fn typed_extension(block: &Block) -> Option<(Span, &[PluginField])> {
    match &block.data {
        BlockData::ExtensionWithFields { kind: 33, fields } => Some((block.span, fields)),
        BlockData::Paragraph(InlineContent::Parsed(inlines)) => {
            inlines.iter().find_map(|inline| match &inline.kind {
                InlineKind::ExtensionWithFields { kind: 33, fields } => {
                    Some((inline.span, fields.as_ref()))
                }
                _ => None,
            })
        }
        _ => None,
    }
}

#[test]
fn rejecting_a_mid_codepoint_text_field_leaves_prior_stream_output_intact() {
    let bad_offset = r"\05\00\00\00\02\00\00\00\04\00\00\00";
    let original_offset = r"\05\00\00\00\01\00\00\00\04\00\00\00";
    let wasm = wat::parse_str(TYPED.replace(original_offset, bad_offset)).unwrap();
    let module = WasmPlugin::load(&wasm, rule(Scope::Block), Limits::default()).unwrap();
    let mut parser = Parser::new(Options::default());
    parser.add_plugin(module.instantiate().unwrap()).unwrap();
    let mut changes = Vec::new();
    parser.append("prior text\n\n@中\n", &mut changes).unwrap();
    let prior = changes.clone();
    assert!(matches!(
        parser.append("文!\n", &mut changes),
        Err(ParseError::Plugin(error)) if error.message.contains("field 5") && error.message.contains("UTF-8")
    ));
    assert_eq!(changes, prior);
    assert!(matches!(
        parser.append("more", &mut changes),
        Err(ParseError::Finished)
    ));
}
