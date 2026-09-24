//! The headless smoke binary (composition-seams acceptance, UI-as-plugin
//! note): reads a script from a file or stdin, executes it against the Host
//! API contract, writes the bounce, and prints the session summary. No
//! frontend — the reference host a Tauri shell will eventually mirror.

use std::io::Read;
use std::path::PathBuf;

use host::{parse_script, run_script, summarize};

fn main() {
    let script_text = if let Some(path) = std::env::args().nth(1) {
        std::fs::read_to_string(&path).unwrap_or_else(|e| {
            eprintln!("host: cannot read script '{path}': {e}");
            std::process::exit(2);
        })
    } else {
        let mut text = String::new();
        if let Err(e) = std::io::stdin().read_to_string(&mut text) {
            eprintln!("host: cannot read script from stdin: {e}");
            std::process::exit(2);
        }
        text
    };

    let script = match parse_script(&script_text) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("host: bad script: {e}");
            std::process::exit(2);
        }
    };

    let session = match run_script(&script) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("host: script failed: {e}");
            std::process::exit(1);
        }
    };

    // Collect the bounce path from the script (the last Bounce command).
    let bounce_path: Option<PathBuf> = script
        .iter()
        .rev()
        .find_map(|c| match c {
            host::HostCommand::Bounce { path, .. } => Some(path.clone()),
            _ => None,
        })
        .filter(|p| p.exists());

    match bounce_path {
        Some(p) => println!(
            "host: bounced {} bytes to {}",
            std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0),
            p.display()
        ),
        None => eprintln!("host: no bounce written (no Bounce command or it failed)"),
    }

    // How the last seek was served: a long jump into a piece is a warm-up run-in, not a
    // render from 0 — worth saying, because it is the difference between interactive and
    // not on a 30-minute arrangement.
    if let Some((frame, warmed)) = session.last_seek() {
        let rate = session.sample_rate().max(1) as f64;
        let how = if warmed {
            format!(
                "warmed (a {:.2} s run-in)",
                host::SEEK_WARMUP_FRAMES as f64 / rate
            )
        } else {
            "full replay (state placed after the run-in, or a short jump)".to_string()
        };
        println!("host: seek to frame {frame} — {how}");
    }

    // The export report (peak/RMS are measured, not guessed, and are read back from
    // the session's record of the last successful export).
    if let Some(record) = session.last_export() {
        let path = script
            .iter()
            .rev()
            .find_map(|c| match c {
                host::HostCommand::Export { path, .. } => Some(path.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let db = |x: f32| {
            if x > 0.0 {
                20.0 * x.log10()
            } else {
                f32::NEG_INFINITY
            }
        };
        println!(
            "host: exported {} frames ({}) to {} — peak {:.4} ({:.1} dBFS), rms {:.4} ({:.1} dBFS), \
             drained {} frames",
            record.frames,
            host::ExportFormat::from_code(record.format)
                .map(|f| f.name())
                .unwrap_or("?"),
            path.display(),
            record.peak,
            db(record.peak),
            record.rms,
            db(record.rms),
            record.drained_frames
        );
    } else if script
        .iter()
        .any(|c| matches!(c, host::HostCommand::Export { .. }))
    {
        eprintln!("host: no export written (the Export command failed)");
    }
    print!("{}", summarize(&session));
}
