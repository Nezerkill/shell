//! R1 spike — `ext-session-lock-v1` client written directly against
//! `wayland-client`, with bindings generated at compile time from the vendored
//! staging XML (`wayland_scanner::generate_interfaces!` /
//! `generate_client_code!`).  No session-lock wrapper crate, no toolkit, no GPU:
//! every frame is a `wl_shm` buffer painted by hand.
//!
//! What it does, in order:
//!   * binds `wl_compositor`, `wl_shm`, `zxdg_output_manager_v1`, every
//!     `wl_output` (v4) and `ext_session_lock_manager_v1`;
//!   * `lock`, then one `ext_session_lock_surface_v1` per output, immediately;
//!   * acks every `configure` and commits a buffer of *exactly* the requested
//!     size (the protocol makes a mismatch a fatal `dimensions_mismatch`), so a
//!     resize forces a repaint instead of a stretched or hole-y frame;
//!   * dumps every committed frame to PNG and prints one JSON line per event,
//!     so a shell script can assert on the whole life cycle;
//!   * unlocks on a stdin line (`unlock`), on `--hold` timeout, or never (the
//!     `SIGKILL` case the compositor has to survive).
//!
//! Safety: refuses to run unless `R1_NESTED=1`, which only `run.sh` sets after
//! it started a *headless* sway.  There is no way to lock the live session
//! with this binary by accident.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, Read, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::mpsc;

use std::time::{Duration, Instant};

use font8x8::UnicodeFonts;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

// ---------------------------------------------------------------------------
// Bindings generated from the vendored XML at compile time.
// ---------------------------------------------------------------------------
mod proto {
    use wayland_client;
    // generated code refers to `super::wl_surface`, `super::wl_output`, ...
    use wayland_client::protocol::*;

    pub mod __interfaces_esl {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("./protocols/ext-session-lock-v1.xml");
    }
    pub mod __interfaces_xdgout {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("./protocols/xdg-output-unstable-v1.xml");
    }
    use self::__interfaces_esl::*;
    use self::__interfaces_xdgout::*;

    wayland_scanner::generate_client_code!("./protocols/ext-session-lock-v1.xml");
    wayland_scanner::generate_client_code!("./protocols/xdg-output-unstable-v1.xml");
}

use proto::ext_session_lock_manager_v1::ExtSessionLockManagerV1;
use proto::ext_session_lock_surface_v1::ExtSessionLockSurfaceV1;
use proto::ext_session_lock_v1::ExtSessionLockV1;
use proto::zxdg_output_manager_v1::ZxdgOutputManagerV1;
use proto::zxdg_output_v1::ZxdgOutputV1;

static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn ms() -> u128 {
    T0.get_or_init(Instant::now).elapsed().as_millis()
}

/// One JSON line per notable event; `run.sh` greps these.
fn emit(kind: &str, fields: &str) {
    let mut out = std::io::stdout().lock();
    if fields.is_empty() {
        let _ = writeln!(out, "{{\"t\":{},\"ev\":\"{kind}\"}}", ms());
    } else {
        let _ = writeln!(out, "{{\"t\":{},\"ev\":\"{kind}\",{fields}}}", ms());
    }
    let _ = out.flush();
}

// ---------------------------------------------------------------------------
// Small hand-rolled raster frame (XRGB8888 / RGB) with the 8x8 font8x8 bitmap.
// ---------------------------------------------------------------------------
struct Frame {
    w: u32,
    h: u32,
    px: Vec<u32>,
}

const WHITE: u32 = 0x00f2f2f2;

impl Frame {
    fn new(w: u32, h: u32, bg: u32) -> Self {
        Frame {
            w,
            h,
            px: vec![bg; (w as usize) * (h as usize)],
        }
    }

    fn fill_rect(&mut self, x: i64, y: i64, rw: i64, rh: i64, color: u32) {
        let x0 = x.max(0).min(self.w as i64);
        let y0 = y.max(0).min(self.h as i64);
        let x1 = (x + rw).max(0).min(self.w as i64);
        let y1 = (y + rh).max(0).min(self.h as i64);
        for yy in y0..y1 {
            let row = (yy as usize) * (self.w as usize);
            for xx in x0..x1 {
                self.px[row + xx as usize] = color;
            }
        }
    }

