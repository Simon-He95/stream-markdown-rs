use serde_json::{json, Value};
use std::hint::black_box;
use std::ops::Range;
#[cfg(not(feature = "count-allocations"))]
use std::time::Instant;
use stream_markdown_rs::{parse, restore_history, Document, Options, ParseError};

const VISIBLE: usize = 4;
const BACKGROUND_BATCH: usize = 32;

fn restore<'a>(
    messages: &'a [&'a str],
    visible: Range<usize>,
    options: &Options,
    batched: bool,
    mut published: impl FnMut(),
) -> Result<Vec<Option<Document<'a>>>, ParseError> {
    let mut slots = Vec::with_capacity(messages.len());
    slots.resize_with(messages.len(), || None);
    if !batched {
        for (index, message) in messages.iter().enumerate() {
            slots[index] = Some(parse(message, options)?);
        }
        published();
        return Ok(slots);
    }
    let visible_count = visible.len();
    let mut messages_to_parse = restore_history(messages, visible, options);
    for (index, document) in messages_to_parse.by_ref().take(visible_count) {
        slots[index] = Some(document?);
    }
    published();
    for _ in (visible_count..messages.len()).step_by(BACKGROUND_BATCH) {
        for (index, document) in messages_to_parse.by_ref().take(BACKGROUND_BATCH) {
            slots[index] = Some(document?);
        }
        published();
    }
    Ok(slots)
}

pub fn run(source: &str, rounds: usize, batched: bool, verify: bool) -> Result<Value, String> {
    // The preview-tail fixture builds independent messages separated by blank lines.
    let messages: Vec<&str> = source.split_inclusive("\n\n").collect();
    let visible = messages.len().saturating_sub(VISIBLE)..messages.len();
    let options = Options::default();
    if verify {
        let eager = restore(&messages, visible.clone(), &options, false, || {});
        let staged = restore(&messages, visible, &options, true, || {});
        let eager = eager.map_err(|error| error.to_string())?;
        let staged = staged.map_err(|error| error.to_string())?;
        let equal = eager.iter().zip(&staged).all(|(a, b)| {
            let (Some(a), Some(b)) = (a, b) else {
                return false;
            };
            a.source == b.source
                && a.nodes == b.nodes
                && a.nodes
                    .iter()
                    .all(|node| a.parent(node.id) == b.parent(node.id))
        });
        return Ok(json!({
            "measurement": "verification", "verified": equal,
            "messages": messages.len(),
        }));
    }
    #[cfg(feature = "count-allocations")]
    {
        let _ = rounds;
        let baseline = crate::allocator::Baseline::start();
        let docs = restore(&messages, visible, &options, batched, || {})
            .map_err(|error| error.to_string())?;
        black_box(&docs);
        let peak = baseline.peak_delta();
        drop(docs);
        let returned = baseline.live_delta();
        Ok(json!({
            "measurement": "allocations",
            "messages": messages.len(),
            "peak_live_bytes_delta": peak,
            "live_after_drop_bytes_delta": returned,
        }))
    }
    #[cfg(not(feature = "count-allocations"))]
    {
        let mut first = Vec::with_capacity(rounds);
        let mut totals = Vec::with_capacity(rounds);
        let mut tails = Vec::with_capacity(rounds);
        let mut batches = Vec::with_capacity(messages.len().div_ceil(BACKGROUND_BATCH) + 1);
        drop(
            restore(&messages, visible.clone(), &options, batched, || {})
                .map_err(|error| error.to_string())?,
        );
        for _ in 0..rounds {
            batches.clear();
            let started = Instant::now();
            let mut previous = started;
            let docs = restore(&messages, visible.clone(), &options, batched, || {
                let now = Instant::now();
                batches.push(now.duration_since(previous).as_nanos());
                previous = now;
            })
            .map_err(|error| error.to_string())?;
            black_box(&docs);
            totals.push(started.elapsed().as_nanos());
            first.push(batches[0]);
            tails.push(crate::percentile(&mut batches, 95));
        }
        Ok(json!({
            "measurement": "timing",
            "messages": messages.len(),
            "first_output_samples_ns": &first,
            "total_samples_ns": &totals,
            "batch_p95_samples_ns": &tails,
            "first_output_median_ns": crate::percentile(&mut first, 50),
            "median_ns": crate::percentile(&mut totals, 50),
            "batch_p95_median_ns": crate::percentile(&mut tails, 50),
        }))
    }
}
