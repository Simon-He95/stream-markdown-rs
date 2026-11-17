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

Pass `--borrowed-source --mode stream` when the caller already owns the input. Both engines then borrow identical prefixes of the source allocated before measurement. The Rust candidate uses `BorrowedParser` and applies the same block changes as the source-owning candidate. The gates compare against the borrowed `pulldown` replay.

## Equivalent outputs and timing

- **Full** collects all native pulldown-cmark 0.13.6 events into a retained buffer and materializes the candidate's final typed document. It measures parsing plus materializing each engine's native semantic output.
- **Stream** feeds every chunk to the candidate append API and applies the changes to a retained block view. Pulldown-cmark has a whole-input parser, so both replay baselines parse every visible prefix and reuse an event buffer. `pulldown-owned` accumulates the same chunks in its own growing string and parses that input. To retain the event buffer safely across reallocations, the benchmark reanchors its source-backed event strings to identical slices in the fixture, outside the growing string. Owned event strings move without cloning. `pulldown` takes a borrowed prefix from the fixture with zero source copying and gives a conservative lower-bound comparison. Last append already uses EOF in both replay baselines, and their finish operations are no-ops. Timing ratios describe this TUI replay workload.
- **Verification** compares canonical semantic event traces before reporting a ratio. It checks final text, tag nesting, headings, alignment, links and other supported fields. Streaming additionally checks source spans and closure states against the candidate's full parse. Prefix previews can differ from eager EOF handling by pulldown. Intermediate correctness is checked separately from these performance rows. Unsupported constructs and mismatches produce failed rows with a diagnostic.
- **Timing** warms full workloads for at least 3 ms and runs each stream workload once before measuring. Full mode reports median duration. Stream mode reports median total, first append, time and commit number of the first nonempty view, per-round append p95, maximum append and finish. The first-append and first-output samples are also recorded in measurement order. Semantic verification and rendering are outside the timed region.
- **Memory** counts successful Rust allocator calls, reallocations, total requested bytes and peak live requested bytes relative to a baseline. It samples live bytes with the source and final output retained and after both are dropped. The incremental view releases its reusable update buffer after applying the final batch. Input fixtures and chunk boundaries exist before measurement. Requested bytes track allocator layout sizes, independently of RSS, allocator metadata and transient native realloc buffers. The timing build excludes the counting allocator.
- **Type sizes** come from Rust's `size_of` on the verification binary's target. The `native_layout_bytes` field reports the span, block, stream view slot, change, inline content and pulldown event sizes. These measurements run outside the timed and allocation-counted paths.

Use `--min-speedup` and `--min-memory-ratio` when a release threshold is set. The matrix requires semantic verification for every selected row regardless of thresholds. The gates use the source-owning baseline for stream and the borrowed baseline for full. Each completed row prints its primary ratios and any failed thresholds to stderr. The final JSON includes the borrowed streaming ratios and all measurements. The pulldown finish and zero or negative denominators have no comparison ratio. Timing can vary with host load. The report records the wall time of each worker and its child-process CPU time on POSIX systems, including startup and warmup. Use multiple identical runs on a quiet machine before treating small changes as regressions. The report also records OS, architecture and binary digests. Pass `--revision "$(git rev-parse HEAD)"` to tag a run from a clean checkout.

To collect a pulldown-only calibration while the candidate is being developed:

```sh
cargo run --release -p stream-markdown-bench --no-default-features -- pulldown stream mixed medium
cargo run --release -p stream-markdown-bench --no-default-features --features count-allocations -- pulldown full mixed medium
```

[Pulldown-cmark source](https://github.com/pulldown-cmark/pulldown-cmark) · [Rust allocator contract](https://doc.rust-lang.org/std/alloc/trait.GlobalAlloc.html)
