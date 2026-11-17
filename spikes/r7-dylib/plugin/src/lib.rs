//! R7 spike plugin. Behaviour is picked at call time from `R7_MODE`:
//! `ok | panic | abort | segv | oom | api_mismatch`.
//!
//! `R7_RAW=1` skips the `catch_unwind` inside `call`, so we can observe what
//! abi_stable/libffi alone do with a panicking plugin.

use abi_stable::export_root_module;
use abi_stable::prefix_type::PrefixTypeTrait;
use abi_stable::std_types::{RResult, RSlice, RString, RVec};
use r7_plugin_api::{RootModule, RootModule_Ref};

#[export_root_module]
pub fn root_module() -> RootModule_Ref {
    RootModule { plugin_api, call }.leak_into_prefix()
}

extern "C" fn plugin_api() -> u32 {
    match std::env::var("R7_MODE").as_deref() {
        Ok("api_mismatch") => 999,
        _ => r7_plugin_api::HOST_PLUGIN_API,
    }
}

extern "C" fn call(method: u32, payload: RSlice<'_, u8>) -> RResult<RVec<u8>, RString> {
    let bytes = payload.as_slice();
    if std::env::var_os("R7_RAW").is_some() {
        // No protection at all: let the panic cross the extern "C" boundary.
        return run(method, bytes);
    }
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(method, bytes)));
    match caught {
        Ok(value) => value,
        Err(payload) => RResult::RErr(RString::from(format!(
            "caught panic at ABI boundary: {}",
            panic_message(&payload)
        ))),
    }
}

fn run(method: u32, payload: &[u8]) -> RResult<RVec<u8>, RString> {
    let mode = std::env::var("R7_MODE").unwrap_or_else(|_| "ok".to_owned());
    match mode.as_str() {
        "ok" | "api_mismatch" => {
            let mut out = b"pong:".to_vec();
            out.extend_from_slice(payload);
            out.extend_from_slice(format!("|method={method}").as_bytes());
            RResult::ROk(RVec::from(out))
        }
        "panic" => panic!("spike panic"),
        "abort" => std::process::abort(),
        "segv" => {
            // Intentional null dereference: the point is the SIGSEGV.
            let value = unsafe { std::ptr::read_volatile(std::ptr::null::<u8>()) };
            RResult::ROk(RVec::from(vec![value]))
        }
        "oom" => oom(),
        other => RResult::RErr(RString::from(format!("unknown R7_MODE={other}"))),
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "<non-string panic payload>".to_owned()
    }
}

/// Allocate 64 MiB chunks until the allocator gives up (Rust aborts on OOM).
/// Capped so a careless run cannot eat all RAM; run.sh also sets `ulimit -v`.
fn oom() -> ! {
    let limit: usize = std::env::var("R7_OOM_CHUNKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(96);
    let mut keep: Vec<Vec<u8>> = Vec::new();
    for index in 0..=limit {
        let mut chunk = vec![0u8; 64 * 1024 * 1024];
        std::hint::black_box(&mut chunk);
        keep.push(chunk);
        eprintln!("plugin: allocated chunk {index} ({} MiB)", (index + 1) * 64);
    }
    eprintln!("plugin: {limit} chunks allocated, aborting on purpose");
    std::process::abort()
}
