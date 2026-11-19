# stream-markdown-rs benchmarks

This report measures the native Rust parser with a caller consuming typed output. It also measures visible-first history restore and allocation retention. It ships with per-round samples, source and executable fingerprints, a correctness gate, and the commands used to reproduce it.

## Comparison contract

- **Full parse.** Both engines receive the same borrowed UTF-8 source with GFM options. The candidate materializes a typed document. The baseline collects pulldown-cmark 0.13.6 events. A canonical trace verifies both outputs for the supported syntax of the fixture.
- **Stream.** Both see identical code-point-aligned packets. The candidate calls `append` and applies typed changes to a retained view. The source-owning baseline accumulates the same input and uses the whole-document pulldown parser to replay each prefix. A borrowed replay baseline is also recorded. This measures the cost of implementing a live terminal preview with a whole-document parser, alongside a purpose-built incremental parser.
- **Preview.** AI previews are a separate workload fed 32-byte target packets. Verification applies and fingerprints updates from each packet, then compares the final view with the parser's full result. Preview timings include applying inline overlays.
- **History.** Eager and visible-first restoration use the same Rust parser, message slices, and final retained documents. The staged run publishes the latest four messages first, then restores up to 32 per batch. Verification compares every final slot. First visible publication and full restore are separate costs.
- **Memory.** Peak and retained requested bytes come from a separate counting allocator build. They cover the parser and the retained output above fixture and harness baseline. The standard timing build uses the normal allocator. Requested bytes exclude process RSS, allocator arenas, and runtime shared pages.

## Results

Measured 2026-11-17 on Apple M1 Pro with 32 GiB RAM and macOS 27.2.1 (arm64), rustc 1.98.1 and Python 3.14.5.

Canonical GFM traces and candidate final-stream checks passed **33/33** matrix rows. The values below list **stream-markdown-rs / pulldown-cmark**. A stream row uses source-owning prefix replay as its peer. The full parser baseline borrows the source. Size labels are targets. The fixture generator rounds to whole fragments and the JSON records exact UTF-8 lengths and packet counts.

### Real chat

| Nominal size | Full parse, ms | Full peak, KiB | Streaming total, ms | Append p95, µs | Streaming peak, KiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 KiB | 0.00367 / 0.00442 | 4.05 / 26.3 | 0.0107 / 0.0834 | 0.75 / 5.04 | 5.86 / 27.3 |
| 16 KiB | 0.0271 / 0.0376 | 37.4 / 135 | 0.066 / 5.06 | 0.833 / 43.5 | 56.1 / 151 |
| 128 KiB | 0.235 / 0.316 | 316 / 1,050 | 0.675 / 56.9 | 4.17 / 486 | 462 / 1,170 |
| 1 MiB | 1.96 / 3.01 | 2,560 / 8,450 | 2.44 / 415 | 13.2 / 3,080 | 3,720 / 9,470 |
| 8 MiB | 16.9 / 23.9 | 20,500 / 67,700 | 20.5 / 3,640 | 109 / 25,000 | 29,800 / 75,900 |

### Mixed Markdown

| Nominal size | Full parse, ms | Full peak, KiB | Streaming total, ms | Append p95, µs | Streaming peak, KiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 KiB | 0.00571 / 0.00979 | 10.2 / 43 | 0.0132 / 0.207 | 0.708 / 11.9 | 11.9 / 44 |
| 16 KiB | 0.0891 / 0.144 | 169 / 544 | 0.148 / 25.6 | 0.875 / 186 | 186 / 560 |
| 128 KiB | 0.686 / 1.18 | 1,350 / 4,350 | 0.851 / 202 | 3.54 / 1,510 | 1,480 / 4,480 |
| 1 MiB | 6.35 / 18.7 | 10,800 / 34,800 | 6.53 / 1,790 | 28.3 / 13,500 | 11,900 / 35,800 |
| 8 MiB | 48.4 / 100 | 86,500 / 279,000 | 52 / 15,300 | 244 / 113,000 | 94,800 / 287,000 |

At 16 KiB with 32-byte target packets, both borrowed-source variants consumed the real-chat fixture. Streaming total was 0.0848 / 8.26 ms. Peak requested allocation was 40.1 / 135 KiB.

The 8 MiB real-chat run produced its first output on packet one in both engines. Pulldown prefix replay took 142.5 µs and the incremental parser took 144.7 µs. The differences later in the stream account for the total-time gap.

### Additional grammar shapes at 16 KiB

Ratios below divide pulldown by the incremental parser. Values below 1 favor pulldown. The stream comparison assumes a visual update attempt after every packet. An application that coalesces updates can replay fewer prefixes, which changes the comparison.

