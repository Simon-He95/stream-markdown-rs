# stream-markdown-rs

Incremental CommonMark and GitHub Flavored Markdown parser for Rust terminal UIs. It delivers typed block updates with lifecycle state, optional AI streaming previews, and full-document parsing.

[Performance report and reproduction protocol](docs/benchmarks.md) · [Parser design](docs/design.md)

A Rust library for parsing AI streaming Markdown in terminal UIs. The parser turns each append into typed block updates with content changes and lifecycle state. A terminal UI can render paragraphs, code, tables, and lists progressively from the same stream and use block status to decide when to commit final styling. Native append latency, memory use, and update scope are the primary metrics. GFM is the default; AI previews can be combined with GFM or CommonMark, and runtime plugins are an optional host layer.

The repository contains the Rust batch and incremental parsing core, an optional WASM plugin host, conformance tests, and native benchmarks. A TUI can call `Parser::append` for change batches, `Parser::finish` to settle the session, or `parse` for a complete document. Set `Options::math` and `Options::footnotes` to `true` to enable math and named footnotes. `Options::markstream` enables VMR containers, admonitions, CJK emphasis, and typographic extensions. Tags registered through `Options::custom_tags` borrow their attributes and raw body while also carrying parsed inline children. AI previews can deliver temporary inline overlays for open paragraphs and headings; closing a node delivers its final structure. A change batch from one append can merge temporary updates for the same node, while batches already returned to the caller stay unchanged.

Finalization is explicit. `finish()` is the final marker for a streaming session: it consumes the last line, settles pending references and active blocks, clears every preview, and leaves every block `Closed` or `Incomplete`. `parse()` is final by definition. Call `finish()` even when the last chunk appears to complete the message; an append alone is always a streaming state. See [finalization and malformed input](docs/design.md#finalization-and-malformed-input).

A TUI that already owns the source can move its `String` into `SourceBuffer` and parse appended content with `BorrowedParser`. Ranges delivered by the parser borrow that single source. [Source and incremental output](docs/design.md#source-and-incremental-output)

## Verification

After cloning the repository, use Rust 1.88 or newer and pnpm to run the following checks.

```sh
cargo test --locked --workspace
pnpm install --frozen-lockfile
pnpm test
pnpm test:spec --native
pnpm test:spec --native-stream
mkdir -p results
pnpm --silent bench > results/reference-node.json
pnpm test:spec --list
```

- Node behavior cases in `test/behavior` can switch between the reference implementation and the candidate adapter.
- `test/original` holds the remaining original cases, including assertions that depend on legacy tokens, plugins, and runtime internals, plus mixed cases whose node-level assertions have not yet been extracted.
- `test:differential` replays fixed seeds and real corpora. Candidate mode compares every commit with a freshly created reference instance. Reference mode also records how reused sync instances differ from new instances.
- `test:spec` downloads and verifies the official CommonMark 0.31.2 and cmark-gfm 0.29.0.gfm.13 examples on first run. `--native` checks complete Rust parsing; `--native-stream` checks appending one Unicode scalar at a time and applying the resulting changes. The reference source and real corpus are bundled, so regression tests run offline after installation.

The reference source archive and corpus are pinned to `markstream-vue` commit `31668cf54cb74fc46a6b4d0e9e7f0db67fe8bfed`. The archive and the legacy tokenizer serve verification only. Uncommitted changes from the original repository are not part of the baseline.

## Candidate integration

Run compatibility tests against this repository's Rust candidate:

```sh
cargo build --locked --release --example node_candidate
node --test test/native-candidate.test.mjs
PARSER_CANDIDATE=./scripts/native-candidate.mjs pnpm test:candidate
```

The Node verification bridge appends chunk by chunk to one Rust session and applies Rust changes to the legacy node view. The bridge is for compatibility tests; Rust TUIs and native benchmarks call the Rust API directly.

```sh
PARSER_CANDIDATE=./path/to/adapter.mjs pnpm test:candidate
PARSER_CANDIDATE=./path/to/adapter.mjs pnpm test:spec
PARSER_CANDIDATE=./path/to/adapter.mjs pnpm --silent bench > results/comparison-node.json
```

The Node verification adapter exports synchronous `createSession({ id, factoryOptions, parseOptions })`, returning `append(chunk)`, `finish()`, `snapshot()`, and optional `dispose()` and `metrics()`. The adapter applies each candidate update in order and reads current nodes only when a test needs them. The conformance entry point calls `renderSpec(source, { dialect, extensions })` and converts nodes from the same core into HTML for the specification examples.

[Streaming previews, Rust API, plugins, block caching, and acceptance design](docs/design.md)

[Native benchmarks](crates/bench/README.md) · [WASM plugin host and protocol](crates/plugin-host/README.md)
