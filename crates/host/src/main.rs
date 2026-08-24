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
        Some(p) => println!("host: bounced {} bytes to {}", std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0), p.display()),
        None => eprintln!("host: no bounce written (no Bounce command or it failed)"),
    }
    print!("{}", summarize(&session));
}
