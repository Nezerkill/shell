//! R5 probe A (crate route): does the crates.io `wayland-protocols` crate ship
//! `ext-image-copy-capture-v1` + `ext-image-capture-source-v1` client bindings?
//!
//! This file is the question: it names the module paths and types a real capture
//! client uses, and then binds the live managers from the running compositor
//! without sending a single capture request.

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

use wayland_protocols::ext::image_capture_source::v1::client as ics;
use wayland_protocols::ext::image_copy_capture::v1::client as icc;

use icc::ext_image_copy_capture_cursor_session_v1::ExtImageCopyCaptureCursorSessionV1;
use icc::ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1;
use icc::ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options};
use icc::ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1;
use ics::ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1;
use ics::ext_image_capture_source_v1::ExtImageCaptureSourceV1;
use ics::ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1;

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
    // 1. Type level: the interfaces exist with the right names/versions. Any of
    //    these failing to resolve is a compile error, i.e. the crate lacks it.
    println!("type  {} v{}", ExtImageCopyCaptureManagerV1::interface().name, ExtImageCopyCaptureManagerV1::interface().version);
    println!("type  {} v{}", ExtImageCopyCaptureSessionV1::interface().name, ExtImageCopyCaptureSessionV1::interface().version);
    println!("type  {} v{}", ExtImageCopyCaptureFrameV1::interface().name, ExtImageCopyCaptureFrameV1::interface().version);
    println!("type  {} v{}", ExtImageCopyCaptureCursorSessionV1::interface().name, ExtImageCopyCaptureCursorSessionV1::interface().version);
    println!("type  {} v{}", ExtImageCaptureSourceV1::interface().name, ExtImageCaptureSourceV1::interface().version);
    println!("type  {}", ExtOutputImageCaptureSourceManagerV1::interface().name);
    println!("type  {}", ExtForeignToplevelImageCaptureSourceManagerV1::interface().name);

    // 2. Requests/enums the shell will really send, named so they must exist.
    fn _manager_requests(r: icc::ext_image_copy_capture_manager_v1::Request) {
        use icc::ext_image_copy_capture_manager_v1::Request as R;
        match r {
            R::CreateSession { .. } | R::CreatePointerCursorSession { .. } | R::Destroy => {}
            _ => {}
        }
    }
    fn _frame_requests(r: icc::ext_image_copy_capture_frame_v1::Request) {
        use icc::ext_image_copy_capture_frame_v1::Request as R;
        match r {
            R::Destroy | R::Capture => {}
            _ => {}
        }
    }
    let opts = Options::PaintCursors;
    println!("type  manager Options::PaintCursors = bits {}", opts.bits());
    let _reason = icc::ext_image_copy_capture_frame_v1::FailureReason::Unknown;

    // 3. Live: bind the managers from the running compositor, request nothing.
    if !bind_live() {
        println!("live  SKIP: no compositor socket / bind refused");
    }
    println!("crate-route: OK");
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
            println!("live  bind failed: mgr={:?}", mgr.err().map(|e| e.to_string()));
            println!("live  bind failed: output-src={:?}", src.err().map(|e| e.to_string()));
            println!("live  bind failed: toplevel-src={:?}", top.err().map(|e| e.to_string()));
            return false;
        }
    }
    // Flush the binds; no capture request is ever sent.
    let _ = queue.roundtrip(&mut st);
    true
}