    fn stroke_rect(&mut self, x: i64, y: i64, rw: i64, rh: i64, t: i64, color: u32) {
        self.fill_rect(x, y, rw, t, color);
        self.fill_rect(x, y + rh - t, rw, t, color);
        self.fill_rect(x, y, t, rh, color);
        self.fill_rect(x + rw - t, y, t, rh, color);
    }

    /// (x, y) = top-left, `s` = pixel scale, returns the advance in pixels.
    fn text(&mut self, x: i64, y: i64, s: i64, text: &str, color: u32) -> i64 {
        let mut cx = x;
        for ch in text.chars() {
            let up = ch.to_ascii_uppercase();
            if let Some(glyph) = font8x8::BASIC_FONTS.get(up) {
                for (row, bits) in glyph.iter().enumerate() {
                    for col in 0..8 {
                        if bits & (1 << col) != 0 {
                            self.fill_rect(cx + (col as i64) * s, y + (row as i64) * s, s, s, color);
                        }
                    }
                }
            }
            cx += 9 * s;
        }
        cx - x
    }

    fn hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for v in &self.px {
            for b in v.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        }
        h
    }

    fn frac_of(&self, color: u32) -> f64 {
        let n = self.px.iter().filter(|p| **p == color).count();
        n as f64 / self.px.len() as f64
    }

    fn to_png(&self, path: &std::path::Path) -> std::io::Result<()> {
        let mut rgb = Vec::with_capacity((self.w as usize) * (self.h as usize) * 3);
        for p in &self.px {
            rgb.push((p >> 16) as u8);
            rgb.push((p >> 8) as u8);
            rgb.push(*p as u8);
        }
        let file = File::create(path)?;
        let mut enc = png::Encoder::new(file, self.w, self.h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header()?;
        writer.write_image_data(&rgb)?;
        writer.finish()?;
        Ok(())
    }
}

/// Per-output background: opaque, far away from the red desktop that sits
/// behind a lock surface in the test compositor.
fn bg_for(name: &str) -> u32 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    const PALETTE: [u32; 4] = [0x001b2b4a, 0x00103a24, 0x002e1b3f, 0x003a2810];
    PALETTE[(h as usize) % PALETTE.len()]
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------
struct OutRec {
    output: wl_output::WlOutput,
    name: String,
    logical: Option<(i32, i32)>,
    physical: Option<(i32, i32)>,
    scale: i32,
    xdg: Option<ZxdgOutputV1>,
    lock_surface: Option<ExtSessionLockSurfaceV1>,
    lock_surface_created: bool,
}

struct LockSurf {
    out: usize,
    surface: wl_surface::WlSurface,
    lock_surface: ExtSessionLockSurfaceV1,
    configures: u32,
    paints: u32,
    buffer: Option<wl_buffer::WlBuffer>,
    last: Option<(u32, i32, i32)>,
}

struct App {
    compositor: wl_compositor::WlCompositor,
    shm: wl_shm::WlShm,
    manager: ExtSessionLockManagerV1,
    outputs: Vec<OutRec>,
    lock: Option<ExtSessionLockV1>,
    lock_surfaces: Vec<LockSurf>,
    locked: bool,
    finished: bool,
    lock_failed: bool,
    dump_dir: PathBuf,
    label: String,
    frames: u32,
    stopped: bool,
    _fds: Vec<OwnedFd>,
}

impl App {
    fn output_name(&self, i: usize) -> String {
        let o = &self.outputs[i];
        if !o.name.is_empty() {
            o.name.clone()
        } else {
            format!("OUT-{i}")
        }
    }

    fn logical_of(&self, i: usize) -> Option<(i32, i32)> {
        self.outputs[i].logical.or(self.outputs[i].physical)
    }

    /// Create one lock surface per output; must happen right after `lock` so
    /// the compositor never has to show an intermediate blank frame.
    fn create_lock_surfaces(&mut self, qh: &QueueHandle<App>) {
        let lock = self.lock.clone().expect("lock");
        let n = self.outputs.len();
        for i in 0..n {
            let surface = self.compositor.create_surface(qh, ());
            let out = self.outputs[i].output.clone();
            let idx = self.lock_surfaces.len();
            let ls = lock.get_lock_surface(&surface, &out, qh, idx);
            let name = self.output_name(i);
            emit(
                "lock_surface_created",
                &format!("\"index\":{idx},\"output\":\"{name}\",\"of\":{n}"),
            );
            self.lock_surfaces.push(LockSurf {
                out: i,
                surface,
                lock_surface: ls,
                configures: 0,
                paints: 0,
                buffer: None,
                last: None,
            });
            self.outputs[i].lock_surface = Some(self.lock_surfaces[idx].lock_surface.clone());
            self.outputs[i].lock_surface_created = true;
        }
    }

