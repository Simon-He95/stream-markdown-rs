# Native parser benchmark

Use Python 3.11 or later for the matrix runner. Build two release binaries. The timing build uses the ordinary allocator. The allocation build counts requests and live requested bytes. `--target-dir` gives each binary its own path so the matrix runner can launch each scenario in a fresh process.

```sh
cargo build --release --locked -p stream-markdown-bench --target-dir target/benchmark-timing
cargo build --release --locked -p stream-markdown-bench --features count-allocations --target-dir target/benchmark-allocations
python3 crates/bench/run_matrix.py \
  --timing-binary target/benchmark-timing/release/stream-markdown-bench \
  --memory-binary target/benchmark-allocations/release/stream-markdown-bench \
  --case plain --size small
```

Omit selectors to run both modes across eight corpus shapes and five target sizes. The nominal sizes are 1 KiB, 16 KiB, 128 KiB, 1 MiB and 8 MiB. Synthetic workloads end at complete source fragments. The real-chat workload is a prefix or repetitions of the repository's chat fixture. The report records actual UTF-8 byte length and a source fingerprint so allocation and timing binaries cannot silently compare different corpora. Streaming also reports the sum of the prefix lengths submitted to the replay parser. Streaming uses identical code-point-aligned chunks for both engines. The default chunks are `max(32, target_bytes / 256)`. Pass `--chunk-bytes 32` to measure tiny LLM chunks across sizes. Very large prefix replays can exhaust the per-worker time budget. The matrix reports the timeout with no ratios for the row and continues to the next workload.

## Equivalent outputs and timing

- **Full** collects all native pulldown-cmark 0.13.6 events into a retained buffer and materializes the candidate's final typed document. It measures parsing plus materializing each engine's native semantic output.
- **Stream** feeds every chunk to the candidate append API and applies the changes to a retained block view. Pulldown-cmark has a whole-input parser, so both replay baselines parse every visible prefix and reuse an event buffer. `pulldown-owned` accumulates the same chunks in its own growing string and parses that input. To retain the event buffer safely across reallocations, the benchmark reanchors its source-backed event strings to identical slices in the fixture, outside the growing string. Owned event strings move without cloning. `pulldown` takes a borrowed prefix from the fixture with zero source copying and gives a conservative lower-bound comparison. Last append already uses EOF in both replay baselines, and their finish operations are no-ops. Timing ratios describe this TUI replay workload.
- **Verification** compares canonical semantic event traces before reporting a ratio. It checks final text, tag nesting, headings, alignment, links and other supported fields. Streaming additionally checks source spans and closure states against the candidate's full parse. Prefix previews can differ from eager EOF handling by pulldown. Intermediate correctness is checked separately from these performance rows. Unsupported constructs and mismatches produce failed rows with a diagnostic.
- **Timing** warms each workload once. Full mode reports median duration. Stream mode reports median total, first append, time and commit number of the first nonempty view, per-round append p95, maximum append and finish. Semantic verification and rendering are outside the timed region.
- **Memory** counts successful Rust allocator calls, reallocations, total requested bytes and peak live requested bytes relative to a baseline. It samples live bytes with the output retained and after both the output and session are dropped. Input fixtures and chunk boundaries exist before measurement. Requested bytes track allocator layout sizes, independently of RSS, allocator metadata and transient native realloc buffers. The timing build excludes the counting allocator.

Use `--min-speedup` and `--min-memory-ratio` when a release threshold is set. The matrix requires semantic verification for every selected row regardless of thresholds. The gates use the source-owning baseline for stream and the borrowed baseline for full. The borrowed streaming ratios are reported separately. The pulldown finish and zero or negative denominators have no comparison ratio. Timing can vary with host load. Use multiple identical runs on a quiet machine before treating small changes as regressions. The report records OS, architecture and binary digests. Pass `--revision "$(git rev-parse HEAD)"` to tag a run from a clean checkout.

To collect a pulldown-only calibration while the candidate is being developed:

```sh
cargo run --release -p stream-markdown-bench --no-default-features -- pulldown stream mixed medium
cargo run --release -p stream-markdown-bench --no-default-features --features count-allocations -- pulldown full mixed medium
```

[Pulldown-cmark source](https://github.com/pulldown-cmark/pulldown-cmark) · [Rust allocator contract](https://doc.rust-lang.org/std/alloc/trait.GlobalAlloc.html)
