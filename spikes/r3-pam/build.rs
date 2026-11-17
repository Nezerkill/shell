// Link against the system libpam as advertised by pkg-config (devShell linux-pam).
// No external crates: this must build offline and without guessing library paths.
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=PAM_LIB_DIR");
    println!("cargo:rerun-if-env-changed=PATH");

    // Escape hatch: an explicit lib dir always wins (used to prove the ABI point).
    if let Ok(dir) = std::env::var("PAM_LIB_DIR") {
        println!("cargo:rustc-link-search=native={dir}");
        println!("cargo:rustc-link-lib=pam");
        return;
    }

    let out = Command::new("pkg-config")
        .args(["--libs", "--cflags", "pam"])
        .output();

    match out {
        Ok(out) if out.status.success() => {
            let flags = String::from_utf8_lossy(&out.stdout).into_owned();
            for flag in flags.split_whitespace() {
                if let Some(p) = flag.strip_prefix("-L") {
                    println!("cargo:rustc-link-search=native={p}");
                } else if let Some(l) = flag.strip_prefix("-l") {
                    println!("cargo:rustc-link-lib={l}");
                }
            }
        }
        _ => panic!(
            "pkg-config cannot find 'pam'. Run this spike inside the project dev shell:\n\
             cd /home/nezerkill/shell && nix develop --no-warn-dirty -c bash -c 'cargo run --manifest-path spikes/r3-pam/Cargo.toml'"
        ),
    }
}
