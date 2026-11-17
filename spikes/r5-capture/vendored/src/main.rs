//! R5 probe B (vendored-XML route), part 2: client bindings generated from the
//! XML files vendored in ./protocols, then bound against the live compositor.
//!
//! Nothing here comes from the `wayland-protocols` crate: only `wayland-client`
//! plus the code generator.

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

/// ext-foreign-toplevel-list-v1 — needed only because
/// ext-image-capture-source-v1 takes an `ext_foreign_toplevel_handle_v1`.
pub mod foreign_toplevel {
    #![allow(unused_imports)] // the trailing imports are for cross-protocol refs
    use wayland_client;
    use wayland_client::protocol::*;

    #[allow(missing_docs)]
    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/ext-foreign-toplevel-list-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocols/ext-foreign-toplevel-list-v1.xml");
}

/// ext-image-capture-source-v1
pub mod capture_source {
    use wayland_client;
    use wayland_client::protocol::*;
    use super::foreign_toplevel::*;

    #[allow(missing_docs)]
    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        use super::super::foreign_toplevel::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/ext-image-capture-source-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocols/ext-image-capture-source-v1.xml");
}

/// ext-image-copy-capture-v1
pub mod copy_capture {
    use wayland_client;
    use wayland_client::protocol::*;
    use super::capture_source::*;

    #[allow(missing_docs)]
    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        use super::super::capture_source::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/ext-image-copy-capture-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocols/ext-image-copy-capture-v1.xml");
}

use capture_source::ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1;
use capture_source::ext_image_capture_source_v1::ExtImageCaptureSourceV1;
use capture_source::ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1;
use copy_capture::ext_image_copy_capture_cursor_session_v1::ExtImageCopyCaptureCursorSessionV1;
use copy_capture::ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1;
use copy_capture::ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options};
use copy_capture::ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1;

struct State;

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

macro_rules! ignore_events {
    ($($iface:ty),* $(,)?) => {$(
        impl Dispatch<$iface, ()> for State {
            fn event(
                _: &mut Self,
                _: &$iface,
                _: <$iface as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )*};
}
ignore_events!(
    ExtImageCopyCaptureManagerV1,
    ExtOutputImageCaptureSourceManagerV1,
    ExtForeignToplevelImageCaptureSourceManagerV1,
);

fn main() {
    // The generated interfaces, straight from the vendored XML.
    println!("gen   {} v{}", ExtImageCopyCaptureManagerV1::interface().name, ExtImageCopyCaptureManagerV1::interface().version);
    println!("gen   {} v{}", ExtImageCopyCaptureSessionV1::interface().name, ExtImageCopyCaptureSessionV1::interface().version);
    println!("gen   {} v{}", ExtImageCopyCaptureFrameV1::interface().name, ExtImageCopyCaptureFrameV1::interface().version);
    println!("gen   {} v{}", ExtImageCopyCaptureCursorSessionV1::interface().name, ExtImageCopyCaptureCursorSessionV1::interface().version);
    println!("gen   {} v{}", ExtImageCaptureSourceV1::interface().name, ExtImageCaptureSourceV1::interface().version);
    println!("gen   {} v{}", ExtOutputImageCaptureSourceManagerV1::interface().name, ExtOutputImageCaptureSourceManagerV1::interface().version);
    println!(
        "gen   {} v{}",
        ExtForeignToplevelImageCaptureSourceManagerV1::interface().name,
        ExtForeignToplevelImageCaptureSourceManagerV1::interface().version
    );
    println!("gen   manager Options::PaintCursors = bits {}", Options::PaintCursors.bits());
    println!("gen   codegen report: {}", env!("R5_CODEGEN_REPORT"));

    // Same live check as probe A: bind the managers, send no capture request.
    if !bind_live() {
        println!("live  SKIP: no compositor socket / bind refused");
    }
    println!("vendored-route: OK");
}

fn bind_live() -> bool {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            println!("live  no compositor ({e})");
            return false;
        }
    };
    let (globals, mut queue) = match registry_queue_init::<State>(&conn) {
        Ok(v) => v,
        Err(e) => {
            println!("live  registry init failed ({e})");
            return false;
        }
    };
    let qh = queue.handle();
    let mut st = State;
    let mgr = globals.bind::<ExtImageCopyCaptureManagerV1, State, ()>(&qh, 1..=1, ());
    let src = globals.bind::<ExtOutputImageCaptureSourceManagerV1, State, ()>(&qh, 1..=1, ());
    let top = globals.bind::<ExtForeignToplevelImageCaptureSourceManagerV1, State, ()>(&qh, 1..=1, ());
    match (&mgr, &src, &top) {
        (Ok(m), Ok(s), Ok(t)) => {
            println!("live  bound manager       object_id={} version={}", m.id().protocol_id(), m.version());
            println!("live  bound output-source object_id={} version={}", s.id().protocol_id(), s.version());
            println!("live  bound toplevel-src  object_id={} version={}", t.id().protocol_id(), t.version());
        }
        _ => {
            println!("live  bind failed for at least one manager");
            return false;
        }
    }
    let _ = queue.roundtrip(&mut st);
    true
}
