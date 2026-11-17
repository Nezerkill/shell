// CPU-side reference implementation, statistics and PNG dumping for r2-render.
// Everything here is deterministic: no threads, no randomness, no network.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

// ------------------------------------------------------------------- images
pub struct Img {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Img {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            rgba: vec![0; (w * h * 4) as usize],
        }
    }

    pub fn set(&mut self, x: u32, y: u32, p: [u8; 4]) {
        let i = ((y * self.w + x) * 4) as usize;
        self.rgba[i..i + 4].copy_from_slice(&p);
    }
}

/// `#[inline]` linear -> sRGB encode, so the dumped PNGs look like the shader
/// intended instead of like a linear-light dump.
pub fn srgb_encode(v: u8) -> u8 {
    let x = v as f32 / 255.0;
    let s = if x <= 0.003_130_8 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    };
    (s.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

pub fn crop(src: &[u8], sw: u32, sh: u32, x: u32, y: u32, w: u32, h: u32) -> Img {
    assert!(x + w <= sw && y + h <= sh, "crop out of bounds");
    let mut out = Img::new(w, h);
    for yy in 0..h {
        for xx in 0..w {
            let si = (((y + yy) * sw + (x + xx)) * 4) as usize;
            let mut px = [src[si], src[si + 1], src[si + 2], src[si + 3]];
            px[0] = srgb_encode(px[0]);
            px[1] = srgb_encode(px[1]);
            px[2] = srgb_encode(px[2]);
            px[3] = 255;
            out.set(xx, yy, px);
        }
    }
    out
}

pub fn downscale(src: &[u8], sw: u32, sh: u32, factor: u32) -> Img {
    let w = sw / factor;
    let h = sh / factor;
    let mut out = Img::new(w, h);
    for yy in 0..h {
        for xx in 0..w {
            let mut acc = [0u32; 3];
            for dy in 0..factor {
                for dx in 0..factor {
                    let si = (((yy * factor + dy) * sw + (xx * factor + dx)) * 4) as usize;
                    for c in 0..3 {
                        acc[c] += src[si + c] as u32;
                    }
                }
            }
            let n = factor * factor;
            out.set(
                xx,
                yy,
                [
                    srgb_encode((acc[0] / n) as u8),
                    srgb_encode((acc[1] / n) as u8),
                    srgb_encode((acc[2] / n) as u8),
                    255,
                ],
            );
        }
    }
    out
}

pub fn write_png(path: &Path, img: &Img) -> std::io::Result<u64> {
    let f = File::create(path)?;
    let w = BufWriter::new(f);
    let mut enc = png::Encoder::new(w, img.w, img.h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header()?;
    writer.write_image_data(&img.rgba)?;
    writer.finish()?;
    Ok(std::fs::metadata(path)?.len())
}

// ------------------------------------------------------- CPU blur reference
// Mirrors the WGSL chain exactly, including the 8-bit rounding of every
// intermediate render target: 2x2 box downsample -> quantise -> Gaussian H ->
// quantise -> Gaussian V -> quantise. Weights are recomputed from sigma in f64
// like the shader does in f32, so the result should agree to ~1 LSB.

#[inline]
fn q8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

#[inline]
fn dq8(q: u8) -> f32 {
    q as f32 / 255.0
}

pub struct CpuBlur {
    pub w: usize,
    pub h: usize,
    /// Quantised RGB of the final (vertical) pass, same encoding as the GPU
    /// Rgba8Unorm target we read back.
    pub rgb: Vec<[u8; 3]>,
}

pub fn cpu_blur(src_rgba: &[u8], sw: usize, sh: usize, radius: i32, sigma: f32) -> CpuBlur {
    assert!(
        sw.is_multiple_of(2) && sh.is_multiple_of(2),
        "downsample needs even size"
    );
    let dw = sw / 2;
    let dh = sh / 2;

    // 1. 2x2 box downsample + quantise (GPU: fs_blur_ds into Rgba8Unorm).
    let mut ds = vec![[0f32; 3]; dw * dh];
    for y in 0..dh {
        for x in 0..dw {
            let mut acc = [0f32; 3];
            for dy in 0..2 {
                for dx in 0..2 {
                    let si = ((y * 2 + dy) * sw + (x * 2 + dx)) * 4;
                    for c in 0..3 {
                        acc[c] += src_rgba[si + c] as f32 / 255.0;
                    }
                }
            }
            let mut px = [0f32; 3];
            for c in 0..3 {
                px[c] = dq8(q8(acc[c] * 0.25));
            }
            ds[y * dw + x] = px;
        }
    }

    // Weights, identical formula to the shader, normalised by their sum.
    let r = radius.max(0) as isize;
    let s2 = (sigma.max(0.5) as f64) * (sigma.max(0.5) as f64);
    let mut wts: Vec<f64> = Vec::with_capacity((2 * r + 1) as usize);
    let mut wsum = 0.0f64;
    for i in -r..=r {
        let w = (-0.5 * (i * i) as f64 / s2).exp();
        wts.push(w);
        wsum += w;
    }
    for w in wts.iter_mut() {
        *w /= wsum;
    }

    // 2. horizontal pass + quantise
    let mut horiz = vec![[0f32; 3]; dw * dh];
    for y in 0..dh {
        for x in 0..dw {
            let mut acc = [0f32; 3];
            for (k, w) in wts.iter().enumerate() {
                let sx = (x as isize + k as isize - r).clamp(0, dw as isize - 1) as usize;
                let px = ds[y * dw + sx];
                for c in 0..3 {
                    acc[c] += px[c] * *w as f32;
                }
            }
            let mut px = [0f32; 3];
            for c in 0..3 {
                px[c] = dq8(q8(acc[c]));
            }
            horiz[y * dw + x] = px;
        }
    }

    // 3. vertical pass + quantise (this is what we compare against)
    let mut out = vec![[0u8; 3]; dw * dh];
    for y in 0..dh {
        for x in 0..dw {
            let mut acc = [0f32; 3];
            for (k, w) in wts.iter().enumerate() {
                let sy = (y as isize + k as isize - r).clamp(0, dh as isize - 1) as usize;
                let px = horiz[sy * dw + x];
                for c in 0..3 {
                    acc[c] += px[c] * *w as f32;
                }
            }
            out[y * dw + x] = [q8(acc[0]), q8(acc[1]), q8(acc[2])];
        }
    }

    CpuBlur {
        w: dw,
        h: dh,
        rgb: out,
    }
}

// ---------------------------------------------------------------- statistics
/// Nearest-rank percentile on a sorted copy (p in 0..=1).
pub fn pct(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let rank = ((p * (s.len() as f64 - 1.0)).round() as usize).min(s.len() - 1);
    s[rank]
}

pub fn max_of(v: &[f64]) -> f64 {
    v.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
}

// ------------------------------------------------------------------- checker
pub struct Checker {
    pub taken: usize,
    pub failed: usize,
}

impl Checker {
    pub fn new() -> Self {
        Self { taken: 0, failed: 0 }
    }

    pub fn check(&mut self, name: &str, ok: bool, detail: &str) {
        self.taken += 1;
        if ok {
            println!("  PASS  {name}: {detail}");
        } else {
            self.failed += 1;
            println!("  FAIL  {name}: {detail}");
        }
    }
}
