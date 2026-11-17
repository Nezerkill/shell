// Spike R3: can we authenticate via PAM from Rust on this host, with no external helper?
//
// Deliberately raw FFI: no pam-client / pam-sys crates, so the binary has zero
// third-party dependencies and links exactly what pkg-config points at. The four
// checks are documented in RESULT.md; `cargo run` runs all of them once.
//
// Passwords used here are fake and hardcoded; nothing is read from disk or argv.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;

const PAM_SUCCESS: c_int = 0;
const PAM_BUF_ERR: c_int = 5;
// const PAM_PERM_DENIED: c_int = 6;
const PAM_AUTH_ERR: c_int = 7;
const PAM_CONV_ERR: c_int = 19;

const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_ERROR_MSG: c_int = 3;
const PAM_TEXT_INFO: c_int = 4;

const WRONG_PASSWORD: &str = "r3-pam-deliberately-wrong-password";

#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

#[repr(C)]
struct PamConv {
    conv: Option<
        extern "C" fn(
            c_int,
            *mut *mut PamMessage,
            *mut *mut PamResponse,
            *mut c_void,
        ) -> c_int,
    >,
    appdata_ptr: *mut c_void,
}

type PamHandle = *mut c_void;

extern "C" {
    fn pam_start(
        service: *const c_char,
        user: *const c_char,
        conv: *const PamConv,
        pamh: *mut PamHandle,
    ) -> c_int;
    fn pam_start_confdir(
        service: *const c_char,
        user: *const c_char,
        conv: *const PamConv,
        confdir: *const c_char,
        pamh: *mut PamHandle,
    ) -> c_int;
    fn pam_authenticate(pamh: PamHandle, flags: c_int) -> c_int;
    fn pam_end(pamh: PamHandle, status: c_int) -> c_int;
    fn pam_strerror(pamh: PamHandle, errnum: c_int) -> *const c_char;
    // libc, declared by hand to keep the crate dependency-free.
    fn calloc(nmemb: usize, size: usize) -> *mut c_void;
    fn strdup(s: *const c_char) -> *mut c_char;
    fn free(p: *mut c_void);
}

fn code_name(rc: c_int) -> String {
    let name = match rc {
        0 => "PAM_SUCCESS",
        1 => "PAM_OPEN_ERR",
        2 => "PAM_SYMBOL_ERR",
        3 => "PAM_SERVICE_ERR",
        4 => "PAM_SYSTEM_ERR",
        5 => "PAM_BUF_ERR",
        6 => "PAM_PERM_DENIED",
        7 => "PAM_AUTH_ERR",
        8 => "PAM_CRED_INSUFFICIENT",
        9 => "PAM_AUTHINFO_UNAVAIL",
        10 => "PAM_USER_UNKNOWN",
        11 => "PAM_MAXTRIES",
        12 => "PAM_NEW_AUTHTOK_REQD",
        13 => "PAM_ACCT_EXPIRED",
        14 => "PAM_SESSION_ERR",
        15 => "PAM_CRED_UNAVAIL",
        _ => return format!("PAM_ERR_{rc}"),
    };
    format!("{name}")
}

fn describe(pamh: PamHandle, rc: c_int) -> String {
    let msg = unsafe {
        let p = pam_strerror(pamh, rc);
        if p.is_null() {
            String::new()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    };
    format!("{} ({}) {:?}", code_name(rc), rc, msg)
}

/// PAM conversation: answers every prompt with the same (fake) password and prints
/// whatever the stack wants to say. appdata_ptr carries the C string of the password.
extern "C" fn conv_cb(
    num_msg: c_int,
    msg: *mut *mut PamMessage,
    resp: *mut *mut PamResponse,
    appdata: *mut c_void,
) -> c_int {
    unsafe {
        if num_msg <= 0 || msg.is_null() || resp.is_null() {
            return PAM_CONV_ERR;
        }
        let n = num_msg as usize;
        let replies = calloc(n, std::mem::size_of::<PamResponse>()) as *mut PamResponse;
        if replies.is_null() {
            return PAM_BUF_ERR;
        }
        let password = appdata as *const c_char;

        for i in 0..n {
            let m = *msg.add(i);
            if m.is_null() {
                free(replies as *mut c_void);
                return PAM_CONV_ERR;
            }
            let modern = (*m).msg_style;
            let text = if (*m).msg.is_null() {
                "<null>".to_string()
            } else {
                CStr::from_ptr((*m).msg).to_string_lossy().into_owned()
            };
            match modern {
                PAM_PROMPT_ECHO_OFF | PAM_PROMPT_ECHO_ON => {
                    eprintln!("    [conv] prompt(style={modern}): {text:?} -> <masked>");
                    (*replies.add(i)).resp = if password.is_null() {
                        ptr::null_mut()
                    } else {
                        strdup(password)
                    };
                    (*replies.add(i)).resp_retcode = 0;
                }
                PAM_ERROR_MSG | PAM_TEXT_INFO => {
                    eprintln!("    [conv] info(style={modern}): {text:?}");
                    (*replies.add(i)).resp = ptr::null_mut();
                    (*replies.add(i)).resp_retcode = 0;
                }
                _ => {
                    // Unknown style: release what we already allocated, refuse the talk.
                    for j in 0..i {
                        let r = &*replies.add(j);
                        if !r.resp.is_null() {
                            free(r.resp as *mut c_void);
                        }
                    }
                    free(replies as *mut c_void);
                    return PAM_CONV_ERR;
                }
            }
        }
        *resp = replies;
        PAM_SUCCESS
    }
}

fn cstring(s: &str) -> CString {
    CString::new(s).expect("CString::new: string contains NUL")
}

/// Which libpam did the loader actually map into this process? This is the honest
/// answer to "what are we linked against", independent of pkg-config flags.
fn loaded_libpam() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(maps) = std::fs::read_to_string("/proc/self/maps") {
        for line in maps.lines() {
            if let Some(path) = line.split_whitespace().nth(5) {
                if path.contains("/libpam.so") && !out.iter().any(|p| p == path) {
                    out.push(path.to_string());
                }
            }
        }
    }
    out
}

