# WASM parser rules

`stream-markdown-plugin-host` is optional. Applications load a module at runtime, bind it to a core `Rule`, and instantiate it for a parser session. A compiled `WasmPlugin` can supply independent instances for multiple documents. The base parser retains its native dispatch when no WASM rules are registered.

```rust,no_run
use stream_markdown_plugin_host::{Limits, WasmPlugin};
use stream_markdown_rs::{Options, ParseError, Parser, PluginError, Rule, Scope};

fn load(bytes: &[u8]) -> Result<WasmPlugin, PluginError> {
    WasmPlugin::load(
        bytes,
        Rule { trigger: b'@', scope: Scope::Block, kind: 7 },
        Limits::default(),
    )
}

fn parser(module: &WasmPlugin) -> Result<Parser, ParseError> {
    let mut parser = Parser::new(Options::default());
    parser.add_plugin(module.instantiate()?)?;
    Ok(parser)
}
```

The application provides the non-whitespace ASCII trigger, the block or inline scope, and the numeric extension kind. The core decides which candidate positions are valid in the current Markdown context. The accepted original source range becomes the extension payload. ABI v2 rules can also return typed fields. The application interprets field IDs according to the extension kind.

## Guest ABI version 1

A module exports one wasm32 linear memory named `memory` and these functions.

| Export | Signature | Behavior |
| --- | --- | --- |
| `ms_abi_version` | `() -> i32` | Return `1`. |
| `ms_reserve_input` | `(capacity: i32) -> i32` | Reserve the requested byte capacity and return its linear-memory offset. The reservation remains valid for the instance lifetime. |
| `ms_step` | `(phase: i32, ptr: i32, len: i32) -> i64` | Read exactly the UTF-8 slice at `ptr` of length `len`. Return the disposition in the high 32 bits and consumed byte count in the low 32 bits. |

The host copies only the current input window into the reserved buffer. Guest code can keep progress in its mutable globals or memory. The next call may overwrite the entire input buffer, including data from previous calls.

| Phase | Code | Input |
| --- | ---: | --- |
| Probe | 0 | Candidate starts at the configured trigger. Begin fresh rule state. |
| Advance | 1 | Input continues at the previous unconsumed source offset. |
| Finish | 2 | The document or the rule's containing block scope ends. The window may be empty. |

| Disposition | Code | Consumed count |
| --- | ---: | --- |
| Reject | 0 | Zero. The core handles the span with ordinary Markdown syntax. |
| NeedMore | 1 | The bytes the rule has consumed. Zero keeps this window pending until more input is available. |
| Accept | 2 | Bytes consumed from this window through the match. Probe must consume at least one byte. Advance and Finish may consume zero if a prior call already advanced the rule. |

All counts are relative to the current window and must land on UTF-8 character boundaries. A rule can accept a prefix and leave the rest for the Markdown core. Compiling the same rule for static and incremental input should produce the same accepted source with different chunk boundaries.

Block rules run on complete Markdown lines, including the last line at EOF. A rule that asks for more input owns its candidate until it accepts, rejects, or receives Finish. The parser emits an extension when the rule accepts. If the rule rejects after Advance or still needs more input at Finish, the parser replays the buffered candidate with ordinary Markdown block rules.

Losing the enclosing quote or list prefix finishes a pending block rule before the next out-of-scope line. The accepted extension keeps its original parent, and the next line goes through native block parsing. Inline rules process the source-backed text windows that Markdown assigns to them.

## Guest ABI version 2

Return `2` from `ms_abi_version` and export `ms_fields() -> i64`. Existing phase codes, disposition codes and input buffer rules carry over. Use disposition `3` in `ms_step` to accept with fields. The consumed byte count stays relative to the current window. When the host sees disposition `3`, it calls `ms_fields` with the remaining fuel budget for the step. Return the record count in the high 32 bits and the wasm32 memory offset in the low 32 bits.

Each 12-byte field record uses little-endian encoding.

| Offset | Size | Meaning |
| ---: | ---: | --- |
| 0 | 2 | Unsigned field ID |
| 2 | 1 | Type code |
| 3 | 1 | Reserved, zero |
| 4 | 8 | Type-specific payload |

Type `0` is text. Its payload consists of unsigned 32-bit start and end offsets relative to the **entire accepted match**, with the end excluded. Text can point into an earlier input window. The parser validates UTF-8 boundaries against the matched source, and consumers can borrow the text through `PluginFieldValue::as_text(matched)`. Type `1` is a signed 64-bit integer. Type `2` is a boolean stored as the 64-bit integer `0` or `1`.

The host accepts up to 1,024 records per match and validates the full record table against guest memory. Disposition `2` remains available on either ABI and keeps the original no-fields output without calling `ms_fields`. The [typed fixture](tests/fixtures/typed.wat) accepts a span containing a Unicode character and emits one of each field type.

## Limits and failure

The host limits module bytes before compilation, uses one memory and one table per instance, and applies the configured memory and instruction budgets. Each version, reserve, and step call gets an independent fuel budget. Modules use only local linear memory and have no imports or start function. ABI version and buffer bounds are checked on instantiation before registration.

A trap or invalid result terminates the rule instance and returns a core `PluginError` to the caller. The core owns document transaction handling and emitted node IDs. The fixture in [delimited.wat](tests/fixtures/delimited.wat) exercises the ABI with a rule that accepts a range from `@` through `!`, including multiple input calls.
