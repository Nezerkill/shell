//! R5 probe B (vendored-XML route), part 1: run the *system* code generator
//! (`wayland-scanner` from the devShell) over the vendored XML files, the way a
//! C/FFI client (or a bindgen-based build) would. This proves the XML copies are
//! complete and that the protocol can be generated outside of crates.io.
//!
//! Part 2 lives in src/main.rs: the same XMLs are fed to the `wayland-scanner`
//! crate's proc-macros to produce the Rust bindings the binary actually uses.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// (xml file, interfaces that must appear in the generated C)
const PROTOCOLS: &[(&str, &[&str])] = &[
    (
        "ext-image-copy-capture-v1.xml",
        &[
            "ext_image_copy_capture_manager_v1",
            "ext_image_copy_capture_session_v1",
            "ext_image_copy_capture_frame_v1",
            "ext_image_copy_capture_cursor_session_v1",
        ],
    ),
    (
        "ext-image-capture-source-v1.xml",
        &[
            "ext_image_capture_source_v1",
            "ext_output_image_capture_source_manager_v1",
            "ext_foreign_toplevel_image_capture_source_manager_v1",
        ],
    ),
    ("ext-foreign-toplevel-list-v1.xml", &["ext_foreign_toplevel_list_v1"]),
];

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let manifest =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let scanner = which("wayland-scanner").unwrap_or_else(|| {
        panic!("wayland-scanner not in PATH (it comes from the project devShell); cannot generate from vendored XML")
    });

    let mut report = String::new();
    for (xml_name, expected) in PROTOCOLS {
        let xml = manifest.join("protocols").join(xml_name);
        println!("cargo:rerun-if-changed=protocols/{xml_name}");
        assert!(xml.exists(), "missing vendored protocol {}", xml.display());

        let header = out_dir.join(format!("{xml_name}.h"));
        let code = out_dir.join(format!("{xml_name}.c"));
        run(
            &scanner,
            &[
                OsString::from("client-header"),
                xml.clone().into_os_string(),
                header.clone().into_os_string(),
            ],
        );
        run(
            &scanner,
            &[
                OsString::from("private-code"),
                xml.clone().into_os_string(),
                code.clone().into_os_string(),
            ],
        );

        let h = std::fs::read_to_string(&header).expect("generated header");
        let c = std::fs::read_to_string(&code).expect("generated code");
        for iface in *expected {
            let sym = format!("{iface}_interface");
            assert!(h.contains(&sym), "{xml_name}: {sym} missing from client header");
            assert!(c.contains(&sym), "{xml_name}: {sym} missing from private code");
        }
        // The generated C must carry the request/event tables, not just names.
        assert!(
            c.contains("wl_message"),
            "{xml_name}: private code has no wl_message tables"
        );
        report.push_str(&format!(
            "{xml_name}: header {} lines / {} B, private-code {} lines / {} B, {} interfaces checked\n",
            h.lines().count(),
            h.len(),
            c.lines().count(),
            c.len(),
            expected.len(),
        ));
    }

    // The report is quoted in RESULT.md, so keep it findable.
    let report_path = out_dir.join("codegen-report.txt");
    std::fs::write(&report_path, &report).expect("write report");
    println!("cargo:rustc-env=R5_CODEGEN_REPORT={}", report_path.display());
}

fn run(scanner: &Path, args: &[OsString]) {
    let out = Command::new(scanner)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn {}: {e}", scanner.display()));
    assert!(
        out.status.success(),
        "{} {:?} failed: {}",
        scanner.display(),
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|p| p.join(bin))
        .find(|p| p.is_file())
}
