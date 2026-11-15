//! `cargo run --release [--features oss-rest] [-- <case> [<contender>]]`
//!
//! With no arguments, runs every (case, contender) pair, each in its own
//! child process so the peak-heap reading belongs to that pair alone.
//! Throughput is input bytes over the median of several runs.

mod cases;
mod contenders;

use std::alloc;
use std::process::Command;
use std::time::Instant;

use cap::Cap;

#[global_allocator]
static ALLOCATOR: Cap<alloc::System> = Cap::new(alloc::System, usize::MAX);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => drive(None),
        [case] => drive(Some(case)),
        [case, contender] => run_one(case, contender),
        _ => eprintln!("usage: chunkjson-bench [<case> [<contender>]]"),
    }
}

fn drive(only: Option<&String>) {
    let exe = std::env::current_exe().expect("own path");
    println!("{:<12} {:<24} {:>10} {:>12} {:>14}", "case", "contender", "MB", "MB/s", "peak heap MB");
    let dir = std::env::temp_dir().join("chunkjson-bench");
    std::fs::create_dir_all(&dir).expect("input dir");
    for case in cases::NAMES {
        if only.is_some_and(|o| o != case) {
            continue;
        }
        std::fs::write(dir.join(case), cases::build(case)).expect("input written");
        for contender in contenders::names() {
            let out = Command::new(&exe).args([case, contender]).output().expect("child runs");
            print!("{}", String::from_utf8_lossy(&out.stdout));
            if !out.status.success() {
                println!("{case:<12} {contender:<24} failed: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or(""));
            }
        }
    }
}

fn run_one(case: &str, contender: &str) {
    // Read, not built here: building allocates far more than parsing, and the
    // allocator's high-water mark cannot be reset.
    let input = std::fs::read(std::env::temp_dir().join("chunkjson-bench").join(case)).expect("run without arguments first");
    let base = ALLOCATOR.allocated();
    let runs = if input.len() > (64 << 20) { 5 } else { 11 };
    let mut times = Vec::with_capacity(runs);
    let mut check = 0u64;
    let mut peak = 0usize;
    for _ in 0..runs {
        let start = Instant::now();
        check = contenders::run(contender, case, &input);
        times.push(start.elapsed().as_secs_f64());
        peak = peak.max(ALLOCATOR.max_allocated().saturating_sub(base));
    }
    times.sort_by(f64::total_cmp);
    let mb = input.len() as f64 / 1e6;
    let median = times[runs / 2];
    println!(
        "{case:<12} {contender:<24} {mb:>10.1} {:>12.0} {:>14.2}   (check {check})",
        mb / median,
        peak as f64 / 1e6
    );
}