/// Point 5 evidence: the absolute module paths a host service file uses.
fn show_host_service_stack(service: &str) -> Option<String> {
    let path = format!("/etc/pam.d/{service}");
    let text = std::fs::read_to_string(&path).ok()?;
    println!("[0] host stack {path} (auth lines):");
    let mut first_unix = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with("auth") || line.starts_with("account") {
            println!("    {line}");
            if first_unix.is_none() && line.contains("pam_unix.so") {
                first_unix = line.split_whitespace().nth(2).map(str::to_string);
            }
        }
    }
    if let Some(m) = &first_unix {
        let exists = std::path::Path::new(m).exists();
        println!("    -> pam_unix.so path = {m} (exists={exists})");
    }
    first_unix
}

fn run_case(
    label: &str,
    service: &str,
    user: &str,
    confdir: Option<&str>,
    authenticate: bool,
) -> c_int {
    println!("\n[{label}] service={service:?} user={user:?} confdir={confdir:?}");
    let svc = cstring(service);
    let usr = cstring(user);
    let pass = cstring(WRONG_PASSWORD);
    let conv = PamConv {
        conv: Some(conv_cb),
        appdata_ptr: pass.as_ptr() as *mut c_void,
    };
    let mut pamh: PamHandle = ptr::null_mut();

    let rc = unsafe {
        match confdir {
            Some(dir) => {
                let dir = cstring(dir);
                println!("    pam_start_confdir({service:?}, {user:?}, conv, {dir:?})");
                pam_start_confdir(svc.as_ptr(), usr.as_ptr(), &conv, dir.as_ptr(), &mut pamh)
            }
            None => {
                println!("    pam_start({service:?}, {user:?}, conv)");
                pam_start(svc.as_ptr(), usr.as_ptr(), &conv, &mut pamh)
            }
        }
    };
    println!("    -> start: {}", describe(pamh, rc));
    if rc != PAM_SUCCESS {
        return rc;
    }

    let last = if authenticate {
        let rc2 = unsafe { pam_authenticate(pamh, 0) };
        println!("    -> pam_authenticate: {}", describe(pamh, rc2));
        rc2
    } else {
        rc
    };

    let rc_end = unsafe { pam_end(pamh, last) };
    println!("    -> pam_end: {}", describe(ptr::null_mut(), rc_end));
    last
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "all".to_string());
    let user = std::env::args()
        .nth(2)
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "nobody".to_string());
    let manifest = env!("CARGO_MANIFEST_DIR");
    let permit_dir = format!("{manifest}/confdir/permit");
    let deny_dir = format!("{manifest}/confdir/deny");

    println!("r3-pam spike  pid={}  user={user:?}", std::process::id());
    for p in loaded_libpam() {
        println!("libpam mapped: {p}");
    }

    let mut results: Vec<(&str, c_int, Option<c_int>, String)> = Vec::new();

    show_host_service_stack("login");

    if matches!(mode.as_str(), "all" | "link" | "auth" | "confdir") == false {
        eprintln!("usage: r3-pam [all|link|auth|confdir] [user]");
        std::process::exit(2);
    }

    if mode == "all" || mode == "link" {
        let rc = run_case("1 link+discovery", "login", &user, None, false);
        results.push((
            "1 pam_start(\"login\")",
            rc,
            Some(PAM_SUCCESS),
            "start+end on a real /etc/pam.d service".into(),
        ));
    }

    if mode == "all" || mode == "auth" {
        // Exactly one attempt, on purpose: never loop against a real stack.
        let rc = run_case("2 negative auth", "login", &user, None, true);
        results.push((
            "2 wrong password on login",
            rc,
            Some(PAM_AUTH_ERR),
            "stack must evaluate, not fail structurally".into(),
        ));
    }

    if mode == "all" || mode == "confdir" {
        let rc = run_case("3a confdir permit", "shell", &user, Some(&permit_dir), true);
        results.push((
            "3a private confdir + pam_permit",
            rc,
            Some(PAM_SUCCESS),
            "own stack, no /etc touched".into(),
        ));
        let rc = run_case("3b confdir deny", "shell", &user, Some(&deny_dir), true);
        results.push((
            "3b private confdir + pam_deny",
            rc,
            Some(PAM_AUTH_ERR),
            "own stack rejects".into(),
        ));
    }

    println!("\n=== summary ===");
    let mut failed = 0;
    for (label, got, want, why) in &results {
        let ok = match want {
            Some(w) => got == w,
            None => true,
        };
        if !ok {
            failed += 1;
        }
        println!(
            "{:<32} {}  got={} ({}), want={}  [{}]",
            label,
            if ok { "PASS" } else { "FAIL" },
            code_name(*got),
            got,
            want.map(|w| code_name(w)).unwrap_or_else(|| "-".into()),
            why
        );
    }
    if failed > 0 {
        eprintln!("\n{failed} check(s) did not match expectation");
        std::process::exit(1);
    }
}
