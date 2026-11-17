//! spike R6 — KDL binds generator for Niri.
//!
//! `r6-binds gen <spec.json> <out.kdl>` reads a small declarative hotkey spec
//! (the shape of the shell's `binds` config model) and writes a KDL fragment
//! for `~/.config/niri/config.kdl`. The fragment is meant to be checked with
//! `niri validate` before it is written next to the user's config.
//!
//! Exit codes: 0 ok, 1 I/O, 2 invalid spec.

mod combo;
mod emit;
mod json;
mod spec;

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [cmd, spec_path, out_path] if cmd == "gen" => match generate(spec_path, out_path) {
            Ok(summary) => {
                println!("{summary}");
                ExitCode::SUCCESS
            }
            Err(GenError::Spec(msg)) => {
                eprintln!("spec error: {msg}");
                ExitCode::from(2)
            }
            Err(GenError::Io(msg)) => {
                eprintln!("io error: {msg}");
                ExitCode::from(1)
            }
        },
        _ => {
            eprintln!("usage: r6-binds gen <spec.json> <out.kdl>");
            ExitCode::from(2)
        }
    }
}

enum GenError {
    Spec(String),
    Io(String),
}

impl From<std::io::Error> for GenError {
    fn from(e: std::io::Error) -> Self {
        GenError::Io(e.to_string())
    }
}

fn generate(spec_path: &str, out_path: &str) -> Result<String, GenError> {
    let text = std::fs::read_to_string(spec_path).map_err(|e| GenError::Io(format!("{spec_path}: {e}")))?;
    let spec = spec::load(&text).map_err(GenError::Spec)?;
    emit::check_locked(&spec).map_err(GenError::Spec)?;
    let kdl = emit::fragment(&spec).map_err(GenError::Spec)?;

    if let Some(parent) = Path::new(out_path).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(out_path, &kdl)?;

    let kinds: Vec<&str> = spec.binds.iter().map(|b| b.action.kind()).collect();
    Ok(format!(
        "wrote {out_path}: {} binds [{}], hotkey-overlay skip-at-startup={}",
        spec.binds.len(),
        kinds.join(","),
        spec.skip_hotkey_overlay_at_startup
    ))
}
