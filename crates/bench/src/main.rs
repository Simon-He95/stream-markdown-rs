#[cfg(feature = "count-allocations")]
mod allocator;
#[cfg(feature = "candidate")]
mod candidate;
mod corpus;
#[cfg(feature = "candidate")]
mod history;
mod replay;
#[cfg(feature = "candidate")]
mod trace;

use pulldown_cmark::{Event, Options, Parser};
use replay::OwnedPulldown;
use serde_json::{json, Value};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::hint::black_box;
use std::process::ExitCode;
#[cfg(not(feature = "count-allocations"))]
use std::time::{Duration, Instant};

#[cfg(feature = "candidate")]
type Input = stream_markdown_rs::SourceBuffer;
#[cfg(not(feature = "candidate"))]
type Input = String;

const PULLDOWN_VERSION: &str = "0.13.6";

fn pulldown_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM
}

trait Engine<'a> {
    type Document;
    type Session;

    fn parse(source: &'a str) -> Result<Self::Document, String>;
    fn start(input: &'a Input) -> Self::Session;
    fn append(session: &mut Self::Session, source: &'a str, start: usize) -> Result<(), String>;
    fn finish(session: &mut Self::Session) -> Result<(), String>;
    #[cfg(not(feature = "count-allocations"))]
    fn has_output(session: &Self::Session) -> bool;
}

struct Pulldown;

impl<'a> Engine<'a> for Pulldown {
    type Document = Vec<Event<'a>>;
    type Session = Vec<Event<'a>>;

    fn parse(source: &'a str) -> Result<Self::Document, String> {
        Ok(Parser::new_ext(source, pulldown_options()).collect())
    }

    fn start(_: &'a Input) -> Self::Session {
        Vec::new()
    }

    fn append(session: &mut Self::Session, source: &'a str, _: usize) -> Result<(), String> {
        session.clear();
        session.extend(Parser::new_ext(source, pulldown_options()));
        Ok(())
    }

    fn finish(_: &mut Self::Session) -> Result<(), String> {
        Ok(())
    }

    #[cfg(not(feature = "count-allocations"))]
    fn has_output(session: &Self::Session) -> bool {
        !session.is_empty()
    }
}

fn consume_stream<'a, E: Engine<'a>>(
    session: &mut E::Session,
    source: &'a str,
    ends: &[usize],
    mut observe: impl FnMut(&mut E::Session, &'a str, usize) -> Result<(), String>,
) -> Result<(), String> {
    let mut start = 0;
    for &end in ends {
        observe(session, &source[..end], start)?;
        black_box(&*session);
        start = end;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Mode {
    Full,
    Stream,
}

impl Mode {
    fn from_str(input: &str) -> Option<Self> {
        match input {
            "full" => Some(Self::Full),
            "stream" => Some(Self::Stream),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Stream => "stream",
        }
    }
}

#[cfg(not(feature = "count-allocations"))]
fn percentile(values: &mut [u128], fraction: usize) -> u128 {
    values.sort_unstable();
    values[(values.len() * fraction).div_ceil(100) - 1]
}

#[cfg(not(feature = "count-allocations"))]
fn timed<'a, E: Engine<'a>>(
    mode: Mode,
    input: &'a Input,
    ends: &[usize],
    rounds: usize,
) -> Result<Value, String> {
    let source = input.as_str();
    let mut totals = Vec::with_capacity(rounds);
    let mut firsts = Vec::with_capacity(rounds);
    let mut first_outputs = Vec::with_capacity(rounds);
    let mut first_output_commits = Vec::with_capacity(rounds);
    let mut maxima = Vec::with_capacity(rounds);
    let mut tails = Vec::with_capacity(rounds);
    let mut finishes = Vec::with_capacity(rounds);
    let mut commits = Vec::with_capacity(ends.len());

    match mode {
        Mode::Full => {
            let warmup = Instant::now();
            loop {
                black_box(E::parse(black_box(source))?);
                if warmup.elapsed() >= Duration::from_millis(3) {
                    break;
                }
            }
            for _ in 0..rounds {
                let begin = Instant::now();
                let output = E::parse(black_box(source))?;
                black_box(&output);
                totals.push(begin.elapsed().as_nanos());
                drop(output);
            }
        }
        Mode::Stream => {
            let mut warmup = E::start(input);
            consume_stream::<E>(&mut warmup, source, ends, E::append)?;
            E::finish(&mut warmup)?;
            drop(warmup);

            for _ in 0..rounds {
                commits.clear();
                let began = Instant::now();
                let mut session = E::start(input);
                let mut first_output = None;
                consume_stream::<E>(&mut session, source, ends, |session, prefix, start| {
                    let commit = Instant::now();
                    E::append(session, black_box(prefix), start)?;
                    commits.push(commit.elapsed().as_nanos());
                    if first_output.is_none() && E::has_output(session) {
                        first_output = Some((began.elapsed().as_nanos(), commits.len()));
                    }
                    Ok(())
                })?;
                let finish = Instant::now();
                E::finish(&mut session)?;
                let ended = Instant::now();
                black_box(&session);
                totals.push(ended.duration_since(began).as_nanos());
                firsts.push(commits[0]);
                maxima.push(*commits.iter().max().unwrap());
                if first_output.is_none() && E::has_output(&session) {
                    first_output =
                        Some((ended.duration_since(began).as_nanos(), commits.len() + 1));
                }
                if let Some((time, index)) = first_output {
                    first_outputs.push(time);
                    first_output_commits.push(index as u128);
                }
                tails.push(percentile(&mut commits, 95));
                finishes.push(ended.duration_since(finish).as_nanos());
            }
        }
    }

    let mut output = json!({
        "measurement": "timing",
        "rounds": rounds,
        "total_samples_ns": &totals,
        "median_ns": percentile(&mut totals, 50),
    });
    if matches!(mode, Mode::Stream) {
        output["first_append_samples_ns"] = json!(&firsts);
        output["first_output_samples_ns"] = json!(&first_outputs);
        output["first_append_median_ns"] = json!(percentile(&mut firsts, 50));
        if !first_outputs.is_empty() {
            output["first_output_median_ns"] = json!(percentile(&mut first_outputs, 50));
            output["first_output_commit_median"] = json!(percentile(&mut first_output_commits, 50));
        }
        output["max_append_median_ns"] = json!(percentile(&mut maxima, 50));
        output["append_p95_samples_ns"] = json!(&tails);
        output["append_p95_median_ns"] = json!(percentile(&mut tails, 50));
        output["finish_median_ns"] = json!(percentile(&mut finishes, 50));
    }
    Ok(output)
}

#[cfg(feature = "count-allocations")]
fn memory<'a, E: Engine<'a>>(
    mode: Mode,
    input: &'a Input,
    ends: &[usize],
) -> Result<Value, String> {
    let source = input.as_str();
    let baseline = allocator::Baseline::start();
    let live_at_finish = match mode {
        Mode::Full => {
            let output = E::parse(black_box(source))?;
            black_box(&output);
            let live = baseline.live_delta();
            drop(output);
            live
        }
        Mode::Stream => {
            let mut session = E::start(input);
            consume_stream::<E>(&mut session, source, ends, E::append)?;
            E::finish(&mut session)?;
            black_box(&session);
            let live = baseline.live_delta();
            drop(session);
            live
        }
    };
    let returned = baseline.live_delta();
    let peak = baseline.peak_delta();
    let allocations = baseline.allocations();
    let reallocations = baseline.reallocations();
    let requested = baseline.requested();
    Ok(json!({
        "measurement": "allocations",
        "peak_live_bytes_delta": peak,
        "live_at_finish_bytes_delta": live_at_finish,
        "live_after_drop_bytes_delta": returned,
        "allocation_requests": allocations,
        "reallocation_requests": reallocations,
        "total_requested_bytes": requested,
    }))
}

#[cfg(all(feature = "candidate", feature = "count-allocations"))]
fn soak<'a, E: Engine<'a>>(
    mode: Mode,
    input: &'a Input,
    ends: &[usize],
    rounds: usize,
) -> Result<Value, String> {
    let source = input.as_str();
    let mut checkpoints = Vec::with_capacity(rounds.min(22));
    let baseline = allocator::Baseline::start();
    let mut min_live = 0;
    let mut max_live = 0;
    for iteration in 1..=rounds {
        match mode {
            Mode::Full => {
                let output = E::parse(black_box(source))?;
                black_box(&output);
            }
            Mode::Stream => {
                let mut session = E::start(input);
                consume_stream::<E>(&mut session, source, ends, E::append)?;
                E::finish(&mut session)?;
                black_box(&session);
            }
        }
        let live = baseline.live_delta();
        min_live = min_live.min(live);
        max_live = max_live.max(live);
        if iteration == 1 || iteration == rounds || iteration % rounds.div_ceil(20) == 0 {
            checkpoints.push((iteration, live));
        }
    }
    let peak = baseline.peak_delta();
    let returned = baseline.live_delta();
    Ok(json!({
        "measurement": "allocation-soak",
        "rounds": rounds,
        "peak_live_bytes_delta": peak,
        "min_after_drop_bytes_delta": min_live,
        "max_after_drop_bytes_delta": max_live,
        "live_after_drop_bytes_delta": returned,
        "checkpoints": checkpoints,
    }))
}

fn measure<'a, E: Engine<'a>>(
    mode: Mode,
    input: &'a Input,
    ends: &[usize],
    rounds: usize,
) -> Result<Value, String> {
    #[cfg(feature = "count-allocations")]
    {
        let _ = rounds;
        memory::<E>(mode, input, ends)
    }
    #[cfg(not(feature = "count-allocations"))]
    timed::<E>(mode, input, ends, rounds)
}

fn run(args: &[String]) -> Result<Value, String> {
    let usage = "usage: stream-markdown-bench <pulldown|pulldown-owned|candidate|candidate-borrowed|verify|verify-borrowed|candidate-preview|verify-preview|history-eager|history-batched|verify-history|soak|soak-preview> <full|stream> <case> <size> [rounds] [chunk-bytes]";
    let [action, mode, case, size, remaining @ ..] = args else {
        return Err(usage.into());
    };
    if remaining.len() > 2 {
        return Err(usage.into());
    }
    let mode = Mode::from_str(mode).ok_or(usage)?;
    let target = corpus::SIZES
        .iter()
        .find_map(|(name, bytes)| (*name == size).then_some(*bytes))
        .ok_or_else(|| format!("unknown size {size}; expected {:?}", corpus::SIZES))?;
    let source = corpus::source(case, target)
        .ok_or_else(|| format!("unknown case {case}; expected {:?}", corpus::CASES))?;
    #[cfg(feature = "candidate")]
    let input = Input::new(source);
    #[cfg(not(feature = "candidate"))]
    let input = source;
    let source = input.as_str();
    let rounds = positive_number(remaining.first(), 5)?;
    let chunk_bytes = positive_number(remaining.get(1), 32.max(target / 256))?;
    let ends = match mode {
        Mode::Full => Vec::new(),
        Mode::Stream => corpus::chunk_ends(source, chunk_bytes),
    };
    let mut result = match action.as_str() {
        "pulldown" => measure::<Pulldown>(mode, &input, &ends, rounds)?,
        "pulldown-owned" if matches!(mode, Mode::Stream) => {
            measure::<OwnedPulldown>(mode, &input, &ends, rounds)?
        }
        "pulldown-owned" => return Err("pulldown-owned measures streaming input only".into()),
        #[cfg(feature = "candidate")]
        "candidate" => measure::<candidate::Candidate>(mode, &input, &ends, rounds)?,
        #[cfg(feature = "candidate")]
        "candidate-preview" if matches!(mode, Mode::Stream) => {
            measure::<candidate::PreviewCandidate>(mode, &input, &ends, rounds)?
        }
        #[cfg(all(feature = "candidate", feature = "count-allocations"))]
        "soak" => soak::<candidate::Candidate>(mode, &input, &ends, rounds)?,
        #[cfg(all(feature = "candidate", feature = "count-allocations"))]
        "soak-preview" if matches!(mode, Mode::Stream) => {
            soak::<candidate::PreviewCandidate>(mode, &input, &ends, rounds)?
        }
        #[cfg(feature = "candidate")]
        "verify-preview" if matches!(mode, Mode::Stream) => {
            candidate::verify_preview(&input, &ends)?
        }
        #[cfg(feature = "candidate")]
        "history-eager" | "history-batched" | "verify-history"
            if matches!(mode, Mode::Full) && case == "preview-tail" =>
        {
            history::run(
                source,
                rounds,
                action == "history-batched",
                action == "verify-history",
            )?
        }
        #[cfg(feature = "candidate")]
        "verify" => candidate::verify(mode, &input, &ends, false)?,
        #[cfg(feature = "candidate")]
        "verify-borrowed" if matches!(mode, Mode::Stream) => {
            candidate::verify(mode, &input, &ends, true)?
        }
        #[cfg(feature = "candidate")]
        "candidate-borrowed" if matches!(mode, Mode::Stream) => {
            measure::<candidate::BorrowedCandidate>(mode, &input, &ends, rounds)?
        }
        #[cfg(not(feature = "candidate"))]
        "candidate" | "candidate-borrowed" | "verify" | "verify-borrowed" => {
            return Err("rebuild with the candidate feature".into());
        }
        _ => return Err(usage.into()),
    };
    result["engine"] = json!(action);
    result["mode"] = json!(mode.name());
    result["case"] = json!(case);
    result["size"] = json!(size);
    result["source_bytes"] = json!(source.len());
    result["target_bytes"] = json!(target);
    let mut fingerprint = DefaultHasher::new();
    source.hash(&mut fingerprint);
    result["source_fingerprint"] = json!(format!("{:016x}", fingerprint.finish()));
    result["commits"] = json!(ends.len());
    result["prefix_replay_bytes"] = json!(
        matches!(mode, Mode::Stream).then(|| ends.iter().map(|&end| end as u128).sum::<u128>())
    );
    result["chunk_bytes"] = json!(matches!(mode, Mode::Stream).then_some(chunk_bytes));
    result["pulldown_version"] = json!(PULLDOWN_VERSION);
    Ok(result)
}

fn positive_number(input: Option<&String>, default: usize) -> Result<usize, String> {
    match input {
        Some(value) => value
            .parse()
            .ok()
            .filter(|&value| value != 0)
            .ok_or_else(|| format!("expected a positive integer, saw {value}")),
        None => Ok(default),
    }
}

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match run(&args) {
        Ok(result) => {
            println!("{result}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