| Corpus | Full total | Full peak | Stream total | Stream append p95 | Stream peak |
| --- | ---: | ---: | ---: | ---: | ---: |
| plain | 1.46× | 6.33× | 81.84× | 107.16× | 2.91× |
| long-paragraph | 1.87× | 2.93× | 244.80× | 1893.36× | 2.81× |
| open-fence | 17.78× | 489.11× | 39.83× | 76.39× | 3.03× |
| table | 1.86× | 5.70× | 216.70× | 323.84× | 5.33× |
| list | 1.61× | 3.50× | 184.85× | 310.21× | 3.20× |
| late-reference | 1.46× | 4.23× | 155.25× | 231.63× | 3.83× |

### AI previews and visible-first history

| Preview corpus | Nominal size | Packets | First output, µs (packet) | Append p95, µs | Total, ms | Peak, KiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| real-chat | 128 KiB | 3,962 | 0.834 (1) | 0.875 | 1.29 | 462 |
| preview-tail | 1 MiB | 32,776 | 0.875 (1) | 0.958 | 8.03 | 2,020 |

History values list **eager / visible-first**. First publication is timed inside the parser fixture. Both approaches retain the final history. Small histories may finish in a single visible batch.

| Messages | First publication, ms | Full restore, ms | Batch p95, ms | Peak, KiB |
| ---: | ---: | ---: | ---: | ---: |
| 2 | 0.00142 / 0.00142 | 0.00146 / 0.00146 | 0.00142 / 0.00142 | 1.23 / 1.23 |
| 25 | 0.0202 / 0.00313 | 0.0202 / 0.0211 | 0.0202 / 0.0181 | 21.5 / 21.5 |
| 198 | 0.166 / 0.00383 | 0.166 / 0.164 | 0.166 / 0.0266 | 175 / 175 |
| 1,584 | 1.36 / 0.00821 | 1.36 / 1.38 | 1.36 / 0.0323 | 1,400 / 1,400 |
| 12,672 | 10.8 / 0.0362 | 10.8 / 10.9 | 10.8 / 0.029 | 11,200 / 11,200 |

### Allocation retention

The counting allocator checks requested live bytes after each completed 16 KiB real-chat session is dropped. The fixed input fixture and harness buffers are allocated before the baseline.

| Lifecycle repeated 1,000 times | Max live bytes above baseline after drop | Last iteration |
| --- | ---: | ---: |
| full | 0 | 0 |
| stream | 0 | 0 |
| stream with AI previews | 0 | 0 |

This checks the measured Rust allocation lifecycles. OS page residency, caller-owned history and plugin runtimes have their own retention behavior.

[Raw matrix, preview, history and allocation samples](./benchmarks/data/2026-11-17-darwin-arm64)

## Reproduce

The engine source measured here is commit `a16a31b0ba3e8b59a27016f8d64376ce41772f86` of this repository. The benchmark runner is in this report's checkout. These commands use Rust 1.98.1 and Python 3.11 or later. The parser supports Rust 1.88. The sample was recorded with Python 3.14.5.

```sh
cargo build --locked --release -p stream-markdown-bench --target-dir target/public-timing
cargo build --locked --release -p stream-markdown-bench \
  --features count-allocations --target-dir target/public-allocations
python3 -B crates/bench/run_evidence.py \
  --timing-binary target/public-timing/release/stream-markdown-bench \
  --memory-binary target/public-allocations/release/stream-markdown-bench \
  --revision "$(git rev-parse HEAD)" --output results/public-evidence
```

The suite grows mixed Markdown and a real-chat fixture across five sizes, exercises six additional input shapes at 16 KiB, and runs borrowed 32-byte streaming at 16 KiB. It uses eleven timing rounds through 128 KiB, five at 1 MiB, and three at 8 MiB. The matrix alternates engine order across workloads. Each timed worker warms up first. Suite workers run sequentially. Matrix reports retain failed rows and timeouts. The source digest is tied to the recorded Rust toolchain. The underlying real-chat fixture is [checked in](../fixtures/corpus/ai-chat-streaming.md).

For correctness outside the timed GFM overlap, run the [Rust, Node compatibility, and CommonMark/GFM suites](../README.md#verification). The Node bridge includes transport and adaptation overhead, so native parser numbers use the native benchmark executables.

## Challenge a result

Use a [GitHub issue](https://github.com/Simon-He95/stream-markdown-rs/issues/new) with your commands, machine and OS, compiler and revision, raw JSON and input digest. Describe ownership, dialect, packet boundaries and the retained output. If your parser supports incremental updates, benchmark that API and verify its final output against the same trace. For a whole-input engine, label prefix replay separately from full parsing. Keep the rows where it loses, and include mismatches and timeouts.

For first-output claims, report the packet number as well as elapsed time. For memory, report allocation high-water and after-drop bytes separately from OS RSS when available. Rerun close comparisons in independent processes on a quiet machine and report the range. The data supports comparisons for the measured inputs and policy. Universal throughput and feature coverage require additional corpora and baselines.
