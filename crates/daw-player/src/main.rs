//! Hear the engine: plays a demo pattern live, or renders it to a WAV file.
//!
//! ```text
//! daw-player play [--bars N] [--compare]
//! daw-player render <out.wav> [--bars N]
//! ```
//!
//! `--compare` records what the engine produced during live playback and
//! checks it sample-for-sample against an offline render of the same song.

mod demo;
mod live;
mod offline;

use std::process::ExitCode;

#[global_allocator]
static ALLOCATOR: daw_rt::GuardedAllocator = daw_rt::GuardedAllocator;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const USAGE: &str = "usage:
  daw-player play [--bars N] [--compare]
  daw-player render <out.wav> [--bars N]";

/// Default length: twice through the four-bar loop.
const DEFAULT_BARS: u32 = 8;

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<()> {
    let mut bars = DEFAULT_BARS;
    let mut compare = false;
    let mut positional = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bars" => {
                bars = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .filter(|&n| n > 0)
                    .ok_or("--bars needs a positive whole number")?;
            }
            "--compare" => compare = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => positional.push(arg),
        }
    }
    match positional.as_slice() {
        [cmd] if cmd == "play" => live::play(bars, compare),
        [cmd, path] if cmd == "render" => offline::render_to_file(path.as_ref(), bars),
        _ => Err(USAGE.into()),
    }
}

/// Frames in `bars` bars of the demo at `sample_rate`.
fn frames_for_bars(bars: u32, sample_rate: u32) -> usize {
    let seconds = f64::from(bars) * 4.0 * 60.0 / demo::TEMPO_BPM;
    (seconds * f64::from(sample_rate)).round() as usize
}