    fn paint_and_commit(
        &mut self,
        idx: usize,
        serial: u32,
        w: i32,
        h: i32,
        qh: &QueueHandle<App>,
    ) -> Result<(), String> {
        let name = self.output_name(self.lock_surfaces[idx].out);
        if w <= 0 || h <= 0 {
            return Err(format!("configure with non-positive size {w}x{h}"));
        }
        let bg = bg_for(&name);

        // --- paint -------------------------------------------------------
        let mut f = Frame::new(w as u32, h as u32, bg);
        let s = ((h as i64) / 150).clamp(2, 12);
        let pad = 12.max(s * 3);
        f.stroke_rect(0, 0, w as i64, h as i64, (s * 2).max(4), WHITE);

        let paints = self.lock_surfaces[idx].paints + 1;
        let lines = [
            format!("OUT {}", name),
            format!("{w}x{h}"),
            format!("SURF {}/{}", idx + 1, self.lock_surfaces.len()),
            format!("PAINT {paints}"),
            format!("SCALE {}", self.outputs[self.lock_surfaces[idx].out].scale),
            format!("T{}MS", ms()),
            self.label.clone(),
        ];
        let mut y = pad + 8;
        for (i, line) in lines.iter().enumerate() {
            let gs = if i == 0 { s } else { (s as i64 * 2 / 3).max(2) };
            f.text(pad, y, gs, line, if i == 0 { WHITE } else { 0x00d8d8d8 });
            y += 10 * gs + gs;
        }

        // A per-output "tab" on top so a screenshot shows which surface landed
        // on which output even at a glance: index+1 white squares.
        let out_idx = self.lock_surfaces[idx].out as i64;
        for k in 0..=out_idx {
            f.fill_rect(pad + k * (s * 12), h as i64 - pad - s * 8, s * 8, s * 8, WHITE);
        }
        // Paint counter as a bar: proves consecutive frames differ.
        let bar_w = (w as i64 / 8).max(8);
        f.fill_rect(
            0,
            0,
            bar_w.min(paints as i64 * (bar_w / 4).max(4)),
            s * 2,
            0x00ffd040,
        );

        // --- write the PNG ----------------------------------------------
        let png_path = self.dump_dir.join(format!(
            "frame-{}-p{}-{}x{}.png",
            name.replace(['/', ' '], "_"),
            paints,
            w,
            h
        ));
        f.to_png(&png_path).map_err(|e| format!("png dump failed: {e}"))?;

        // --- shm buffer of exactly the acked size ------------------------
        let stride = w * 4;
        let size = (stride * h) as usize;
        let fd = create_memfd("r1-esl-frame").map_err(|e| format!("memfd: {e}"))?;
        {
            let ret = unsafe { libc::ftruncate(fd.as_raw_fd(), size as libc::off_t) };
            if ret != 0 {
                return Err("ftruncate failed".into());
            }
            let ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd.as_raw_fd(),
                    0,
                )
            };
            if ptr == libc::MAP_FAILED {
                return Err("mmap failed".into());
            }
            unsafe {
                std::ptr::copy_nonoverlapping(f.px.as_ptr() as *const u8, ptr as *mut u8, size);
                libc::munmap(ptr, size);
            }
        }
        let pool = self.shm.create_pool(fd.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(0, w, h, stride, wl_shm::Format::Xrgb8888, qh, ());
        pool.destroy();

        let ls = &mut self.lock_surfaces[idx];
        // ack_configure must precede the commit that uses the acked size.
        ls.lock_surface.ack_configure(serial);
        ls.surface.attach(Some(&buffer), 0, 0);
        ls.surface.damage(0, 0, w, h);
        ls.surface.commit();
        if let Some(old) = ls.buffer.replace(buffer) {
            old.destroy();
        }
        ls.paints = paints;
        ls.configures += 1;
        ls.last = Some((serial, w, h));
        self.frames += 1;
        self._fds.push(fd);

        emit(
            "configure",
            &format!(
                "\"index\":{idx},\"output\":\"{}\",\"serial\":{serial},\"w\":{w},\"h\":{h},\
                 \"paint\":{paints},\"pixels\":{},\"known_frac\":{:.6},\"bg_frac\":{:.6},\
                 \"white_px\":{},\"hash\":\"{:016x}\",\"bg\":\"#{:06x}\",\"png\":\"{}\"",
                name,
                w as i64 * h as i64,
                f.frac_of(bg) + f.frac_of(WHITE) + f.frac_of(0x00d8d8d8) + f.frac_of(0x00ffd040),
                f.frac_of(bg),
                f.px.iter().filter(|p| **p == WHITE).count(),
                f.hash(),
                bg,
                png_path.display()
            ),
        );
        Ok(())
    }

    /// Client-initiated unlock: the protocol recommends a sync roundtrip so we
    /// know the server processed `unlock_and_destroy` before we exit.
    fn unlock(&mut self, conn: &Connection) -> Result<(), String> {
        self.stopped = true;
        if let Some(lock) = self.lock.take() {
            emit("unlock_requested", &format!("\"locked\":{}", self.locked));
            lock.unlock_and_destroy();
            conn.flush().map_err(|e| format!("flush: {e}"))?;
        }
        Ok(())
    }

    fn destroy_surfaces(&mut self) {
        for ls in &mut self.lock_surfaces {
            if let Some(b) = ls.buffer.take() {
                b.destroy();
            }
            ls.surface.destroy();
        }
        self.lock_surfaces.clear();
    }
}

