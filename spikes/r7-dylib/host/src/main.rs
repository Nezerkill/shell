//! R7 spike host: dlopen the plugin, verify `plugin_api`, then call it.
//!
//! Usage: r7-host <path-to-libr7_plugin.so>
//! Exit codes: 0 ok, 3 abi mismatch (plugin body never called),
//! 4 dlopen/root-module failure, 64 bad usage, 128+n killed by signal n.

use abi_stable::library::RootModule as _;
use abi_stable::std_types::{RSlice, RResult};
use r7_plugin_api::{RootModule_Ref, HOST_PLUGIN_API};
use std::path::Path;

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: r7-host <path-to-libr7_plugin.so>");
        std::process::exit(64);
    };
    println!("host: dlopen {path}");

    let root: RootModule_Ref = match RootModule_Ref::load_from_file(Path::new(&path)) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("host: could not load plugin: {error:?}");
            std::process::exit(4);
        }
    };

    let api: u32 = (root.plugin_api())();
    println!("host: plugin_api={api}, host expects {HOST_PLUGIN_API}");
    if api != HOST_PLUGIN_API {
        eprintln!(
            "host: REFUSING to load plugin: ABI mismatch (plugin_api={api}, host expects {HOST_PLUGIN_API}); plugin body was never called"
        );
        std::process::exit(3);
    }

    let payload = b"payload";
    println!(
        "host: calling call(method=7, payload={:?})",
        String::from_utf8_lossy(payload)
    );
    match (root.call())(7, RSlice::from_slice(payload)) {
        RResult::ROk(bytes) => println!(
            "host: result: Ok({:?})",
            String::from_utf8_lossy(bytes.as_slice())
        ),
        RResult::RErr(error) => println!("host: result: Err({:?})", error.as_str()),
    }
    println!("host: still alive, exiting 0");
}