fn create_memfd(name: &str) -> std::io::Result<OwnedFd> {
    let cname = std::ffi::CString::new(name).unwrap();
    let fd = unsafe {
        libc::syscall(
            libc::SYS_memfd_create,
            cname.as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

// ---------------------------------------------------------------------------
// Event loop helper: read + dispatch with a timeout, so stdin and --hold are
// also observed while the compositor is quiet.
// ---------------------------------------------------------------------------
enum PumpErr {
    Protocol(wayland_client::backend::protocol::ProtocolError),
    Fatal(String),
}

fn dispatch_err(e: wayland_client::DispatchError) -> PumpErr {
    match e {
        wayland_client::DispatchError::Backend(wayland_client::backend::WaylandError::Protocol(p)) => {
            PumpErr::Protocol(p)
        }
        other => PumpErr::Fatal(format!("dispatch: {other}")),
    }
}

fn pump(
    conn: &Connection,
    queue: &mut wayland_client::EventQueue<App>,
    app: &mut App,
    timeout_ms: i32,
) -> Result<(), PumpErr> {
    queue.dispatch_pending(app).map_err(dispatch_err)?;
    if let Some(guard) = queue.prepare_read() {
        let fd = conn.backend().poll_fd().as_raw_fd();
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let n = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if n > 0 && (pfd.revents & libc::POLLIN) != 0 {
            match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(e))
                    if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(wayland_client::backend::WaylandError::Protocol(p)) => return Err(PumpErr::Protocol(p)),
                Err(e) => return Err(PumpErr::Fatal(format!("read: {e}"))),
            }
        } else {
            drop(guard); // releases the read lock, no blocking read happens
        }
    }
    queue.dispatch_pending(app).map_err(dispatch_err)?;
    if let Err(e) = conn.flush() {
        return Err(PumpErr::Fatal(format!("flush: {e}")));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Dispatch impls
// ---------------------------------------------------------------------------
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for App {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

macro_rules! no_events {
    ($($iface:ty),* $(,)?) => {$(
        impl Dispatch<$iface, ()> for App {
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
no_events!(
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_surface::WlSurface,
    wl_callback::WlCallback,
    ExtSessionLockManagerV1,
    ZxdgOutputManagerV1,
);

impl Dispatch<wl_buffer::WlBuffer, ()> for App {
    fn event(
        _: &mut Self,
        proxy: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            proxy.destroy();
        }
    }
}

impl Dispatch<wl_output::WlOutput, usize> for App {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        data: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let i = *data;
        match event {
            wl_output::Event::Name { name } => state.outputs[i].name = name,
            wl_output::Event::Mode {
                flags, width, height, ..
            } => {
                if flags
                    .into_result()
                    .map(|f| f.contains(wl_output::Mode::Current))
                    .unwrap_or(true)
                {
                    state.outputs[i].physical = Some((width, height));
                }
            }
            wl_output::Event::Scale { factor } => state.outputs[i].scale = factor,
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputV1, usize> for App {
    fn event(
        state: &mut Self,
        _: &ZxdgOutputV1,
        event: proto::zxdg_output_v1::Event,
        data: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use proto::zxdg_output_v1::Event as E;
        let i = *data;
        match event {
            E::Name { name } => {
                // xdg-output's name is the one we label the frame with.
                if state.outputs[i].name.is_empty() {
                    state.outputs[i].name = name;
                }
            }
            E::LogicalSize { width, height } => state.outputs[i].logical = Some((width, height)),
            E::Done => {}
            _ => {}
        }
    }
}

impl Dispatch<ExtSessionLockV1, ()> for App {
    fn event(
        state: &mut Self,
        _: &ExtSessionLockV1,
        event: proto::ext_session_lock_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use proto::ext_session_lock_v1::Event as E;
        match event {
            E::Locked => {
                state.locked = true;
                emit(
                    "locked",
                    &format!(
                        "\"outputs\":{},\"surfaces\":{},\"frames\":{}",
                        state.outputs.len(),
                        state.lock_surfaces.len(),
                        state.frames
                    ),
                );
            }
            E::Finished => {
                state.finished = true;
                emit("finished", &format!("\"locked\":{}", state.locked));
            }
        }
    }
}

impl Dispatch<ExtSessionLockSurfaceV1, usize> for App {
    fn event(
        state: &mut Self,
        _: &ExtSessionLockSurfaceV1,
        event: proto::ext_session_lock_surface_v1::Event,
        data: &usize,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use proto::ext_session_lock_surface_v1::Event as E;
        let idx = *data;
        if state.stopped {
            return;
        }
        let E::Configure {
            serial,
            width,
            height,
        } = event;
        {
            let name = state.output_name(state.lock_surfaces[idx].out);
            emit(
                "configure_event",
                &format!(
                    "\"index\":{idx},\"output\":\"{name}\",\"serial\":{serial},\
                     \"w\":{width},\"h\":{height},\"locked_before\":{}",
                    state.locked
                ),
            );
            if let Err(e) = state.paint_and_commit(idx, serial, width as i32, height as i32, qh) {
                emit("error", &format!("\"message\":\"{e}\""));
                state.lock_failed = true;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Console handling: `unlock` on stdin, or a hard `--hold` ceiling.
// ---------------------------------------------------------------------------
enum Cmd {
    Unlock,
    Eof,
}

fn spawn_stdin_thread() -> mpsc::Receiver<Cmd> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.lock().read_line(&mut line) {
                Ok(0) => {
                    let _ = tx.send(Cmd::Eof);
                    return;
                }
                Ok(_) => {
                    let t = line.trim().to_ascii_lowercase();
                    if t == "unlock" || t == "quit" || t == "u" {
                        let _ = tx.send(Cmd::Unlock);
                        return;
                    }
                }
                Err(_) => {
                    let _ = tx.send(Cmd::Eof);
                    return;
                }
            }
        }
    });
    rx
}

// ---------------------------------------------------------------------------
// args
// ---------------------------------------------------------------------------
struct Args {
    expect_outputs: usize,
    dump_dir: PathBuf,
    unlock_on_stdin: bool,
    hold: Duration,
    probe_lock: bool, // lock, expect refusal, prove the session is already locked
    label: String,
    exit_after_lock: bool, // used with --probe-lock
}

impl Default for Args {
    fn default() -> Self {
        Args {
            expect_outputs: 0,
            dump_dir: PathBuf::from("frames"),
            unlock_on_stdin: false,
            hold: Duration::from_secs(60),
            probe_lock: false,
            label: "R1 ESL SPIKE".into(),
            exit_after_lock: false,
        }
    }
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--expect-outputs" => {
                a.expect_outputs = it
                    .next()
                    .ok_or("--expect-outputs needs a value")?
                    .parse()
                    .map_err(|_| "--expect-outputs: not a number")?
            }
            "--dump-dir" => a.dump_dir = PathBuf::from(it.next().ok_or("--dump-dir needs a value")?),
            "--unlock-on-stdin" => a.unlock_on_stdin = true,
            "--hold" => {
                let secs: f64 = it
                    .next()
                    .ok_or("--hold needs a value")?
                    .parse()
                    .map_err(|_| "--hold: not a number")?;
                a.hold = Duration::from_secs_f64(secs.max(0.5));
            }
            "--probe-lock" => {
                a.probe_lock = true;
                a.exit_after_lock = true;
            }
            "--label" => a.label = it.next().ok_or("--label needs a value")?.clone(),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(a)
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------
fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some("ppm-stats") {
        // One line per file, so the harness can scan a whole capture series
        // with a single process instead of one per frame.
        if argv.len() < 2 {
            eprintln!("usage: r1-esl ppm-stats FILE.ppm...");
            std::process::exit(2);
        }
        let mut rc = 0;
        for path in &argv[1..] {
            if ppm_stats(path) != 0 {
                rc = 2;
            }
        }
        std::process::exit(rc);
    }
    if std::env::var("R1_NESTED").as_deref() != Ok("1") {
        eprintln!(
            "refusing to run: R1_NESTED=1 is not set. This binary locks the session; it is \
             meant to run only inside the headless sway started by run.sh."
        );
        std::process::exit(2);
    }
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("r1-esl: {e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(args) {
        eprintln!("r1-esl: fatal: {e}");
        emit("fatal", &format!("\"message\":\"{e}\""));
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("connect: {e}"))?;
    let (globals, mut queue) = registry_queue_init::<App>(&conn).map_err(|e| format!("globals: {e}"))?;
    let qh = queue.handle();

    emit(
        "hello",
        &format!(
            "\"pid\":{},\"wayland_display\":\"{}\",\"dump_dir\":\"{}\",\"probe\":{}",
            std::process::id(),
            std::env::var("WAYLAND_DISPLAY").unwrap_or_default(),
            args.dump_dir.display(),
            args.probe_lock
        ),
    );
    std::fs::create_dir_all(&args.dump_dir).map_err(|e| format!("dump dir: {e}"))?;

    let contents = globals.contents().clone_list();
    for g in &contents {
        match g.interface.as_str() {
            "ext_session_lock_manager_v1"
            | "wl_compositor"
            | "wl_shm"
            | "xdg_output_manager_v1"
            | "zxdg_output_manager_v1" => emit(
                "global",
                &format!(
                    "\"interface\":\"{}\",\"version\":{},\"name\":{}",
                    g.interface, g.version, g.name
                ),
            ),
            _ => {}
        }
    }

    let compositor: wl_compositor::WlCompositor = globals
        .bind(&qh, 4..=6, ())
        .map_err(|e| format!("wl_compositor: {e}"))?;
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).map_err(|e| format!("wl_shm: {e}"))?;
    let manager: ExtSessionLockManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| format!("ext_session_lock_manager_v1: {e} (compositor lacks the protocol)"))?;
    // xdg-output: preferred (v3) but optional — before v3 the name comes from
    // zxdg_output_v1::name, from v3 on from wl_output.name as well.
    let xdg_mgr: Option<ZxdgOutputManagerV1> = globals.bind(&qh, 1..=3, ()).ok();

    let mut app = App {
        compositor,
        shm,
        manager,
        outputs: Vec::new(),
        lock: None,
        lock_surfaces: Vec::new(),
        locked: false,
        finished: false,
        lock_failed: false,
        dump_dir: args.dump_dir.clone(),
        label: args.label.clone(),
        frames: 0,
        stopped: false,
        _fds: Vec::new(),
    };

    let outputs: Vec<_> = contents
        .iter()
        .filter(|g| g.interface == "wl_output")
        .map(|g| (g.name, g.version))
        .collect();
    for (idx, (name, version)) in outputs.iter().enumerate() {
        let out: wl_output::WlOutput =
            globals
                .registry()
                .bind::<wl_output::WlOutput, usize, App>(*name, (*version).min(4), &qh, idx);
        app.outputs.push(OutRec {
            output: out,
            name: String::new(),
            logical: None,
            physical: None,
            scale: 1,
            xdg: None,
            lock_surface: None,
            lock_surface_created: false,
        });
    }
    if let Some(mgr) = &xdg_mgr {
        for i in 0..app.outputs.len() {
            let xo = mgr.get_xdg_output(&app.outputs[i].output, &qh, i);
            app.outputs[i].xdg = Some(xo);
        }
    }

    // Let all initial output state arrive.
    queue.roundtrip(&mut app).map_err(|e| format!("roundtrip: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.outputs.len() < args.expect_outputs
        || app
            .outputs
            .iter()
            .any(|o| o.logical.is_none() && o.physical.is_none())
    {
        if Instant::now() > deadline {
            break;
        }
        pump(&conn, &mut queue, &mut app, 50).map_err(|e| match e {
            PumpErr::Protocol(p) => format!("protocol error: {}", p.message),
            PumpErr::Fatal(m) => m,
        })?;
    }
    for i in 0..app.outputs.len() {
        emit(
            "output",
            &format!(
                "\"index\":{i},\"name\":\"{}\",\"logical\":\"{}\",\"physical\":\"{}\",\"scale\":{}",
                app.output_name(i),
                app.logical_of(i)
                    .map(|(w, h)| format!("{w}x{h}"))
                    .unwrap_or_default(),
                app.outputs[i]
                    .physical
                    .map(|(w, h)| format!("{w}x{h}"))
                    .unwrap_or_default(),
                app.outputs[i].scale
            ),
        );
    }
    emit(
        "outputs_ready",
        &format!(
            "\"count\":{},\"xdg_output\":{}",
            app.outputs.len(),
            xdg_mgr.is_some()
        ),
    );

    // --- lock ---------------------------------------------------------
    let lock = app.manager.lock(&qh, ());
    app.lock = Some(lock);
    emit("lock_requested", &format!("\"outputs\":{}", app.outputs.len()));
    app.create_lock_surfaces(&qh);
    conn.flush().map_err(|e| format!("flush: {e}"))?;

    let rx = if args.unlock_on_stdin {
        spawn_stdin_thread()
    } else {
        mpsc::channel::<Cmd>().1
    };
    let start = Instant::now();
    let mut unlocked = false;
    let mut ready_sent = false;

    loop {
        if app.finished && !app.locked {
            // Compositor refused the lock: it sent `finished` instead of
            // `locked` (ext-session-lock-v1 allows exactly this).
            emit(
                "refused",
                &format!("\"reason\":\"finished\",\"probe\":{}", args.probe_lock),
            );
            app.destroy_surfaces();
            if args.probe_lock {
                std::process::exit(5);
            }
            return Err("lock refused (finished)".into());
        }
        if app.lock_failed {
            emit(
                "refused",
                &format!("\"reason\":\"paint failed\",\"probe\":{}", args.probe_lock),
            );
            app.destroy_surfaces();
            return Err("paint failed".into());
        }

        match pump(&conn, &mut queue, &mut app, 50) {
            Ok(_) => {}
            Err(PumpErr::Protocol(p)) => {
                emit(
                    "protocol_error",
                    &format!(
                        "\"code\":{},\"interface\":\"{}\",\"message\":\"{}\"",
                        p.code,
                        p.object_interface,
                        p.message.replace('"', "'")
                    ),
                );
                // The compositor killed us: when it refuses a second lock it
                // usually sends `finished` instead, but be explicit anyway.
                if args.probe_lock {
                    std::process::exit(3);
                }
                return Err(format!("protocol error: {}", p.message));
            }
            Err(PumpErr::Fatal(m)) => return Err(m),
        }
        let _ = conn.flush();

        if args.probe_lock {
            // A probe either got `finished` (refused, handled above) or is
            // still waiting. Give the compositor 1.5 s to answer.
            if start.elapsed() > Duration::from_millis(1500) {
                emit("probe_no_answer", &format!("\"locked\":{}", app.locked));
                if app.locked {
                    app.unlock(&conn)?;
                    let _ = queue.roundtrip(&mut app);
                }
                app.destroy_surfaces();
                return Err("probe got no locked/finished".into());
            }
            continue;
        }

        // Wait until every surface has produced its first frame; only then
        // report the locked observable as fully ready.
        if !ready_sent
            && !unlocked
            && app.locked
            && !app.lock_surfaces.is_empty()
            && app.frames >= app.lock_surfaces.len() as u32
        {
            ready_sent = true;
            emit(
                "ready",
                &format!(
                    "\"frames\":{},\"surfaces\":{},\"elapsed_ms\":{}",
                    app.frames,
                    app.lock_surfaces.len(),
                    start.elapsed().as_millis()
                ),
            );
        }

        // --- unlock triggers ------------------------------------------
        let mut unlock_now = false;
        match rx.try_recv() {
            Ok(Cmd::Unlock) => {
                emit("stdin_unlock", "");
                unlock_now = true;
            }
            Ok(Cmd::Eof) => {
                emit("stdin_eof", "");
            }
            Err(mpsc::TryRecvError::Disconnected) => {}
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if !unlock_now && start.elapsed() >= args.hold {
            emit(
                "auto_unlock",
                &format!("\"held_ms\":{}", start.elapsed().as_millis()),
            );
            unlock_now = true;
        }
        if unlock_now && !unlocked {
            unlocked = true;
            app.unlock(&conn)?;
            // Sync: guarantees the server processed unlock_and_destroy.
            queue
                .roundtrip(&mut app)
                .map_err(|e| format!("roundtrip after unlock: {e}"))?;
            emit("released", &format!("\"locked_was\":{}", app.locked));
            // The compositor destroys the lock-surface role objects when the
            // lock object goes away, so the client must NOT destroy its
            // wl_surfaces afterwards: sway answers that with
            // "Protocol error 4 ... surface was destroyed before its role
            // object". Dropping the connection is the correct exit.
            let _ = conn.flush();
            // Drain a little so any `finished`/error is reported.
            let t = Instant::now();
            while t.elapsed() < Duration::from_millis(200) {
                let _ = pump(&conn, &mut queue, &mut app, 20);
            }
            break;
        }
    }

    if app.finished {
        emit("finished_after_unlock", "");
    }
    emit(
        "exit",
        &format!("\"frames\":{},\"unlocked\":{unlocked}", app.frames),
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// grim PPM histogram: proves what the *compositor* put on the output.
// ---------------------------------------------------------------------------
fn ppm_stats(path: &str) -> i32 {
    let mut data = Vec::new();
    match File::open(path).and_then(|mut f| f.read_to_end(&mut data)) {
        Ok(_) => {}
        Err(e) => {
            eprintln!("ppm-stats: {e}");
            return 2;
        }
    }
    // P6 header: magic, width height, maxval, all separated by whitespace,
    // possibly with '#' comments.
    let mut i = 0usize;
    let mut tokens: Vec<String> = Vec::new();
    while tokens.len() < 4 && i < data.len() {
        while i < data.len() && (data[i] as char).is_whitespace() {
            i += 1;
        }
        if i < data.len() && data[i] == b'#' {
            while i < data.len() && data[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let s = i;
        while i < data.len() && !(data[i] as char).is_whitespace() {
            i += 1;
        }
        if i > s {
            tokens.push(String::from_utf8_lossy(&data[s..i]).into_owned());
        }
    }
    i += 1; // single whitespace byte after maxval
    if tokens.len() < 4 || tokens[0] != "P6" {
        eprintln!("ppm-stats: not a P6 PPM");
        return 2;
    }
    let w: usize = tokens[1].parse().unwrap_or(0);
    let h: usize = tokens[2].parse().unwrap_or(0);
    let pixels = w * h;
    if pixels == 0 || data.len() < i + pixels * 3 {
        eprintln!("ppm-stats: truncated PPM ({} bytes for {w}x{h})", data.len());
        return 2;
    }
    let mut counts: HashMap<u32, usize> = HashMap::new();
    let mut red = 0usize;
    let mut green = 0usize;
    let mut dark = 0usize;
    for p in 0..pixels {
        let o = i + p * 3;
        let (r, g, b) = (data[o], data[o + 1], data[o + 2]);
        let rgb = ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
        *counts.entry(rgb).or_insert(0) += 1;
        if r >= 200 && g <= 60 && b <= 60 {
            red += 1;
        }
        if g >= 200 && r <= 60 && b <= 60 {
            green += 1;
        }
        if r <= 40 && g <= 40 && b <= 40 {
            dark += 1;
        }
    }
    let (top, topc) = counts
        .iter()
        .max_by_key(|(_, c)| **c)
        .map(|(k, c)| (*k, *c))
        .unwrap();
    println!(
        "{{\"file\":\"{}\",\"w\":{w},\"h\":{h},\"pixels\":{pixels},\"red\":{red},\
         \"green\":{green},\"dark\":{dark},\"distinct\":{},\"top\":\"#{top:06x}\",\
         \"top_count\":{topc}}}",
        path,
        counts.len()
    );
    0
}
