//! Native framed game embedding ("Inside Clade").
//!
//! When `"When Minecraft starts"` is `"embed"`, Clade keeps its own frameless window open as a host
//! frame and splits the host client area into two **disjoint** regions:
//!
//! - Top strip `(0, 0, client_w, header_px)`: WebView2 (`WRY_WEBVIEW` + `ICoreWebView2Controller`)
//!   renders `WindowChrome` (`Clade` wordmark, drag strip, Minimize / Maximize / Close buttons).
//! - Stage `(0, header_px, client_w, client_h - header_px)` in host client coordinates, mapped to
//!   screen coordinates: the running Minecraft window is styled as a borderless top-level window
//!   **owned** by Clade's host (`GWLP_HWNDPARENT`, `WS_EX_APPWINDOW` stripped, flush below the strip).
//!
//! Keeping Minecraft an owned top-level `WS_POPUP` instead of a `WS_CHILD` preserves Chromium
//! DirectComposition/Mica in WebView2 AND allows Windows to return the game HWND from
//! `GetActiveWindow()` / `SetForegroundWindow()` so GLFW's `"Back to Game"` pointer grab works.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

/// Event announcing a change to the framed game embedding state.
pub const EMBED_EVENT: &str = "game-embed-state";

/// Logical height (CSS pixels at 100% DPI) of the custom Clade header strip in framed mode.
pub const HEADER_HEIGHT_LOGICAL_PX: i32 = 44;

const POLL_IDLE: Duration = Duration::from_millis(4);
const POLL_ACTIVE: Duration = Duration::from_millis(24);
const ATTACH_TIMEOUT: Duration = Duration::from_secs(2);

/// Physical stage rectangle inside Clade's main window where the game window sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub radius: u32,
}

impl StageBounds {
    pub fn clamped(x: i32, y: i32, width: u32, height: u32, radius: u32) -> Self {
        let w = width.clamp(64, 16_384);
        let h = height.clamp(64, 16_384);
        let r = radius.min(w.min(h) / 2);
        Self {
            x,
            y,
            width: w,
            height: h,
            radius: r,
        }
    }
}

/// Converts host-client [`StageBounds`] at client origin `(origin_x, origin_y)` into screen `(x, y, width, height)`.
pub fn stage_to_screen_rect(
    origin_x: i32,
    origin_y: i32,
    stage: StageBounds,
) -> (i32, i32, i32, i32) {
    let w = i32::try_from(stage.width).unwrap_or(1280).max(64);
    let h = i32::try_from(stage.height).unwrap_or(720).max(64);
    (origin_x + stage.x, origin_y + stage.y, w, h)
}

/// Disjoint native rectangles for WebView2's top strip and Minecraft's stage in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FramedLayout {
    pub webview_width: i32,
    pub webview_height: i32,
    pub stage: StageBounds,
}

/// Computes non-overlapping top-strip and stage bounds from host client pixels and monitor DPI.
pub fn compute_framed_layout(client_w: i32, client_h: i32, dpi: u32) -> Option<FramedLayout> {
    if client_w < 64 || client_h < 64 {
        return None;
    }
    let scale = (f64::from(dpi.max(48)) / 96.0).clamp(0.5, 4.0);
    let max_header = (client_h - 64).max(24);
    let header_px =
        ((f64::from(HEADER_HEIGHT_LOGICAL_PX) * scale).round() as i32).clamp(24, max_header);
    let stage_h = (client_h - header_px).max(64) as u32;
    Some(FramedLayout {
        webview_width: client_w,
        webview_height: header_px,
        stage: StageBounds::clamped(0, header_px, client_w as u32, stage_h, 0),
    })
}

/// UI-facing embedding snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameEmbedStatus {
    pub active: bool,
    pub watching: bool,
    pub instance_id: Option<String>,
    pub maximized: bool,
    pub closing: bool,
}

/// Top-down BGRX/XRGB pixel capture of the game stage kept strictly in RAM for close animations.
#[derive(Clone)]
pub struct CapturedStage {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u32>,
}

const REVEAL_HOLD_MS: u64 = 180;
const REVEAL_DURATION_MS: u64 = 600;
const CLOSE_HOLD_MS: u64 = 130;
const CLOSE_DURATION_MS: u64 = 490;

/// Smooth rounded-viewport reveal geometry and premultiplied-alpha veil opacities for one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevealFrame {
    pub inner_left: i32,
    pub inner_top: i32,
    pub inner_right: i32,
    pub inner_bottom: i32,
    pub radius: i32,
    pub inner_alpha: u8,
    pub outer_alpha: u8,
    pub border_alpha: u8,
    pub shadow_alpha: u8,
    pub shadow_spread: i32,
}

/// Smooth rounded-viewport fold-out geometry for closing an embedded game stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloseFrame {
    pub inner_left: i32,
    pub inner_top: i32,
    pub inner_right: i32,
    pub inner_bottom: i32,
    pub radius: i32,
    pub card_alpha: u8,
    pub border_alpha: u8,
    pub shadow_alpha: u8,
    pub shadow_spread: i32,
}

/// Resolves whether native embedded-window reveal animation should run.
pub fn should_animate_embed(mode: &str, os_animations_enabled: Option<bool>) -> bool {
    match mode {
        "on" => false,
        "off" => true,
        _ => os_animations_enabled.unwrap_or(true),
    }
}

/// Computes the eased viewport expansion and two-zone warm-paper dissolve at normalized progress `t`.
pub fn compute_reveal_frame(width: i32, height: i32, progress: f32) -> RevealFrame {
    let w = width.max(1);
    let h = height.max(1);
    let t = progress.clamp(0.0, 1.0);
    if t >= 1.0 {
        return RevealFrame {
            inner_left: 0,
            inner_top: 0,
            inner_right: w,
            inner_bottom: h,
            radius: 0,
            inner_alpha: 0,
            outer_alpha: 0,
            border_alpha: 0,
            shadow_alpha: 0,
            shadow_spread: 0,
        };
    }

    let hold = (REVEAL_HOLD_MS as f32) / (REVEAL_DURATION_MS as f32);
    let u = ((t - hold) / (1.0 - hold)).clamp(0.0, 1.0);
    if u <= 0.0 {
        return RevealFrame {
            inner_left: 0,
            inner_top: 0,
            inner_right: w,
            inner_bottom: h,
            radius: 0,
            inner_alpha: 255,
            outer_alpha: 255,
            border_alpha: 0,
            shadow_alpha: 0,
            shadow_spread: 0,
        };
    }

    let eased = 1.0 - (1.0 - u).powi(3);

    let max_pad_x = ((w as f32) * 0.045)
        .clamp(12.0, 64.0)
        .min((w / 4).max(0) as f32);
    let max_pad_y = ((h as f32) * 0.045)
        .clamp(10.0, 46.0)
        .min((h / 4).max(0) as f32);
    let pad_x = ((1.0 - eased) * max_pad_x).round() as i32;
    let pad_y = ((1.0 - eased) * max_pad_y).round() as i32;

    let inner_w = (w - pad_x * 2).max(16.min(w));
    let inner_h = (h - pad_y * 2).max(16.min(h));
    let inner_left = ((w - inner_w) / 2).clamp(0, w - inner_w);
    let inner_right = inner_left + inner_w;

    let lift_y = ((1.0 - eased) * 14.0).round() as i32;
    let inner_top = ((h - inner_h) / 2 + lift_y).clamp(0, h - inner_h);
    let inner_bottom = inner_top + inner_h;

    let radius = (((1.0 - eased) * 18.0).round() as i32).clamp(0, inner_w.min(inner_h) / 2);

    let inner_t = (u / 0.34).clamp(0.0, 1.0);
    let inner_e = 1.0 - (1.0 - inner_t).powi(3);
    let inner_alpha = ((1.0 - inner_e) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    let visibility = 1.0 - (f32::from(inner_alpha) / 255.0);

    let outer_alpha = if u <= 0.72 {
        255
    } else {
        let ot = ((u - 0.72) / 0.28).clamp(0.0, 1.0);
        let oe = 1.0 - (1.0 - ot).powi(3);
        ((1.0 - oe) * 255.0 + 0.5).clamp(0.0, 255.0) as u8
    };

    let edge_weight = visibility * (1.0 - eased);
    let shadow_alpha = if u >= 0.96 || edge_weight <= 0.005 {
        0
    } else {
        (edge_weight * 48.0).round().clamp(0.0, 255.0) as u8
    };
    let shadow_spread = if shadow_alpha == 0 {
        0
    } else {
        ((1.0 - eased) * 12.0 + 3.0).round().clamp(0.0, 16.0) as i32
    };

    let border_alpha = if u >= 0.96 || edge_weight <= 0.005 {
        0
    } else {
        (edge_weight * 56.0).round().clamp(0.0, 255.0) as u8
    };

    RevealFrame {
        inner_left,
        inner_top,
        inner_right,
        inner_bottom,
        radius,
        inner_alpha,
        outer_alpha,
        border_alpha,
        shadow_alpha,
        shadow_spread,
    }
}

/// Computes the close hold-beat and fold-out viewport shrink geometry at normalized progress `t`.
pub fn compute_close_frame(width: i32, height: i32, progress: f32) -> CloseFrame {
    let w = width.max(1);
    let h = height.max(1);
    let t = progress.clamp(0.0, 1.0);
    if t >= 1.0 {
        return CloseFrame {
            inner_left: 0,
            inner_top: 0,
            inner_right: w,
            inner_bottom: h,
            radius: 0,
            card_alpha: 0,
            border_alpha: 0,
            shadow_alpha: 0,
            shadow_spread: 0,
        };
    }

    let hold = (CLOSE_HOLD_MS as f32) / (CLOSE_DURATION_MS as f32);
    let u = ((t - hold) / (1.0 - hold)).clamp(0.0, 1.0);
    if u <= 0.0 {
        return CloseFrame {
            inner_left: 0,
            inner_top: 0,
            inner_right: w,
            inner_bottom: h,
            radius: 0,
            card_alpha: 255,
            border_alpha: 0,
            shadow_alpha: 0,
            shadow_spread: 0,
        };
    }

    let eased = 1.0 - (1.0 - u).powi(3);
    let max_pad_x = ((w as f32) * 0.075)
        .clamp(18.0, 96.0)
        .min((w / 4).max(0) as f32);
    let max_pad_y = ((h as f32) * 0.075)
        .clamp(14.0, 70.0)
        .min((h / 4).max(0) as f32);
    let pad_x = (eased * max_pad_x).round() as i32;
    let pad_y = (eased * max_pad_y).round() as i32;

    let inner_w = (w - pad_x * 2).max(16.min(w));
    let inner_h = (h - pad_y * 2).max(16.min(h));
    let inner_left = ((w - inner_w) / 2).clamp(0, w - inner_w);
    let inner_right = inner_left + inner_w;

    let drop_y = (eased * 18.0).round() as i32;
    let inner_top = ((h - inner_h) / 2 + drop_y).clamp(0, h - inner_h);
    let inner_bottom = inner_top + inner_h;

    let radius = ((eased * 20.0).round() as i32).clamp(0, inner_w.min(inner_h) / 2);

    let card_alpha = if u <= 0.06 {
        255
    } else {
        let fade = ((u - 0.06) / 0.94).clamp(0.0, 1.0).powf(1.25);
        ((1.0 - fade) * 255.0).round().clamp(0.0, 255.0) as u8
    };

    let shadow_spread = if u <= 0.01 {
        0
    } else {
        (eased * 14.0 + 3.0).round().clamp(0.0, 18.0) as i32
    };
    let shadow_alpha = if u <= 0.01 {
        0
    } else {
        ((u * std::f32::consts::PI).sin() * 54.0)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    let border_alpha = if u <= 0.01 {
        0
    } else {
        ((u * std::f32::consts::PI).sin() * 48.0)
            .round()
            .clamp(0.0, 255.0) as u8
    };

    CloseFrame {
        inner_left,
        inner_top,
        inner_right,
        inner_bottom,
        radius,
        card_alpha,
        border_alpha,
        shadow_alpha,
        shadow_spread,
    }
}

/// Computes `(c * a + 127) / 255` without hardware integer division for all bytes `0..=255`.
#[inline(always)]
pub fn mul_div_255(c: u32, a: u32) -> u32 {
    let t = c * a + 128;
    (t + (t >> 8)) >> 8
}

/// Encodes straight `(r, g, b, a)` as little-endian premultiplied `BGRA` for `UpdateLayeredWindow`.
#[inline(always)]
pub fn premul_bgra(r: u8, g: u8, b: u8, a: u8) -> u32 {
    if a == 255 {
        return (255u32 << 24) | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
    }
    if a == 0 {
        return 0;
    }
    let a16 = u32::from(a);
    let pm_r = mul_div_255(u32::from(r), a16);
    let pm_g = mul_div_255(u32::from(g), a16);
    let pm_b = mul_div_255(u32::from(b), a16);
    (a16 << 24) | (pm_r << 16) | (pm_g << 8) | pm_b
}

#[inline(always)]
fn over_premul(fg_r: u8, fg_g: u8, fg_b: u8, fg_a: u8, bg_premul: u32) -> u32 {
    if fg_a == 0 {
        return bg_premul;
    }
    if fg_a == 255 {
        return (255u32 << 24) | (u32::from(fg_r) << 16) | (u32::from(fg_g) << 8) | u32::from(fg_b);
    }
    let fa = u32::from(fg_a);
    let inv = 255 - fa;
    let ba = (bg_premul >> 24) & 0xFF;
    let br = (bg_premul >> 16) & 0xFF;
    let bg = (bg_premul >> 8) & 0xFF;
    let bb = bg_premul & 0xFF;

    let fr = mul_div_255(u32::from(fg_r), fa);
    let fg_c = mul_div_255(u32::from(fg_g), fa);
    let fb = mul_div_255(u32::from(fg_b), fa);

    let out_a = (fa + mul_div_255(ba, inv)).min(255);
    let out_r = (fr + mul_div_255(br, inv)).min(out_a);
    let out_g = (fg_c + mul_div_255(bg, inv)).min(out_a);
    let out_b = (fb + mul_div_255(bb, inv)).min(out_a);
    (out_a << 24) | (out_r << 16) | (out_g << 8) | out_b
}

#[inline(always)]
fn lerp_premul(bg: u32, fg: u32, w_in: f32) -> u32 {
    if w_in <= 0.0 {
        return bg;
    }
    if w_in >= 1.0 {
        return fg;
    }
    let w = (w_in * 255.0 + 0.5).clamp(0.0, 255.0) as u32;
    let inv = 255 - w;
    let a = mul_div_255((bg >> 24) & 0xFF, inv) + mul_div_255((fg >> 24) & 0xFF, w);
    let r = (mul_div_255((bg >> 16) & 0xFF, inv) + mul_div_255((fg >> 16) & 0xFF, w)).min(a);
    let g = (mul_div_255((bg >> 8) & 0xFF, inv) + mul_div_255((fg >> 8) & 0xFF, w)).min(a);
    let b = (mul_div_255(bg & 0xFF, inv) + mul_div_255(fg & 0xFF, w)).min(a);
    (a << 24) | (r << 16) | (g << 8) | b
}

#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn rounded_rect_sdf(px: f32, py: f32, l: f32, t: f32, r: f32, b: f32, rad: f32) -> f32 {
    if rad > 0.0 {
        let clx = l + rad;
        let crx = r - rad;
        let cty = t + rad;
        let cby = b - rad;
        let left_side = px < clx;
        let right_side = px > crx;
        let top_side = py < cty;
        let bot_side = py > cby;
        if (left_side || right_side) && (top_side || bot_side) {
            let cx = if left_side { clx } else { crx };
            let cy = if top_side { cty } else { cby };
            let dx = px - cx;
            let dy = py - cy;
            return (dx * dx + dy * dy).sqrt() - rad;
        }
        return (l - px).max(px - r).max(t - py).max(py - b);
    }
    let dx = (l - px).max(px - r);
    let dy = (t - py).max(py - b);
    if dx > 0.0 && dy > 0.0 {
        (dx * dx + dy * dy).sqrt()
    } else {
        dx.max(dy)
    }
}

#[inline(always)]
fn edge_shadow_alpha(d: f32, shadow_alpha: u8, shadow_spread: i32) -> u8 {
    if d <= 0.0 || shadow_alpha == 0 || shadow_spread <= 0 {
        return 0;
    }
    let t = (1.0 - d / (shadow_spread as f32)).clamp(0.0, 1.0);
    (f32::from(shadow_alpha) * t * t + 0.5).clamp(0.0, 255.0) as u8
}

#[inline(always)]
fn edge_rim_alpha(d: f32, border_alpha: u8) -> u8 {
    if border_alpha == 0 {
        return 0;
    }
    let w_rim = (1.0 - d.abs()).clamp(0.0, 1.0);
    (f32::from(border_alpha) * w_rim + 0.5).clamp(0.0, 255.0) as u8
}

#[inline(always)]
fn reveal_pixel(d: f32, outer_px: u32, inner_px: u32, frame: RevealFrame) -> u32 {
    let sh_a = edge_shadow_alpha(d, frame.shadow_alpha, frame.shadow_spread);
    let bg_px = if sh_a > 0 {
        over_premul(43, 38, 34, sh_a, outer_px)
    } else {
        outer_px
    };
    let base_px = lerp_premul(bg_px, inner_px, (0.5 - d).clamp(0.0, 1.0));
    let rim_a = edge_rim_alpha(d, frame.border_alpha);
    over_premul(43, 38, 34, rim_a, base_px)
}

#[inline(always)]
fn close_row_color(y: usize, card_t: usize, card_h: usize) -> (u8, u8, u8) {
    let rel = (y.saturating_sub(card_t) as f32) / (card_h.max(1) as f32);
    let t = rel.clamp(0.0, 1.0);
    (
        (52.0 - t * 16.0 + 0.5) as u8,
        (46.0 - t * 14.0 + 0.5) as u8,
        (41.0 - t * 13.0 + 0.5) as u8,
    )
}

struct PaintBounds {
    l: usize,
    r: usize,
    t: usize,
    b: usize,
    rad: usize,
    flat_l: usize,
    flat_r: usize,
    in_l: usize,
    in_r: usize,
    band_l: usize,
    band_r: usize,
    band_t: usize,
    band_b: usize,
}

#[allow(clippy::too_many_arguments)]
fn compute_paint_bounds(
    w: usize,
    h: usize,
    inner_left: i32,
    inner_top: i32,
    inner_right: i32,
    inner_bottom: i32,
    radius: i32,
    shadow_alpha: u8,
    shadow_spread: i32,
) -> PaintBounds {
    let l = inner_left.clamp(0, w as i32) as usize;
    let r = inner_right.clamp(l as i32, w as i32) as usize;
    let t = inner_top.clamp(0, h as i32) as usize;
    let b = inner_bottom.clamp(t as i32, h as i32) as usize;
    let rad = (radius.max(0) as usize).min((r - l) / 2).min((b - t) / 2);
    let flat_l = (l + rad).min(r);
    let flat_r = r.saturating_sub(rad).max(flat_l);
    let in_l = flat_l.max((l + 1).min(r));
    let in_r = flat_r.min(r.saturating_sub(1)).max(in_l);
    let spread = if shadow_alpha > 0 && shadow_spread > 0 {
        shadow_spread as usize
    } else {
        0
    };
    PaintBounds {
        l,
        r,
        t,
        b,
        rad,
        flat_l,
        flat_r,
        in_l,
        in_r,
        band_l: l.saturating_sub(spread),
        band_r: (r + spread).min(w),
        band_t: t.saturating_sub(spread),
        band_b: (b + spread).min(h),
    }
}

/// Fills a top-down 32-bit premultiplied BGRA pixel buffer for one [`RevealFrame`].
pub fn paint_reveal_bgra(buf: &mut [u32], width: i32, height: i32, frame: RevealFrame) {
    if width <= 0 || height <= 0 {
        return;
    }
    let w = width as usize;
    let h = height as usize;
    if buf.len() < w.saturating_mul(h) {
        return;
    }
    let buf = &mut buf[..w * h];

    if frame.inner_alpha == 0
        && frame.outer_alpha == 0
        && frame.border_alpha == 0
        && frame.shadow_alpha == 0
    {
        buf.fill(0);
        return;
    }
    let outer_px = premul_bgra(243, 237, 227, frame.outer_alpha);
    let inner_px = premul_bgra(243, 237, 227, frame.inner_alpha);
    if frame.inner_alpha == frame.outer_alpha && frame.border_alpha == 0 && frame.shadow_alpha == 0
    {
        buf.fill(outer_px);
        return;
    }

    let pb = compute_paint_bounds(
        w,
        h,
        frame.inner_left,
        frame.inner_top,
        frame.inner_right,
        frame.inner_bottom,
        frame.radius,
        frame.shadow_alpha,
        frame.shadow_spread,
    );
    let lf = pb.l as f32;
    let rf = pb.r as f32;
    let tf = pb.t as f32;
    let bf = pb.b as f32;
    let rad_f = pb.rad as f32;

    for (y, row) in buf.chunks_exact_mut(w).enumerate() {
        if pb.l >= pb.r || pb.t >= pb.b || y < pb.band_t || y >= pb.band_b {
            row.fill(outer_px);
            continue;
        }
        if pb.band_l > 0 {
            row[..pb.band_l].fill(outer_px);
        }
        if pb.band_r < w {
            row[pb.band_r..].fill(outer_px);
        }

        let py = y as f32 + 0.5;
        let is_interior = y > pb.t && y + 1 < pb.b && pb.in_l < pb.in_r;
        let (mid_l, mid_r, mid_px) = if is_interior {
            (pb.in_l, pb.in_r, inner_px)
        } else if pb.flat_l < pb.flat_r {
            let row_d = if y < pb.t {
                tf - py
            } else if y >= pb.b {
                py - bf
            } else if y == pb.t {
                tf - py
            } else {
                py - bf
            };
            (
                pb.flat_l,
                pb.flat_r,
                reveal_pixel(row_d, outer_px, inner_px, frame),
            )
        } else {
            (pb.band_r, pb.band_r, outer_px)
        };

        if mid_l < mid_r {
            row[mid_l..mid_r].fill(mid_px);
        }
        for x in (pb.band_l..mid_l).chain(mid_r..pb.band_r) {
            let d = rounded_rect_sdf(x as f32 + 0.5, py, lf, tf, rf, bf, rad_f);
            row[x] = reveal_pixel(d, outer_px, inner_px, frame);
        }
    }
}

/// Fills a top-down 32-bit premultiplied BGRA pixel buffer for one [`CloseFrame`].
pub fn paint_close_bgra(
    buf: &mut [u32],
    width: i32,
    height: i32,
    frame: CloseFrame,
    snapshot: Option<&CapturedStage>,
) {
    if width <= 0 || height <= 0 {
        return;
    }
    let w = width as usize;
    let h = height as usize;
    if buf.len() < w.saturating_mul(h) {
        return;
    }
    let buf = &mut buf[..w * h];

    if frame.card_alpha == 0 && frame.border_alpha == 0 && frame.shadow_alpha == 0 {
        buf.fill(0);
        return;
    }

    let pb = compute_paint_bounds(
        w,
        h,
        frame.inner_left,
        frame.inner_top,
        frame.inner_right,
        frame.inner_bottom,
        frame.radius,
        frame.shadow_alpha,
        frame.shadow_spread,
    );
    if pb.l >= pb.r || pb.t >= pb.b {
        buf.fill(0);
        return;
    }

    let snap = snapshot.filter(|s| {
        s.width > 0
            && s.height > 0
            && s.pixels.len() >= (s.width as usize).saturating_mul(s.height as usize)
    });
    let card_w = (pb.r - pb.l).max(1);
    let card_h = (pb.b - pb.t).max(1);
    let lf = pb.l as f32;
    let rf = pb.r as f32;
    let tf = pb.t as f32;
    let bf = pb.b as f32;
    let rad_f = pb.rad as f32;

    for (y, row) in buf.chunks_exact_mut(w).enumerate() {
        if y < pb.band_t || y >= pb.band_b {
            row.fill(0);
            continue;
        }
        if pb.band_l > 0 {
            row[..pb.band_l].fill(0);
        }
        if pb.band_r < w {
            row[pb.band_r..].fill(0);
        }

        let py = y as f32 + 0.5;
        let is_interior = y > pb.t && y + 1 < pb.b && pb.in_l < pb.in_r;
        let sy_row = snap.map(|s| {
            let cy = y.saturating_sub(pb.t).min(card_h - 1);
            let sy = (cy * (s.height as usize) / card_h).min(s.height as usize - 1);
            &s.pixels[sy * (s.width as usize)..(sy + 1) * (s.width as usize)]
        });
        let (fb_r, fb_g, fb_b) = close_row_color(y, pb.t, card_h);

        let (edge_l_end, edge_r_start) = if is_interior {
            if let Some(src_row) = sy_row {
                let sw = src_row.len();
                let step_fp = (sw << 16) / card_w;
                let mut x_fp = (pb.in_l - pb.l) * step_fp;
                let a = frame.card_alpha;
                if a == 255 {
                    for dst in &mut row[pb.in_l..pb.in_r] {
                        let rgb = src_row[(x_fp >> 16).min(sw - 1)] & 0x00FF_FFFF;
                        *dst = 0xFF00_0000 | rgb;
                        x_fp += step_fp;
                    }
                } else if a == 0 {
                    row[pb.in_l..pb.in_r].fill(0);
                } else {
                    let a16 = u32::from(a);
                    for dst in &mut row[pb.in_l..pb.in_r] {
                        let rgb = src_row[(x_fp >> 16).min(sw - 1)];
                        let r_pm = mul_div_255((rgb >> 16) & 0xFF, a16);
                        let g_pm = mul_div_255((rgb >> 8) & 0xFF, a16);
                        let b_pm = mul_div_255(rgb & 0xFF, a16);
                        *dst = (a16 << 24) | (r_pm << 16) | (g_pm << 8) | b_pm;
                        x_fp += step_fp;
                    }
                }
            } else {
                row[pb.in_l..pb.in_r].fill(premul_bgra(fb_r, fb_g, fb_b, frame.card_alpha));
            }
            (pb.in_l, pb.in_r)
        } else if (y < pb.t || y >= pb.b) && pb.flat_l < pb.flat_r {
            let d = if y < pb.t { tf - py } else { py - bf };
            let sh_a = edge_shadow_alpha(d, frame.shadow_alpha, frame.shadow_spread);
            let sh_px = if sh_a > 0 {
                premul_bgra(43, 38, 34, sh_a)
            } else {
                0
            };
            let rim_a = edge_rim_alpha(d, frame.border_alpha);
            row[pb.flat_l..pb.flat_r].fill(over_premul(246, 240, 230, rim_a, sh_px));
            (pb.flat_l, pb.flat_r)
        } else if snap.is_none() && (y == pb.t || y + 1 == pb.b) && pb.flat_l < pb.flat_r {
            let d = if y == pb.t { tf - py } else { py - bf };
            let w_in = (0.5 - d).clamp(0.0, 1.0);
            let card_px = premul_bgra(fb_r, fb_g, fb_b, frame.card_alpha);
            let base_px = lerp_premul(0, card_px, w_in);
            let rim_a = edge_rim_alpha(d, frame.border_alpha);
            row[pb.flat_l..pb.flat_r].fill(over_premul(246, 240, 230, rim_a, base_px));
            (pb.flat_l, pb.flat_r)
        } else {
            (pb.band_r, pb.band_r)
        };

        for x in (pb.band_l..edge_l_end).chain(edge_r_start..pb.band_r) {
            let d = rounded_rect_sdf(x as f32 + 0.5, py, lf, tf, rf, bf, rad_f);
            let sh_a = edge_shadow_alpha(d, frame.shadow_alpha, frame.shadow_spread);
            let sh_px = if sh_a > 0 {
                premul_bgra(43, 38, 34, sh_a)
            } else {
                0
            };
            let w_in = (0.5 - d).clamp(0.0, 1.0);
            let card_px = if w_in > 0.0 {
                let (cr, cg, cb) = if let (Some(src_row), Some(s)) = (sy_row, snap) {
                    let cx = x.saturating_sub(pb.l).min(card_w - 1);
                    let sx = (cx * (s.width as usize) / card_w).min(src_row.len() - 1);
                    let rgb = src_row[sx];
                    (
                        ((rgb >> 16) & 0xFF) as u8,
                        ((rgb >> 8) & 0xFF) as u8,
                        (rgb & 0xFF) as u8,
                    )
                } else {
                    (fb_r, fb_g, fb_b)
                };
                premul_bgra(cr, cg, cb, frame.card_alpha)
            } else {
                0
            };
            let base_px = lerp_premul(sh_px, card_px, w_in);
            let rim_a = edge_rim_alpha(d, frame.border_alpha);
            row[x] = over_premul(246, 240, 230, rim_a, base_px);
        }
    }
}

pub struct VeilSlot {
    hwnd: AtomicU64,
    cancel: Mutex<Option<Arc<AtomicBool>>>,
}

impl VeilSlot {
    fn new() -> Self {
        Self {
            hwnd: AtomicU64::new(0),
            cancel: Mutex::new(None),
        }
    }

    fn hwnd(&self) -> u64 {
        self.hwnd.load(Ordering::Acquire)
    }

    fn set_active(&self, hwnd: u64, cancel: Arc<AtomicBool>) {
        *self.cancel.lock().unwrap_or_else(|e| e.into_inner()) = Some(cancel);
        self.hwnd.store(hwnd, Ordering::Release);
    }

    fn clear_if_matches(&self, hwnd: u64, cancel: &Arc<AtomicBool>) {
        let _ = self
            .hwnd
            .compare_exchange(hwnd, 0, Ordering::AcqRel, Ordering::Acquire);
        let mut guard = self.cancel.lock().unwrap_or_else(|e| e.into_inner());
        if guard.as_ref().is_some_and(|c| Arc::ptr_eq(c, cancel)) {
            *guard = None;
        }
    }
}

struct ActiveSession {
    instance_id: String,
    pid: u32,
    game_win: AtomicU64,
    veil_slot: VeilSlot,
    attached: AtomicBool,
    ever_attached: AtomicBool,
    strip_active: AtomicBool,
    prev_host_clipchildren: AtomicBool,
    maximized: AtomicBool,
    attach_win_rect: Mutex<Option<(i32, i32, i32, i32)>>,
    pre_expand_win_rect: Mutex<Option<(i32, i32, i32, i32)>>,
    close_snapshot: Mutex<Option<CapturedStage>>,
    close_requested: AtomicBool,
    close_attempts: AtomicU8,
    focus_requested: AtomicBool,
    stop: Arc<AtomicBool>,
}

impl ActiveSession {
    fn new(instance_id: String, pid: u32) -> Self {
        Self {
            instance_id,
            pid,
            game_win: AtomicU64::new(0),
            veil_slot: VeilSlot::new(),
            attached: AtomicBool::new(false),
            ever_attached: AtomicBool::new(false),
            strip_active: AtomicBool::new(false),
            prev_host_clipchildren: AtomicBool::new(false),
            maximized: AtomicBool::new(false),
            attach_win_rect: Mutex::new(None),
            pre_expand_win_rect: Mutex::new(None),
            close_snapshot: Mutex::new(None),
            close_requested: AtomicBool::new(false),
            close_attempts: AtomicU8::new(0),
            focus_requested: AtomicBool::new(false),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    fn status(&self) -> GameEmbedStatus {
        if self.attached.load(Ordering::Acquire) {
            return GameEmbedStatus {
                active: true,
                watching: false,
                instance_id: Some(self.instance_id.clone()),
                maximized: self.maximized.load(Ordering::Acquire),
                closing: self.close_attempts.load(Ordering::Acquire) > 0,
            };
        }
        if !self.stop.load(Ordering::Acquire) && !self.ever_attached.load(Ordering::Acquire) {
            return GameEmbedStatus {
                active: false,
                watching: true,
                instance_id: Some(self.instance_id.clone()),
                maximized: false,
                closing: false,
            };
        }
        GameEmbedStatus::default()
    }

    /// Marks the currently embedded HWND detached while keeping session history (`ever_attached`,
    /// `attach_win_rect`, `prev_host_clipchildren`, `close_snapshot`) until teardown finishes.
    fn mark_window_detached(&self) -> bool {
        #[cfg(windows)]
        win32::dismiss_veil(self);
        self.game_win.store(0, Ordering::Release);
        self.maximized.store(false, Ordering::Release);
        self.close_requested.store(false, Ordering::Release);
        self.close_attempts.store(0, Ordering::Release);
        self.focus_requested.store(false, Ordering::Release);
        let was_attached = self.attached.swap(false, Ordering::AcqRel);
        let was_strip = self.strip_active.swap(false, Ordering::AcqRel);
        was_attached || was_strip
    }
}

/// Snapshot saved when ending a framed session so host geometry and styles restore idempotently.
struct FinishedEmbed {
    was_attached: bool,
    ever_attached: bool,
    strip_active: bool,
    attach_win_rect: Option<(i32, i32, i32, i32)>,
    prev_host_clipchildren: bool,
    close_snapshot: Option<CapturedStage>,
}

/// State managing framed game attachment and stage coordinates.
pub struct EmbedState {
    bounds: Mutex<Option<StageBounds>>,
    bounds_seq: AtomicU64,
    active: Mutex<Option<Arc<ActiveSession>>>,
    close_veil: Arc<VeilSlot>,
}

impl EmbedState {
    pub fn new() -> Self {
        Self {
            bounds: Mutex::new(None),
            bounds_seq: AtomicU64::new(0),
            active: Mutex::new(None),
            close_veil: Arc::new(VeilSlot::new()),
        }
    }

    pub fn set_bounds(&self, bounds: StageBounds) {
        *self.bounds.lock().unwrap_or_else(|e| e.into_inner()) = Some(bounds);
        self.bounds_seq.fetch_add(1, Ordering::Release);
    }

    pub fn current_bounds(&self) -> (Option<StageBounds>, u64) {
        let seq = self.bounds_seq.load(Ordering::Acquire);
        let bounds = *self.bounds.lock().unwrap_or_else(|e| e.into_inner());
        (bounds, seq)
    }

    pub fn status(&self) -> GameEmbedStatus {
        let guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()
            .map_or_else(GameEmbedStatus::default, |s| s.status())
    }

    pub fn is_active(&self) -> bool {
        self.status().active
    }

    fn begin(&self, instance_id: String, pid: u32) -> Arc<ActiveSession> {
        #[cfg(windows)]
        win32::dismiss_veil_slot(&self.close_veil);
        let next = Arc::new(ActiveSession::new(instance_id, pid));
        let prev = {
            let mut guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
            guard.replace(next.clone())
        };
        if let Some(prev) = prev {
            prev.stop.store(true, Ordering::Release);
            prev.strip_active.store(false, Ordering::Release);
            #[cfg(windows)]
            win32::dismiss_veil(&prev);
            let prev_mc = prev.game_win.swap(0, Ordering::AcqRel);
            if prev.attached.swap(false, Ordering::AcqRel) && prev_mc != 0 {
                #[cfg(windows)]
                win32::detach_to_standalone(prev_mc as usize as win32::HWND);
            }
        }
        next
    }

    fn current_session(&self) -> Option<Arc<ActiveSession>> {
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn finish(&self, instance_id: &str) -> Option<FinishedEmbed> {
        let mut guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        let matches = guard.as_ref().is_some_and(|s| s.instance_id == instance_id);
        if !matches {
            return None;
        }
        let session = guard.take()?;
        session.stop.store(true, Ordering::Release);
        #[cfg(windows)]
        win32::dismiss_veil(&session);
        let was_attached = session.attached.swap(false, Ordering::AcqRel);
        let ever_attached = session.ever_attached.load(Ordering::Acquire);
        let strip_active = session.strip_active.swap(false, Ordering::AcqRel);
        let prev_host_clipchildren = session.prev_host_clipchildren.load(Ordering::Acquire);
        let attach_win_rect = *session
            .attach_win_rect
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let close_snapshot = session
            .close_snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        Some(FinishedEmbed {
            was_attached,
            ever_attached,
            strip_active,
            attach_win_rect,
            prev_host_clipchildren,
            close_snapshot,
        })
    }
}

impl Default for EmbedState {
    fn default() -> Self {
        Self::new()
    }
}

/// Native window identifier (`HWND` on Windows) if embedding is possible.
pub fn host_window_id(window: &tauri::WebviewWindow) -> Option<u64> {
    let handle = window.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as usize as u64),
        _ => None,
    }
}

/// True when the current main window exposes a native handle supported for game embedding.
pub fn can_embed(app: &AppHandle) -> bool {
    app.get_webview_window("main")
        .and_then(|w| host_window_id(&w))
        .is_some()
}

pub fn is_active(app: &AppHandle) -> bool {
    app.try_state::<EmbedState>().is_some_and(|s| s.is_active())
}

#[tauri::command]
pub fn get_game_embed_status(state: State<'_, EmbedState>) -> GameEmbedStatus {
    state.status()
}

#[tauri::command]
pub fn set_game_stage_bounds(
    state: State<'_, EmbedState>,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    radius: u32,
) {
    state.set_bounds(StageBounds::clamped(x, y, width, height, radius));
}

thread_local! {
    static EXTRACTED_CONTROLLER: std::cell::RefCell<Option<win32::ICoreWebView2Controller>> =
        const { std::cell::RefCell::new(None) };
}

/// Clones `ICoreWebView2Controller` on the main UI STA thread and returns from
/// `WebviewWindow::with_webview` immediately so `tauri-runtime-wry` releases its
/// `window_id` mutex before `f` performs Win32/Tauri operations that may synchronously
/// dispatch `WindowEvent` callbacks (`Resized`, `Focused`) or nested webview calls.
fn with_controller_on_main_thread(
    app: &AppHandle,
    f: impl FnOnce(win32::ICoreWebView2Controller) + Send + 'static,
) -> bool {
    let Some(w) = app.get_webview_window("main") else {
        return false;
    };
    app.run_on_main_thread(move || {
        let _ = w.with_webview(|webview| {
            let ctrl = webview.controller().clone();
            EXTRACTED_CONTROLLER.with(|slot| {
                *slot.borrow_mut() = Some(ctrl);
            });
        });
        let controller = EXTRACTED_CONTROLLER.with(|slot| slot.borrow_mut().take());
        if let Some(controller) = controller {
            f(controller);
        }
    })
    .is_ok()
}

#[tauri::command]
pub fn toggle_game_maximize(app: AppHandle, state: State<'_, EmbedState>) -> GameEmbedStatus {
    let Some(session) = state.current_session() else {
        return GameEmbedStatus::default();
    };
    if !session.attached.load(Ordering::Acquire) {
        return GameEmbedStatus::default();
    }
    let next = !session.maximized.load(Ordering::Acquire);
    session.maximized.store(next, Ordering::Release);

    if let Some(w) = app.get_webview_window("main") {
        // Expand to the monitor work area instead of Win32 SW_MAXIMIZE so DWM never paints
        // duplicate native caption buttons on top of Clade's custom header strip.
        let _ = w.set_resizable(!next);
        if let Some(host_id) = host_window_id(&w) {
            let session_clone = session.clone();
            let _ = with_controller_on_main_thread(&app, move |controller| {
                let host_hwnd = host_id as usize as win32::HWND;
                win32::set_work_area_expanded(host_hwnd, next, &session_clone);
                if session_clone.strip_active.load(Ordering::Acquire) {
                    if let Some(layout) = win32::layout_for_host(host_hwnd) {
                        let wry_hwnd = win32::controller_wry_hwnd(&controller);
                        win32::apply_webview_bounds(
                            &controller,
                            wry_hwnd,
                            layout.webview_width,
                            layout.webview_height,
                        );
                        win32::sync_stage_bounds_for_layout(
                            host_hwnd,
                            &session_clone,
                            layout.stage,
                            false,
                        );
                        let mc_id = session_clone.game_win.load(Ordering::Acquire);
                        if mc_id != 0 && win32::is_host_foreground(host_hwnd) {
                            win32::focus_window(mc_id as usize as win32::HWND);
                        }
                    }
                }
            });
        }
    }
    let status = session.status();
    let _ = app.emit(EMBED_EVENT, &status);
    status
}

/// Called when the host window receives focus while a framed game is running.
pub fn on_host_focused(app: &AppHandle) {
    if let Some(session) = app
        .try_state::<EmbedState>()
        .and_then(|s| s.current_session())
    {
        if session.attached.load(Ordering::Acquire) {
            session.focus_requested.store(true, Ordering::Release);
        }
    }
}

/// Ensures WebView2 strip and embedded stage stay synchronized when host resizes during a game.
pub fn on_host_resized(app: &AppHandle) {
    let Some(state) = app.try_state::<EmbedState>() else {
        return;
    };
    let Some(session) = state.current_session() else {
        return;
    };
    if !session.strip_active.load(Ordering::Acquire) {
        return;
    }
    let Some(w) = app.get_webview_window("main") else {
        return;
    };
    let Some(host_id) = host_window_id(&w) else {
        return;
    };
    let _ = with_controller_on_main_thread(app, move |controller| {
        if !session.strip_active.load(Ordering::Acquire) {
            return;
        }
        let host_hwnd = host_id as usize as win32::HWND;
        if let Some(layout) = win32::layout_for_host(host_hwnd) {
            let wry_hwnd = win32::controller_wry_hwnd(&controller);
            win32::sync_webview_bounds_if_needed(
                &controller,
                wry_hwnd,
                layout.webview_width,
                layout.webview_height,
            );
            win32::sync_stage_bounds_for_layout(host_hwnd, &session, layout.stage, true);
        }
    });
}

fn should_animate_embed_app(app: &AppHandle) -> bool {
    let mode = app
        .try_state::<crate::ui_settings::SettingsState>()
        .map(|s| s.reduced_motion())
        .unwrap_or_else(|| "system".to_owned());
    should_animate_embed(&mode, win32::os_animations_enabled())
}

/// Sends a graceful close signal to the embedded game window on the first close click.
/// Returns `true` if `CloseRequested` should be prevented so the game can save & close cleanly
/// inside the frame; a second close click detaches the game to a standalone window and falls
/// through to Clade's background-exit handling.
pub fn request_close_or_detach(app: &AppHandle) -> bool {
    let Some(embed) = app.try_state::<EmbedState>() else {
        return false;
    };
    let Some(session) = embed.current_session() else {
        return false;
    };
    let mc_id = session.game_win.load(Ordering::Acquire);
    if !session.attached.load(Ordering::Acquire) || mc_id == 0 {
        return false;
    }
    if session.close_attempts.fetch_add(1, Ordering::AcqRel) == 0 {
        if should_animate_embed_app(app) {
            if let Some(host_hwnd) = app
                .get_webview_window("main")
                .and_then(|w| host_window_id(&w))
                .map(|id| id as usize as win32::HWND)
            {
                let snap = win32::capture_stage_snapshot(host_hwnd, mc_id as usize as win32::HWND);
                *session
                    .close_snapshot
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = snap;
            }
        }
        session.close_requested.store(true, Ordering::Release);
        let _ = app.emit(EMBED_EVENT, session.status());
        return true;
    }
    session.stop.store(true, Ordering::Release);
    win32::dismiss_veil(&session);
    *session
        .close_snapshot
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    win32::detach_to_standalone(mc_id as usize as win32::HWND);
    if session.mark_window_detached() {
        finalize_window_detach(app.clone(), session.clone(), false);
    }
    let _ = embed.finish(&session.instance_id);
    false
}

/// Starts monitoring a newly launched game process (`pid`) to embed its window into Clade's stage.
pub fn start_session(app: AppHandle, instance_id: String, pid: u32) {
    if pid == 0 {
        return;
    }
    let Some(host_win) = app.get_webview_window("main") else {
        return;
    };
    let Some(host_id) = host_window_id(&host_win) else {
        tracing::info!(
            "framed game mode unavailable on this display handle; keeping standalone window"
        );
        return;
    };
    tracing::info!(instance = %instance_id, pid, "watching for game window to embed");
    let state = app.state::<EmbedState>();
    let session = state.begin(instance_id, pid);
    let _ = app.emit(EMBED_EVENT, session.status());

    let thread_name = "clade-game-embed".to_owned();
    let _ = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            run_watcher_win32(app, session, host_id);
        });
}

/// Tears down framed mode when the game finishes, restoring full WebView2 bounds and launcher UI.
pub fn end_session(app: &AppHandle, instance_id: &str) {
    let Some(state) = app.try_state::<EmbedState>() else {
        return;
    };
    let close_veil_slot = state.close_veil.clone();
    let Some(ended) = state.finish(instance_id) else {
        return;
    };
    if ended.was_attached || ended.ever_attached || ended.strip_active {
        let close_anim = (ended.was_attached && should_animate_embed_app(app))
            .then_some((ended.close_snapshot, close_veil_slot));
        restore_full_webview(
            app,
            ended.attach_win_rect,
            Some(ended.prev_host_clipchildren),
            close_anim,
            true,
            true,
        );
    } else {
        let _ = app.emit(EMBED_EVENT, GameEmbedStatus::default());
    }
}

fn try_attach_game_window(
    app: &AppHandle,
    session: &Arc<ActiveSession>,
    host_id: u64,
    mc_id: u64,
) -> bool {
    let Some(w) = app.get_webview_window("main") else {
        return false;
    };
    let animate = should_animate_embed_app(app);
    let close_veil_slot = app.try_state::<EmbedState>().map(|s| s.close_veil.clone());
    let (tx, rx) = std::sync::mpsc::sync_channel::<bool>(1);
    let app_clone = app.clone();
    let session_clone = session.clone();
    let w_clone = w.clone();

    if !with_controller_on_main_thread(app, move |controller| {
        if session_clone.stop.load(Ordering::Acquire) {
            let _ = tx.send(false);
            return;
        }
        let host_hwnd = host_id as usize as win32::HWND;
        let mc_hwnd = mc_id as usize as win32::HWND;
        if !win32::is_valid(host_hwnd) || !win32::is_valid(mc_hwnd) {
            let _ = tx.send(false);
            return;
        }
        let _ = w_clone.set_resizable(true);
        let Some(layout) = win32::layout_for_host(host_hwnd) else {
            let _ = tx.send(false);
            return;
        };
        let Some(stage_rect) = win32::stage_screen_rect(host_hwnd, layout.stage) else {
            let _ = tx.send(false);
            return;
        };

        if let Some(slot) = &close_veil_slot {
            win32::dismiss_veil_slot(slot);
        }
        let veil_handle = if animate {
            win32::create_open_veil(host_hwnd, session_clone.clone(), stage_rect)
        } else {
            None
        };
        let veil_hwnd = veil_handle.as_ref().map(|v| v.hwnd);

        {
            let mut rect_guard = session_clone
                .attach_win_rect
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if rect_guard.is_none() {
                *rect_guard = win32::window_rect(host_hwnd);
            }
        }
        if !session_clone.ever_attached.load(Ordering::Acquire) {
            session_clone
                .prev_host_clipchildren
                .store(win32::host_has_clip_children(host_hwnd), Ordering::Release);
        }
        win32::set_host_clip_children(host_hwnd, true);

        let wry_hwnd = win32::controller_wry_hwnd(&controller);
        win32::ensure_child_clipsiblings(wry_hwnd);

        session_clone.strip_active.store(true, Ordering::Release);
        // SAFETY: executing on the main UI STA thread owning `host_hwnd`, `wry_hwnd`, and `controller`.
        unsafe {
            win32::install_host_subclass(
                host_hwnd,
                win32::HostSubclassState {
                    controller: controller.clone(),
                    wry_hwnd,
                    session: session_clone.clone(),
                },
            );
            win32::install_wry_subclass(
                wry_hwnd,
                win32::WrySubclassState {
                    host_hwnd,
                    controller: controller.clone(),
                    session: session_clone.clone(),
                    sync_pending: std::sync::atomic::AtomicBool::new(false),
                },
            );
        }
        win32::apply_webview_bounds(
            &controller,
            wry_hwnd,
            layout.webview_width,
            layout.webview_height,
        );

        if !win32::attach_and_place(mc_hwnd, host_hwnd, layout.stage, veil_hwnd) {
            if let Some(handle) = &veil_handle {
                win32::cancel_open_veil(handle, &session_clone.veil_slot);
            }
            session_clone.strip_active.store(false, Ordering::Release);
            // SAFETY: still on main UI STA thread.
            unsafe {
                win32::uninstall_wry_subclass(wry_hwnd);
                win32::uninstall_host_subclass(host_hwnd);
            }
            if !session_clone.ever_attached.load(Ordering::Acquire)
                && !session_clone.prev_host_clipchildren.load(Ordering::Acquire)
            {
                win32::set_host_clip_children(host_hwnd, false);
            }
            win32::restore_webview_to_client(host_hwnd, &controller, wry_hwnd);
            let _ = tx.send(false);
            return;
        }

        session_clone.game_win.store(mc_id, Ordering::Release);
        session_clone.ever_attached.store(true, Ordering::Release);
        let newly_attached = !session_clone.attached.swap(true, Ordering::AcqRel);
        if let Some(handle) = &veil_handle {
            win32::activate_open_veil(handle, mc_hwnd);
        }
        if newly_attached {
            tracing::info!(
                instance = %session_clone.instance_id,
                "embedded game window into Clade stage"
            );
        }
        let _ = app_clone.emit(EMBED_EVENT, session_clone.status());
        let _ = tx.send(true);
    }) {
        return false;
    }

    rx.recv_timeout(ATTACH_TIMEOUT).unwrap_or(false)
}

fn restore_full_webview(
    app: &AppHandle,
    restore_rect: Option<(i32, i32, i32, i32)>,
    restore_clipchildren: Option<bool>,
    close_anim: Option<(Option<CapturedStage>, Arc<VeilSlot>)>,
    emit_inactive: bool,
    wait_sync: bool,
) {
    let Some(w) = app.get_webview_window("main") else {
        if emit_inactive {
            let _ = app.emit(EMBED_EVENT, GameEmbedStatus::default());
        }
        return;
    };
    let Some(host_id) = host_window_id(&w) else {
        if emit_inactive {
            let _ = app.emit(EMBED_EVENT, GameEmbedStatus::default());
        }
        return;
    };
    let (done_tx, done_rx) = if wait_sync {
        let (tx, rx) = std::sync::mpsc::sync_channel::<()>(1);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    let app_clone = app.clone();
    let w_clone = w.clone();
    let scheduled = with_controller_on_main_thread(app, move |controller| {
        let host_hwnd = host_id as usize as win32::HWND;
        let visible = win32::is_valid(host_hwnd);
        let wry_hwnd = win32::controller_wry_hwnd(&controller);
        // SAFETY: executing on the main UI STA thread owning `host_hwnd`, `wry_hwnd`, and `webview`.
        unsafe {
            win32::uninstall_wry_subclass(wry_hwnd);
            win32::uninstall_host_subclass(host_hwnd);
        }
        if restore_clipchildren == Some(false) {
            win32::set_host_clip_children(host_hwnd, false);
        }
        win32::set_corner_preference(host_hwnd, false);
        let _ = w_clone.set_resizable(true);
        if let Some((x, y, width, height)) = restore_rect {
            if win32::window_rect(host_hwnd) != Some((x, y, width, height)) {
                win32::set_window_rect(host_hwnd, x, y, width, height);
            }
        }

        if visible {
            if let Some((snapshot, close_slot)) = close_anim {
                if let Some(stage_rect) = win32::layout_for_host(host_hwnd)
                    .and_then(|l| win32::stage_screen_rect(host_hwnd, l.stage))
                {
                    let _ = win32::create_close_veil(host_hwnd, close_slot, stage_rect, snapshot);
                }
            }
        }

        win32::restore_webview_to_client_with_visibility(host_hwnd, &controller, wry_hwnd, visible);

        if visible {
            win32::focus_webview_if_foreground(host_hwnd, &controller);
            win32::redraw_all(host_hwnd);
            crate::webview::resume_window(&app_clone);
            crate::glass::refresh(&app_clone);
        }
        if emit_inactive {
            let _ = app_clone.emit(EMBED_EVENT, GameEmbedStatus::default());
        }
        if let Some(tx) = done_tx {
            let _ = tx.send(());
        }
    });
    if scheduled {
        if let Some(rx) = done_rx {
            let _ = rx.recv_timeout(Duration::from_millis(250));
        }
    } else if emit_inactive {
        let _ = app.emit(EMBED_EVENT, GameEmbedStatus::default());
    }
}

fn finalize_window_detach(app: AppHandle, session: Arc<ActiveSession>, wait_sync: bool) {
    let attach_rect = *session
        .attach_win_rect
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *session
        .pre_expand_win_rect
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    let prev_clip = session.prev_host_clipchildren.load(Ordering::Acquire);
    let snapshot = session
        .close_snapshot
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    let close_anim = if wait_sync && should_animate_embed_app(&app) {
        app.try_state::<EmbedState>()
            .map(|s| (snapshot, s.close_veil.clone()))
    } else {
        None
    };
    restore_full_webview(
        &app,
        attach_rect,
        Some(prev_clip),
        close_anim,
        true,
        wait_sync,
    );
}

fn run_watcher_win32(app: AppHandle, session: Arc<ActiveSession>, host_id: u64) {
    let host_hwnd = host_id as usize as win32::HWND;
    let mut delayed_focus_at: Option<std::time::Instant> = None;

    while !session.stop.load(Ordering::Acquire) {
        let mut mc_id = session.game_win.load(Ordering::Acquire);
        if mc_id != 0 && !win32::is_attached_window_alive(mc_id as usize as win32::HWND, host_hwnd)
        {
            mc_id = 0;
            delayed_focus_at = None;
            if session.mark_window_detached() {
                finalize_window_detach(app.clone(), session.clone(), true);
            }
        }

        if mc_id == 0 {
            if let Some(found) = win32::find_game_window(session.pid, host_hwnd) {
                let found_id = found as usize as u64;
                if try_attach_game_window(&app, &session, host_id, found_id) {
                    delayed_focus_at = Some(std::time::Instant::now() + Duration::from_millis(60));
                }
            }
            std::thread::sleep(POLL_IDLE);
            continue;
        }

        let mc_hwnd = mc_id as usize as win32::HWND;
        if let Some(layout) = win32::layout_for_host(host_hwnd) {
            win32::sync_stage_bounds_for_layout(host_hwnd, &session, layout.stage, true);
        }
        let veil_active = session.veil_slot.hwnd() != 0;
        if veil_active {
            if session.focus_requested.swap(false, Ordering::AcqRel) {
                delayed_focus_at = Some(std::time::Instant::now() + Duration::from_millis(60));
            }
        } else if session.focus_requested.swap(false, Ordering::AcqRel) {
            delayed_focus_at = Some(std::time::Instant::now() + Duration::from_millis(60));
            if win32::should_hand_focus_to_game(host_hwnd) {
                win32::focus_window(mc_hwnd);
            }
        } else if delayed_focus_at.is_some_and(|t| std::time::Instant::now() >= t) {
            delayed_focus_at = None;
            if win32::should_hand_focus_to_game(host_hwnd) {
                win32::focus_window(mc_hwnd);
            }
        }
        if session.close_requested.swap(false, Ordering::AcqRel) {
            win32::request_close(mc_hwnd);
        }
        std::thread::sleep(POLL_ACTIVE);
    }
}

// ===================================================================================== Windows FFI

mod win32 {
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Once};
    use std::time::Duration;

    pub use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller;
    use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC;
    use windows_sys::core::BOOL;
    pub use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
    use windows_sys::Win32::Graphics::Dwm::{
        DwmFlush, DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DEFAULT,
        DWMWCP_DONOTROUND,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, ClientToScreen, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject,
        GetDC, GetMonitorInfoW, MonitorFromWindow, RedrawWindow, ReleaseDC, ScreenToClient,
        SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, MONITORINFO,
        MONITOR_DEFAULTTONEAREST, RDW_ALLCHILDREN, RDW_INVALIDATE, RDW_UPDATENOW, SRCCOPY,
    };
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::Shell::{
        DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows,
        GetClientRect, GetCursorPos, GetForegroundWindow, GetParent, GetWindow, GetWindowLongPtrW,
        GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, LoadCursorW,
        PeekMessageW, PostMessageW, RegisterClassW, SetForegroundWindow, SetWindowLongPtrW,
        SetWindowPos, SystemParametersInfoW, UpdateLayeredWindow, GWLP_HINSTANCE, GWLP_HWNDPARENT,
        GWL_EXSTYLE, GWL_STYLE, GW_HWNDNEXT, GW_HWNDPREV, GW_OWNER, IDC_ARROW, MSG, PM_REMOVE,
        SIZE_MINIMIZED, SPI_GETCLIENTAREAANIMATION, SWP_ASYNCWINDOWPOS, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, ULW_ALPHA, WINDOWPOS,
        WM_APP, WM_CLOSE, WM_DPICHANGED, WM_EXITSIZEMOVE, WM_NCDESTROY, WM_SIZE,
        WM_WINDOWPOSCHANGED, WM_WINDOWPOSCHANGING, WNDCLASSW, WS_CAPTION, WS_CHILD,
        WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_APPWINDOW, WS_EX_CLIENTEDGE, WS_EX_DLGMODALFRAME,
        WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_STATICEDGE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        WS_EX_TRANSPARENT, WS_EX_WINDOWEDGE, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_OVERLAPPEDWINDOW,
        WS_POPUP, WS_SYSMENU, WS_THICKFRAME, WS_VISIBLE,
    };

    use super::{
        compute_close_frame, compute_framed_layout, compute_reveal_frame, paint_close_bgra,
        paint_reveal_bgra, stage_to_screen_rect, ActiveSession, CapturedStage, FramedLayout,
        StageBounds, VeilSlot, CLOSE_DURATION_MS, CLOSE_HOLD_MS, REVEAL_DURATION_MS,
        REVEAL_HOLD_MS,
    };

    const HWND_TOP: HWND = std::ptr::null_mut();
    const EMBED_SUBCLASS_ID: usize = 0x434C_4144_4501;
    const WRY_SUBCLASS_ID: usize = 0x434C_4144_4502;
    const WM_CLADE_SYNC_STRIP: u32 = WM_APP + 0x41;

    pub(super) struct HostSubclassState {
        pub(super) controller: ICoreWebView2Controller,
        pub(super) wry_hwnd: HWND,
        pub(super) session: Arc<ActiveSession>,
    }

    pub(super) struct WrySubclassState {
        pub(super) host_hwnd: HWND,
        pub(super) controller: ICoreWebView2Controller,
        pub(super) session: Arc<ActiveSession>,
        pub(super) sync_pending: AtomicBool,
    }

    pub unsafe fn install_host_subclass(host: HWND, state: HostSubclassState) {
        uninstall_host_subclass(host);
        let raw = Rc::into_raw(Rc::new(state));
        if SetWindowSubclass(
            host,
            Some(embed_host_subclass_proc),
            EMBED_SUBCLASS_ID,
            raw as usize,
        ) == 0
        {
            drop(Rc::from_raw(raw));
        }
    }

    pub unsafe fn uninstall_host_subclass(host: HWND) {
        let mut data: usize = 0;
        if GetWindowSubclass(
            host,
            Some(embed_host_subclass_proc),
            EMBED_SUBCLASS_ID,
            &mut data,
        ) != 0
            && RemoveWindowSubclass(host, Some(embed_host_subclass_proc), EMBED_SUBCLASS_ID) != 0
            && data != 0
        {
            drop(Rc::from_raw(data as *const HostSubclassState));
        }
    }

    unsafe fn clone_host_subclass_state(
        hwnd: HWND,
        expected: usize,
    ) -> Option<Rc<HostSubclassState>> {
        if expected == 0 || IsWindow(hwnd) == 0 {
            return None;
        }
        let mut current: usize = 0;
        if GetWindowSubclass(
            hwnd,
            Some(embed_host_subclass_proc),
            EMBED_SUBCLASS_ID,
            &mut current,
        ) == 0
            || current != expected
        {
            return None;
        }
        let ptr = expected as *const HostSubclassState;
        Rc::increment_strong_count(ptr);
        Some(Rc::from_raw(ptr))
    }

    unsafe extern "system" fn embed_host_subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        uid_subclass: usize,
        dw_ref_data: usize,
    ) -> LRESULT {
        match msg {
            WM_NCDESTROY => {
                let res = DefSubclassProc(hwnd, msg, wparam, lparam);
                if RemoveWindowSubclass(hwnd, Some(embed_host_subclass_proc), uid_subclass) != 0
                    && dw_ref_data != 0
                {
                    drop(Rc::from_raw(dw_ref_data as *const HostSubclassState));
                }
                return res;
            }
            WM_SIZE if wparam != SIZE_MINIMIZED as usize => {
                let res = DefSubclassProc(hwnd, msg, wparam, lparam);
                if let Some(state) = clone_host_subclass_state(hwnd, dw_ref_data) {
                    sync_strip_layout_on_host(hwnd, &state);
                }
                return res;
            }
            WM_DPICHANGED => {
                let res = DefSubclassProc(hwnd, msg, wparam, lparam);
                if let Some(state) = clone_host_subclass_state(hwnd, dw_ref_data) {
                    sync_strip_layout_on_host(hwnd, &state);
                }
                return res;
            }
            WM_WINDOWPOSCHANGED => {
                let res = DefSubclassProc(hwnd, msg, wparam, lparam);
                if IsWindow(hwnd) != 0 && IsIconic(hwnd) == 0 && IsWindowVisible(hwnd) != 0 {
                    if let Some(state) = clone_host_subclass_state(hwnd, dw_ref_data) {
                        sync_stage_position_on_host(hwnd, &state);
                    }
                }
                return res;
            }
            WM_EXITSIZEMOVE => {
                let res = DefSubclassProc(hwnd, msg, wparam, lparam);
                if let Some(state) = clone_host_subclass_state(hwnd, dw_ref_data) {
                    sync_stage_position_on_host(hwnd, &state);
                    if GetForegroundWindow() == hwnd && should_hand_focus_to_game(hwnd) {
                        let mc_id = state.session.game_win.load(Ordering::Acquire);
                        if mc_id != 0 {
                            focus_window(mc_id as usize as HWND);
                        }
                    }
                }
                return res;
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }

    unsafe fn sync_strip_layout_on_host(hwnd: HWND, state: &HostSubclassState) {
        if !state.session.strip_active.load(Ordering::Acquire) {
            return;
        }
        let Some(layout) = layout_for_host(hwnd) else {
            return;
        };
        apply_webview_bounds(
            &state.controller,
            state.wry_hwnd,
            layout.webview_width,
            layout.webview_height,
        );
        sync_stage_bounds_for_layout(hwnd, &state.session, layout.stage, false);
    }

    unsafe fn sync_stage_position_on_host(hwnd: HWND, state: &HostSubclassState) {
        if !state.session.strip_active.load(Ordering::Acquire) {
            return;
        }
        let Some(layout) = layout_for_host(hwnd) else {
            return;
        };
        sync_stage_bounds_for_layout(hwnd, &state.session, layout.stage, true);
    }

    pub(super) fn sync_stage_bounds_for_layout(
        host: HWND,
        session: &ActiveSession,
        stage: StageBounds,
        only_if_needed: bool,
    ) {
        let mc_id = session.game_win.load(Ordering::Acquire);
        if mc_id == 0 {
            return;
        }
        let mc_hwnd = mc_id as usize as HWND;
        let Some(target) = stage_screen_rect(host, stage) else {
            return;
        };
        let veil_id = session.veil_slot.hwnd();
        let below_veil = (veil_id != 0).then_some(veil_id as usize as HWND);
        if !only_if_needed || needs_bounds_sync(mc_hwnd, host, below_veil, target) {
            apply_screen_bounds(
                mc_hwnd,
                target.0,
                target.1,
                target.2,
                target.3,
                false,
                Some(host),
                below_veil,
            );
        }
    }

    pub unsafe fn install_wry_subclass(wry_hwnd: HWND, state: WrySubclassState) {
        if wry_hwnd.is_null() || IsWindow(wry_hwnd) == 0 {
            return;
        }
        uninstall_wry_subclass(wry_hwnd);
        let raw = Rc::into_raw(Rc::new(state));
        if SetWindowSubclass(
            wry_hwnd,
            Some(wry_strip_subclass_proc),
            WRY_SUBCLASS_ID,
            raw as usize,
        ) == 0
        {
            drop(Rc::from_raw(raw));
        }
    }

    pub unsafe fn uninstall_wry_subclass(wry_hwnd: HWND) {
        if wry_hwnd.is_null() || IsWindow(wry_hwnd) == 0 {
            return;
        }
        let mut data: usize = 0;
        if GetWindowSubclass(
            wry_hwnd,
            Some(wry_strip_subclass_proc),
            WRY_SUBCLASS_ID,
            &mut data,
        ) != 0
            && RemoveWindowSubclass(wry_hwnd, Some(wry_strip_subclass_proc), WRY_SUBCLASS_ID) != 0
            && data != 0
        {
            drop(Rc::from_raw(data as *const WrySubclassState));
        }
    }

    unsafe fn clone_wry_subclass_state(
        hwnd: HWND,
        expected: usize,
    ) -> Option<Rc<WrySubclassState>> {
        if expected == 0 || IsWindow(hwnd) == 0 {
            return None;
        }
        let mut current: usize = 0;
        if GetWindowSubclass(
            hwnd,
            Some(wry_strip_subclass_proc),
            WRY_SUBCLASS_ID,
            &mut current,
        ) == 0
            || current != expected
        {
            return None;
        }
        let ptr = expected as *const WrySubclassState;
        Rc::increment_strong_count(ptr);
        Some(Rc::from_raw(ptr))
    }

    unsafe fn clamp_controller_bounds_if_needed(
        controller: &ICoreWebView2Controller,
        width: i32,
        height: i32,
    ) {
        let w = width.max(1);
        let h = height.max(1);
        if !controller_matches_bounds(controller, w, h) {
            let _ = controller.SetBounds(windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            });
            let _ = controller.NotifyParentWindowPositionChanged();
        }
    }

    unsafe extern "system" fn wry_strip_subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        uid_subclass: usize,
        dw_ref_data: usize,
    ) -> LRESULT {
        match msg {
            WM_NCDESTROY => {
                let res = DefSubclassProc(hwnd, msg, wparam, lparam);
                if RemoveWindowSubclass(hwnd, Some(wry_strip_subclass_proc), uid_subclass) != 0
                    && dw_ref_data != 0
                {
                    drop(Rc::from_raw(dw_ref_data as *const WrySubclassState));
                }
                return res;
            }
            WM_WINDOWPOSCHANGING if lparam != 0 => {
                if let Some(state) = clone_wry_subclass_state(hwnd, dw_ref_data) {
                    if state.session.strip_active.load(Ordering::Acquire) {
                        if let Some(layout) = layout_for_host(state.host_hwnd) {
                            let wp = &mut *(lparam as *mut WINDOWPOS);
                            if (wp.flags & SWP_NOMOVE) == 0 {
                                wp.x = 0;
                                wp.y = 0;
                            }
                            if (wp.flags & SWP_NOSIZE) == 0 {
                                wp.cx = layout.webview_width.max(1);
                                wp.cy = layout.webview_height.max(1);
                            }
                            if state
                                .sync_pending
                                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                                .is_ok()
                            {
                                let _ = PostMessageW(hwnd, WM_CLADE_SYNC_STRIP, 0, 0);
                            }
                        }
                    }
                }
            }
            WM_WINDOWPOSCHANGED => {
                let res = DefSubclassProc(hwnd, msg, wparam, lparam);
                if let Some(state) = clone_wry_subclass_state(hwnd, dw_ref_data) {
                    if state.session.strip_active.load(Ordering::Acquire) {
                        if let Some(layout) = layout_for_host(state.host_hwnd) {
                            clamp_controller_bounds_if_needed(
                                &state.controller,
                                layout.webview_width,
                                layout.webview_height,
                            );
                        }
                    }
                }
                return res;
            }
            WM_CLADE_SYNC_STRIP => {
                if let Some(state) = clone_wry_subclass_state(hwnd, dw_ref_data) {
                    state.sync_pending.store(false, Ordering::Release);
                    if IsWindow(hwnd) != 0 && state.session.strip_active.load(Ordering::Acquire) {
                        if let Some(layout) = layout_for_host(state.host_hwnd) {
                            clamp_controller_bounds_if_needed(
                                &state.controller,
                                layout.webview_width,
                                layout.webview_height,
                            );
                        }
                    }
                }
                return 0;
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }

    struct Search {
        pid: u32,
        host: HWND,
        found: Option<HWND>,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam as *mut Search);
        if hwnd == search.host || IsWindowVisible(hwnd) == 0 {
            return 1;
        }
        let mut w_pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut w_pid);
        if w_pid != search.pid {
            return 1;
        }
        let owner = GetWindow(hwnd, GW_OWNER);
        let parent = GetParent(hwnd);
        if !owner.is_null() && owner != search.host && parent != search.host {
            return 1;
        }
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetClientRect(hwnd, &mut rect) == 0 {
            return 1;
        }
        if (rect.right - rect.left) >= 160 && (rect.bottom - rect.top) >= 120 {
            search.found = Some(hwnd);
            return 0;
        }
        1
    }

    pub fn find_game_window(pid: u32, host: HWND) -> Option<HWND> {
        let mut search = Search {
            pid,
            host,
            found: None,
        };
        // SAFETY: `search` outlives synchronous `EnumWindows` invocation.
        unsafe {
            EnumWindows(Some(enum_proc), std::ptr::addr_of_mut!(search) as LPARAM);
        }
        search.found
    }

    pub fn is_host_alive(hwnd: HWND) -> bool {
        // SAFETY: Win32 handles invalid pointers safely by returning 0.
        unsafe { !hwnd.is_null() && IsWindow(hwnd) != 0 }
    }

    pub fn is_valid(hwnd: HWND) -> bool {
        // SAFETY: Win32 handles non-window pointers safely by returning 0.
        unsafe { !hwnd.is_null() && IsWindow(hwnd) != 0 && IsWindowVisible(hwnd) != 0 }
    }

    pub fn layout_for_host(host: HWND) -> Option<FramedLayout> {
        let mut cr = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: reads live client area dimensions and monitor DPI on `host`.
        let dpi = unsafe {
            if !is_host_alive(host) || GetClientRect(host, &mut cr) == 0 {
                return None;
            }
            let raw = GetDpiForWindow(host);
            if raw == 0 {
                96
            } else {
                raw
            }
        };
        compute_framed_layout(cr.right - cr.left, cr.bottom - cr.top, dpi)
    }

    pub fn controller_wry_hwnd(controller: &ICoreWebView2Controller) -> HWND {
        let mut raw = windows::Win32::Foundation::HWND::default();
        // SAFETY: `ParentWindow` writes WebView2's host child HWND (`WRY_WEBVIEW`).
        if unsafe { controller.ParentWindow(&mut raw) }.is_ok() {
            raw.0.cast()
        } else {
            std::ptr::null_mut()
        }
    }

    pub fn apply_webview_bounds(
        controller: &ICoreWebView2Controller,
        wry_hwnd: HWND,
        width: i32,
        height: i32,
    ) {
        let w = width.max(1);
        let h = height.max(1);
        // SAFETY: invoked on the main UI STA thread owning `controller` and `wry_hwnd`.
        unsafe {
            let _ = controller.SetBounds(windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            });
            if !wry_hwnd.is_null() && IsWindow(wry_hwnd) != 0 {
                SetWindowPos(
                    wry_hwnd,
                    HWND_TOP,
                    0,
                    0,
                    w,
                    h,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
            let _ = controller.SetIsVisible(true);
            let _ = controller.NotifyParentWindowPositionChanged();
        }
    }

    fn controller_matches_bounds(
        controller: &ICoreWebView2Controller,
        width: i32,
        height: i32,
    ) -> bool {
        let mut rect = windows::Win32::Foundation::RECT::default();
        // SAFETY: invoked on the main UI STA thread owning `controller`.
        unsafe { controller.Bounds(&mut rect) }.is_ok()
            && rect.left == 0
            && rect.top == 0
            && rect.right == width.max(1)
            && rect.bottom == height.max(1)
    }

    pub fn sync_webview_bounds_if_needed(
        controller: &ICoreWebView2Controller,
        wry_hwnd: HWND,
        width: i32,
        height: i32,
    ) {
        let ctrl_matches = controller_matches_bounds(controller, width, height);
        let mut wr = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let hwnd_matches = !wry_hwnd.is_null()
            && unsafe { GetClientRect(wry_hwnd, &mut wr) } != 0
            && (wr.right - wr.left) == width.max(1)
            && (wr.bottom - wr.top) == height.max(1);
        if !ctrl_matches || !hwnd_matches {
            apply_webview_bounds(controller, wry_hwnd, width, height);
        }
    }

    pub fn restore_webview_to_client(
        host: HWND,
        controller: &ICoreWebView2Controller,
        wry_hwnd: HWND,
    ) {
        restore_webview_to_client_with_visibility(host, controller, wry_hwnd, true);
    }

    pub fn restore_webview_to_client_with_visibility(
        host: HWND,
        controller: &ICoreWebView2Controller,
        wry_hwnd: HWND,
        visible: bool,
    ) {
        let mut cr = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: invoked on the main UI STA thread owning `host` and `controller`.
        unsafe {
            if !is_host_alive(host) || GetClientRect(host, &mut cr) == 0 {
                return;
            }
            let w = (cr.right - cr.left).max(1);
            let h = (cr.bottom - cr.top).max(1);
            let _ = controller.SetBounds(windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            });
            if !wry_hwnd.is_null() && IsWindow(wry_hwnd) != 0 {
                SetWindowPos(
                    wry_hwnd,
                    HWND_TOP,
                    0,
                    0,
                    w,
                    h,
                    SWP_NOACTIVATE | if visible { SWP_SHOWWINDOW } else { 0 },
                );
            }
            if visible {
                let _ = controller.SetIsVisible(true);
            }
            let _ = controller.NotifyParentWindowPositionChanged();
        }
    }

    pub fn focus_webview_if_foreground(host: HWND, controller: &ICoreWebView2Controller) {
        // SAFETY: invoked on the main UI STA thread; raises WebView2 focus only if host is active.
        unsafe {
            if is_host_alive(host) && GetForegroundWindow() == host {
                let _ = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
            }
        }
    }

    pub fn redraw_all(host: HWND) {
        // SAFETY: invalidates and repaints `host` and child surfaces on the UI thread.
        unsafe {
            if is_host_alive(host) {
                RedrawWindow(
                    host,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN,
                );
            }
        }
    }

    pub fn host_has_clip_children(host: HWND) -> bool {
        // SAFETY: reads style flags on live `host`.
        unsafe {
            is_host_alive(host)
                && ((GetWindowLongPtrW(host, GWL_STYLE) as u32) & WS_CLIPCHILDREN) != 0
        }
    }

    pub fn set_host_clip_children(host: HWND, enable: bool) {
        // SAFETY: toggles `WS_CLIPCHILDREN` on live `host` owned by the UI thread.
        unsafe {
            if !is_host_alive(host) {
                return;
            }
            let style = GetWindowLongPtrW(host, GWL_STYLE) as u32;
            let next = if enable {
                style | WS_CLIPCHILDREN
            } else {
                style & !WS_CLIPCHILDREN
            };
            if next != style {
                SetWindowLongPtrW(host, GWL_STYLE, next as i32 as isize);
            }
        }
    }

    pub fn ensure_child_clipsiblings(child: HWND) {
        // SAFETY: ensures `WRY_WEBVIEW` has `WS_CLIPSIBLINGS` set.
        unsafe {
            if child.is_null() || IsWindow(child) == 0 {
                return;
            }
            let style = GetWindowLongPtrW(child, GWL_STYLE) as u32;
            if (style & WS_CLIPSIBLINGS) == 0 {
                SetWindowLongPtrW(child, GWL_STYLE, (style | WS_CLIPSIBLINGS) as i32 as isize);
            }
        }
    }

    pub fn stage_screen_rect(host: HWND, stage: StageBounds) -> Option<(i32, i32, i32, i32)> {
        // SAFETY: reads host visibility/iconic state and client origin on screen.
        unsafe {
            if !is_host_alive(host) || IsIconic(host) != 0 || IsWindowVisible(host) == 0 {
                return None;
            }
            let mut origin = POINT { x: 0, y: 0 };
            if ClientToScreen(host, &mut origin) == 0 {
                return None;
            }
            Some(stage_to_screen_rect(origin.x, origin.y, stage))
        }
    }

    pub fn attach_and_place(
        mc: HWND,
        host: HWND,
        b: StageBounds,
        below_veil: Option<HWND>,
    ) -> bool {
        let Some((sx, sy, sw, sh)) = stage_screen_rect(host, b) else {
            return false;
        };
        // SAFETY: valid live HWNDs belonging to the game window and launcher host.
        unsafe {
            // Assign ownership before stripping the native frame so a refused owner assignment never
            // leaves the standalone Minecraft window borderless and off the taskbar.
            SetWindowLongPtrW(mc, GWLP_HWNDPARENT, host as isize);
            if GetWindow(mc, GW_OWNER) != host {
                return false;
            }

            let style = GetWindowLongPtrW(mc, GWL_STYLE) as u32;
            let strip = WS_CAPTION
                | WS_THICKFRAME
                | WS_MINIMIZEBOX
                | WS_MAXIMIZEBOX
                | WS_SYSMENU
                | WS_CHILD;
            let add = WS_POPUP | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN;
            SetWindowLongPtrW(mc, GWL_STYLE, ((style & !strip) | add) as i32 as isize);

            let ex = GetWindowLongPtrW(mc, GWL_EXSTYLE) as u32;
            let strip_ex = WS_EX_APPWINDOW
                | WS_EX_DLGMODALFRAME
                | WS_EX_WINDOWEDGE
                | WS_EX_CLIENTEDGE
                | WS_EX_STATICEDGE;
            SetWindowLongPtrW(mc, GWL_EXSTYLE, (ex & !strip_ex) as i32 as isize);

            // Clear any stale region once without re-applying per resize so WGL flip swapchains stay intact.
            windows_sys::Win32::Graphics::Gdi::SetWindowRgn(mc, std::ptr::null_mut(), 0);
            set_corner_preference(mc, true);
            apply_screen_bounds(mc, sx, sy, sw, sh, true, Some(host), below_veil);
            if below_veil.is_none() && GetForegroundWindow() == host {
                let _ = SetForegroundWindow(mc);
            }
            true
        }
    }

    pub fn is_window_above(upper: HWND, lower: HWND) -> bool {
        if upper.is_null() || lower.is_null() || upper == lower {
            return false;
        }
        // SAFETY: walks global Z-order chain starting at `upper` with a bounded iteration cap.
        unsafe {
            let mut cur = upper;
            for _ in 0..64 {
                cur = GetWindow(cur, GW_HWNDNEXT);
                if cur.is_null() {
                    return false;
                }
                if cur == lower {
                    return true;
                }
            }
        }
        false
    }

    fn insert_after_above_host(mc: HWND, host: HWND) -> HWND {
        // SAFETY: finds the non-topmost sibling immediately above `host` in Z-order.
        unsafe {
            let mut prev = GetWindow(host, GW_HWNDPREV);
            for _ in 0..16 {
                if prev.is_null()
                    || ((GetWindowLongPtrW(prev, GWL_EXSTYLE) as u32) & WS_EX_TOPMOST) != 0
                {
                    return HWND_TOP;
                }
                if prev != mc {
                    return prev;
                }
                prev = GetWindow(prev, GW_HWNDPREV);
            }
        }
        HWND_TOP
    }

    #[allow(clippy::too_many_arguments)]
    pub fn apply_screen_bounds(
        mc: HWND,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        frame_changed: bool,
        above_host: Option<HWND>,
        below_veil: Option<HWND>,
    ) {
        let valid_veil = below_veil.filter(|&v| v != mc && is_valid(v));
        let needs_z_order = match (valid_veil, above_host) {
            (Some(veil), Some(host)) => !is_window_above(veil, mc) || !is_window_above(mc, host),
            (Some(veil), None) => !is_window_above(veil, mc),
            (None, Some(host)) => !is_window_above(mc, host),
            (None, None) => false,
        };
        let insert_after = match (valid_veil, above_host) {
            (Some(veil), _) if needs_z_order => veil,
            (None, Some(host)) if needs_z_order => insert_after_above_host(mc, host),
            _ => HWND_TOP,
        };
        let mut flags = SWP_NOACTIVATE
            | SWP_ASYNCWINDOWPOS
            | if frame_changed {
                SWP_FRAMECHANGED | SWP_SHOWWINDOW
            } else {
                0
            };
        if !needs_z_order {
            flags |= SWP_NOZORDER;
        }
        // SAFETY: `mc` is an owned borderless top-level window; async flags avoid blocking Clade's
        // UI thread on cross-process OpenGL frame swaps.
        unsafe {
            SetWindowPos(mc, insert_after, x, y, w.max(64), h.max(64), flags);
        }
    }

    pub fn detach_to_standalone(mc: HWND) {
        // SAFETY: restores independent top-level window styles and removes `host` ownership.
        unsafe {
            if mc.is_null() || IsWindow(mc) == 0 {
                return;
            }
            SetWindowLongPtrW(mc, GWLP_HWNDPARENT, 0);
            let style = GetWindowLongPtrW(mc, GWL_STYLE) as u32;
            let next_style = (style & !(WS_POPUP | WS_CHILD)) | WS_OVERLAPPEDWINDOW | WS_VISIBLE;
            SetWindowLongPtrW(mc, GWL_STYLE, next_style as i32 as isize);
            let ex = GetWindowLongPtrW(mc, GWL_EXSTYLE) as u32;
            SetWindowLongPtrW(mc, GWL_EXSTYLE, (ex | WS_EX_APPWINDOW) as i32 as isize);
            set_corner_preference(mc, false);
            SetWindowPos(
                mc,
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED | SWP_SHOWWINDOW | SWP_ASYNCWINDOWPOS,
            );
        }
    }

    pub fn is_attached_window_alive(mc: HWND, host: HWND) -> bool {
        // SAFETY: checks owned game HWND liveness while tolerating OS-automatic hide on host minimize.
        unsafe {
            if mc.is_null() || IsWindow(mc) == 0 {
                return false;
            }
            if !is_host_alive(host) || IsIconic(host) != 0 || IsWindowVisible(host) == 0 {
                return true;
            }
            IsWindowVisible(mc) != 0
        }
    }

    pub fn window_rect(hwnd: HWND) -> Option<(i32, i32, i32, i32)> {
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: `hwnd` is a live window handle and `rect` is initialized.
        unsafe {
            if IsWindow(hwnd) == 0 || GetWindowRect(hwnd, &mut rect) == 0 {
                return None;
            }
        }
        Some((
            rect.left,
            rect.top,
            (rect.right - rect.left).max(1),
            (rect.bottom - rect.top).max(1),
        ))
    }

    pub fn set_window_rect(hwnd: HWND, x: i32, y: i32, width: i32, height: i32) {
        // SAFETY: moves/resizes live top-level host window without disturbing Z-order or activation.
        unsafe {
            if IsWindow(hwnd) != 0 {
                SetWindowPos(
                    hwnd,
                    HWND_TOP,
                    x,
                    y,
                    width.max(1),
                    height.max(1),
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
            }
        }
    }

    pub fn set_corner_preference(hwnd: HWND, square: bool) {
        let pref: i32 = if square {
            DWMWCP_DONOTROUND
        } else {
            DWMWCP_DEFAULT
        };
        // SAFETY: `pref` outlives synchronous DwmSetWindowAttribute invocation.
        unsafe {
            if IsWindow(hwnd) != 0 {
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                    std::ptr::from_ref(&pref).cast(),
                    std::mem::size_of::<i32>() as u32,
                );
            }
        }
    }

    pub fn set_work_area_expanded(host: HWND, expand: bool, session: &super::ActiveSession) {
        // SAFETY: reads monitor work area and updates top-level host position cleanly on UI thread.
        unsafe {
            if IsWindow(host) == 0 {
                return;
            }
            if expand {
                if let Some(cur) = window_rect(host) {
                    *session
                        .pre_expand_win_rect
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) = Some(cur);
                }
                let mon = MonitorFromWindow(host, MONITOR_DEFAULTTONEAREST);
                let mut mi = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    rcMonitor: RECT {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    },
                    rcWork: RECT {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    },
                    dwFlags: 0,
                };
                if GetMonitorInfoW(mon, &mut mi) == 0 {
                    return;
                }
                let work = mi.rcWork;
                let mut wr = RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };
                let mut cr = RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };
                let mut pt = POINT { x: 0, y: 0 };
                let (il, it, ir, ib) = if GetWindowRect(host, &mut wr) != 0
                    && GetClientRect(host, &mut cr) != 0
                    && ClientToScreen(host, &mut pt) != 0
                {
                    let il = (pt.x - wr.left).clamp(0, 32);
                    let it = (pt.y - wr.top).clamp(0, 32);
                    let ir = (wr.right - (pt.x + cr.right)).clamp(0, 32);
                    let ib = (wr.bottom - (pt.y + cr.bottom)).clamp(0, 32);
                    (il, it, ir, ib)
                } else {
                    (0, 0, 0, 0)
                };
                set_corner_preference(host, true);
                set_window_rect(
                    host,
                    work.left - il,
                    work.top - it,
                    (work.right - work.left) + il + ir,
                    (work.bottom - work.top) + it + ib,
                );
            } else {
                set_corner_preference(host, false);
                let saved = *session
                    .pre_expand_win_rect
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some((rx, ry, rw, rh)) = saved {
                    set_window_rect(host, rx, ry, rw, rh);
                }
            }
        }
    }

    pub fn needs_bounds_sync(
        mc: HWND,
        host: HWND,
        below_veil: Option<HWND>,
        target: (i32, i32, i32, i32),
    ) -> bool {
        let mut wr = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: checks live owned game window visibility, owner, Z-order, and screen coordinates.
        unsafe {
            if IsWindowVisible(mc) == 0
                || GetWindow(mc, GW_OWNER) != host
                || GetWindowRect(mc, &mut wr) == 0
            {
                return false;
            }
            if !is_window_above(mc, host) {
                return true;
            }
            if let Some(veil) = below_veil.filter(|&v| v != mc && is_valid(v)) {
                if !is_window_above(veil, mc) {
                    return true;
                }
            }
            wr.left != target.0
                || wr.top != target.1
                || (wr.right - wr.left) != target.2
                || (wr.bottom - wr.top) != target.3
        }
    }

    pub fn is_host_foreground(host: HWND) -> bool {
        // SAFETY: queries live window handle validity and current foreground HWND.
        unsafe { is_host_alive(host) && GetForegroundWindow() == host }
    }

    pub fn should_hand_focus_to_game(host: HWND) -> bool {
        // SAFETY: checks live host focus state and avoids interrupting pointer interactions on the
        // 44px Clade top header strip.
        unsafe {
            if !is_host_foreground(host) || IsIconic(host) != 0 {
                return false;
            }
            let mut pt = POINT { x: 0, y: 0 };
            if GetCursorPos(&mut pt) != 0 && ScreenToClient(host, &mut pt) != 0 {
                if let Some(layout) = layout_for_host(host) {
                    if pt.x >= 0
                        && pt.x < layout.webview_width
                        && pt.y >= 0
                        && pt.y < layout.webview_height
                    {
                        return false;
                    }
                }
            }
            true
        }
    }

    pub fn focus_window(mc: HWND) {
        // SAFETY: activates the owned top-level game window so GLFW's `GetActiveWindow()` matches.
        unsafe {
            if !mc.is_null() && IsWindow(mc) != 0 && IsWindowVisible(mc) != 0 {
                let _ = SetForegroundWindow(mc);
            }
        }
    }

    pub fn request_close(mc: HWND) {
        // SAFETY: posts asynchronous WM_CLOSE to the game window.
        unsafe {
            if IsWindow(mc) != 0 {
                let _ = PostMessageW(mc, WM_CLOSE, 0, 0);
            }
        }
    }

    pub fn os_animations_enabled() -> Option<bool> {
        let mut enabled: BOOL = 1;
        // SAFETY: `enabled` is an initialized stack `BOOL` out-parameter.
        let ok = unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                std::ptr::from_mut(&mut enabled).cast(),
                0,
            )
        };
        if ok != 0 {
            Some(enabled != 0)
        } else {
            None
        }
    }

    pub fn dismiss_veil_slot(slot: &VeilSlot) {
        let prev = slot.hwnd.swap(0, Ordering::AcqRel);
        if let Some(cancel) = slot.cancel.lock().unwrap_or_else(|e| e.into_inner()).take() {
            cancel.store(true, Ordering::Release);
        }
        if prev == 0 {
            return;
        }
        let veil = prev as usize as HWND;
        // SAFETY: posting WM_CLOSE wakes and closes any live veil HWND on its dedicated thread.
        unsafe {
            if !veil.is_null() && IsWindow(veil) != 0 {
                let _ = PostMessageW(veil, WM_CLOSE, 0, 0);
            }
        }
    }

    pub fn dismiss_veil(session: &ActiveSession) {
        dismiss_veil_slot(&session.veil_slot);
    }

    /// Captures the live embedded stage pixels strictly in RAM before `WM_CLOSE` closes Minecraft.
    pub fn capture_stage_snapshot(host: HWND, mc: HWND) -> Option<super::CapturedStage> {
        // SAFETY: invoked on the UI STA thread while `host` and `mc` are visible in the foreground.
        unsafe {
            if !is_host_alive(host)
                || IsIconic(host) != 0
                || IsWindowVisible(host) == 0
                || mc.is_null()
                || IsWindow(mc) == 0
                || IsWindowVisible(mc) == 0
            {
                return None;
            }
            let fg = GetForegroundWindow();
            if fg != host && fg != mc && GetWindow(fg, GW_OWNER) != host {
                return None;
            }
            let layout = layout_for_host(host)?;
            let (sx, sy, sw, sh) = stage_screen_rect(host, layout.stage)?;
            if sw < 16 || sh < 16 {
                return None;
            }

            let screen_dc = GetDC(std::ptr::null_mut());
            if screen_dc.is_null() {
                return None;
            }
            let mem_dc = CreateCompatibleDC(screen_dc);
            if mem_dc.is_null() {
                let _ = ReleaseDC(std::ptr::null_mut(), screen_dc);
                return None;
            }

            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: sw,
                    biHeight: -sh,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: [std::mem::zeroed(); 1],
            };
            let mut raw_bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let dib = CreateDIBSection(
                mem_dc,
                &bmi,
                DIB_RGB_COLORS,
                &mut raw_bits,
                std::ptr::null_mut(),
                0,
            );
            if dib.is_null() || raw_bits.is_null() {
                let _ = DeleteDC(mem_dc);
                let _ = ReleaseDC(std::ptr::null_mut(), screen_dc);
                return None;
            }

            let old_obj = SelectObject(mem_dc, dib.cast());
            let ok = BitBlt(mem_dc, 0, 0, sw, sh, screen_dc, sx, sy, SRCCOPY) != 0;
            let _ = ReleaseDC(std::ptr::null_mut(), screen_dc);

            let out = if ok {
                let raw_slice = std::slice::from_raw_parts(
                    raw_bits.cast::<u32>(),
                    (sw as usize).saturating_mul(sh as usize),
                );
                if raw_slice.iter().any(|&p| (p & 0x00FF_FFFF) != 0) {
                    let step = if sw > 1280 || sh > 800 {
                        2usize
                    } else {
                        1usize
                    };
                    let out_w = ((sw as usize) / step).max(1);
                    let out_h = ((sh as usize) / step).max(1);
                    let mut pixels = Vec::with_capacity(out_w * out_h);
                    let sw_u = sw as usize;
                    for oy in 0..out_h {
                        let row = &raw_slice[oy * step * sw_u..];
                        for ox in 0..out_w {
                            pixels.push(row[ox * step] & 0x00FF_FFFF);
                        }
                    }
                    Some(super::CapturedStage {
                        width: out_w as i32,
                        height: out_h as i32,
                        pixels,
                    })
                } else {
                    None
                }
            } else {
                None
            };

            if !old_obj.is_null() {
                SelectObject(mem_dc, old_obj);
            }
            let _ = DeleteObject(dib.cast());
            let _ = DeleteDC(mem_dc);
            out
        }
    }

    const VEIL_CLASS_NAME: &[u16] = &[
        b'C' as u16,
        b'l' as u16,
        b'a' as u16,
        b'd' as u16,
        b'e' as u16,
        b'E' as u16,
        b'm' as u16,
        b'b' as u16,
        b'e' as u16,
        b'd' as u16,
        b'V' as u16,
        b'e' as u16,
        b'i' as u16,
        b'l' as u16,
        0,
    ];
    static VEIL_CLASS_ONCE: Once = Once::new();

    struct VeilSurface {
        width: i32,
        height: i32,
        mem_dc: HDC,
        dib: HBITMAP,
        old_obj: HGDIOBJ,
        bits: *mut u32,
    }

    impl Drop for VeilSurface {
        fn drop(&mut self) {
            // SAFETY: owns the GDI memory DC and DIB created inside the veil thread.
            unsafe {
                if !self.mem_dc.is_null() {
                    if !self.old_obj.is_null() {
                        SelectObject(self.mem_dc, self.old_obj);
                    }
                    if !self.dib.is_null() {
                        let _ = DeleteObject(self.dib.cast());
                    }
                    let _ = DeleteDC(self.mem_dc);
                }
            }
        }
    }

    unsafe fn push_layered_bitmap(
        hwnd: HWND,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        mem_dc: HDC,
    ) -> bool {
        let pt_dst = POINT { x, y };
        let sz_dst = SIZE {
            cx: width,
            cy: height,
        };
        let pt_src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        UpdateLayeredWindow(
            hwnd,
            std::ptr::null_mut(),
            &pt_dst,
            &sz_dst,
            mem_dc,
            &pt_src,
            0,
            &blend,
            ULW_ALPHA,
        ) != 0
    }

    unsafe fn raise_above(hwnd: HWND, target: HWND) {
        if !target.is_null() && IsWindow(target) != 0 && !is_window_above(hwnd, target) {
            let insert_after = insert_after_above_host(hwnd, target);
            SetWindowPos(
                hwnd,
                insert_after,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    unsafe fn pump_thread_messages() {
        let mut msg = std::mem::zeroed::<MSG>();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            let _ = DispatchMessageW(&msg);
        }
    }

    unsafe extern "system" fn veil_wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_CLOSE {
            let _ = DestroyWindow(hwnd);
            return 0;
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    unsafe fn init_veil_window(
        host: HWND,
        rect: (i32, i32, i32, i32),
        initial_open: bool,
        snapshot: Option<&CapturedStage>,
    ) -> Option<(HWND, VeilSurface)> {
        let (sx, sy, sw, sh) = rect;
        let hinst = GetWindowLongPtrW(host, GWLP_HINSTANCE) as *mut core::ffi::c_void;
        VEIL_CLASS_ONCE.call_once(|| {
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(veil_wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinst,
                hIcon: std::ptr::null_mut(),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: VEIL_CLASS_NAME.as_ptr(),
            };
            let _ = RegisterClassW(&wc);
        });

        // Unowned popup with WS_EX_TOOLWINDOW keeps off the taskbar/Alt+Tab while avoiding cross-thread AttachThreadInput stalls.
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            VEIL_CLASS_NAME.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            sx,
            sy,
            sw,
            sh,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            tracing::warn!("could not create native embed transition veil window");
            return None;
        }

        let screen_dc = GetDC(std::ptr::null_mut());
        let mem_dc = CreateCompatibleDC(screen_dc);
        if !screen_dc.is_null() {
            let _ = ReleaseDC(std::ptr::null_mut(), screen_dc);
        }
        if mem_dc.is_null() {
            let _ = DestroyWindow(hwnd);
            return None;
        }

        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: sw,
                biHeight: -sh,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [std::mem::zeroed(); 1],
        };
        let mut raw_bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(
            mem_dc,
            &bmi,
            DIB_RGB_COLORS,
            &mut raw_bits,
            std::ptr::null_mut(),
            0,
        );
        if dib.is_null() || raw_bits.is_null() {
            tracing::warn!("could not allocate layered transition surface for embed veil");
            let _ = DeleteDC(mem_dc);
            let _ = DestroyWindow(hwnd);
            return None;
        }
        let old_obj = SelectObject(mem_dc, dib.cast());
        let surface = VeilSurface {
            width: sw,
            height: sh,
            mem_dc,
            dib,
            old_obj,
            bits: raw_bits.cast::<u32>(),
        };

        set_corner_preference(hwnd, true);
        let len = (sw as usize).saturating_mul(sh as usize);
        let buf = std::slice::from_raw_parts_mut(surface.bits, len);
        if initial_open {
            paint_reveal_bgra(buf, sw, sh, compute_reveal_frame(sw, sh, 0.0));
        } else {
            paint_close_bgra(buf, sw, sh, compute_close_frame(sw, sh, 0.0), snapshot);
        }
        if !push_layered_bitmap(hwnd, sx, sy, sw, sh, surface.mem_dc) {
            tracing::warn!("UpdateLayeredWindow rejected initial embed transition frame");
            drop(surface);
            let _ = DestroyWindow(hwnd);
            return None;
        }
        let insert_after = insert_after_above_host(hwnd, host);
        SetWindowPos(
            hwnd,
            insert_after,
            sx,
            sy,
            sw,
            sh,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        Some((hwnd, surface))
    }

    pub struct OpenVeilHandle {
        pub hwnd: HWND,
        mc_slot: Arc<AtomicU64>,
        activated: Arc<AtomicBool>,
        cancel: Arc<AtomicBool>,
    }

    pub fn create_open_veil(
        host: HWND,
        session: Arc<ActiveSession>,
        rect: (i32, i32, i32, i32),
    ) -> Option<OpenVeilHandle> {
        let (_, _, sw, sh) = rect;
        if sw < 16 || sh < 16 || !is_valid(host) {
            return None;
        }
        let host_id = host as usize as u64;
        let mc_slot = Arc::new(AtomicU64::new(0));
        let activated = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<u64>(1);

        let mc_slot_t = mc_slot.clone();
        let activated_t = activated.clone();
        let cancel_t = cancel.clone();
        let spawn = std::thread::Builder::new()
            .name("clade-embed-veil".to_owned())
            .spawn(move || {
                // SAFETY: `hwnd` and `surface` belong exclusively to this dedicated VSync thread.
                unsafe {
                    let host = host_id as usize as HWND;
                    let Some((hwnd, surface)) = init_veil_window(host, rect, true, None) else {
                        let _ = ready_tx.send(0);
                        return;
                    };
                    let id = hwnd as usize as u64;
                    session.veil_slot.set_active(id, cancel_t.clone());
                    let _ = ready_tx.send(id);

                    let len = (surface.width as usize).saturating_mul(surface.height as usize);
                    let created_at = std::time::Instant::now();
                    let mut anim_start: Option<std::time::Instant> = None;
                    let mut completed = false;
                    let mut last_mc: HWND = std::ptr::null_mut();
                    let mut x = rect.0;
                    let mut y = rect.1;

                    loop {
                        pump_thread_messages();
                        if cancel_t.load(Ordering::Acquire) || IsWindow(hwnd) == 0 {
                            break;
                        }
                        if DwmFlush() != 0 {
                            std::thread::sleep(Duration::from_millis(8));
                        }
                        pump_thread_messages();
                        if cancel_t.load(Ordering::Acquire) || IsWindow(hwnd) == 0 {
                            break;
                        }

                        let mc = mc_slot_t.load(Ordering::Acquire) as usize as HWND;
                        if !mc.is_null() {
                            last_mc = mc;
                        }
                        if !activated_t.load(Ordering::Acquire) {
                            if session.stop.load(Ordering::Acquire)
                                || !is_host_alive(host)
                                || IsIconic(host) != 0
                                || IsWindowVisible(host) == 0
                                || created_at.elapsed() > Duration::from_millis(1500)
                            {
                                break;
                            }
                            let z_target = if !mc.is_null() && IsWindow(mc) != 0 {
                                mc
                            } else {
                                host
                            };
                            raise_above(hwnd, z_target);
                            continue;
                        }

                        let start = *anim_start.get_or_insert_with(|| {
                            tracing::info!(
                                width = surface.width,
                                height = surface.height,
                                "started native game reveal animation"
                            );
                            std::time::Instant::now()
                        });
                        let elapsed_ms = start.elapsed().as_millis() as u64;
                        let fg = GetForegroundWindow();
                        let user_left = elapsed_ms > REVEAL_HOLD_MS + 100
                            && !fg.is_null()
                            && fg != host
                            && fg != mc;
                        if session.stop.load(Ordering::Acquire)
                            || !session.attached.load(Ordering::Acquire)
                            || !is_host_alive(host)
                            || IsIconic(host) != 0
                            || IsWindowVisible(host) == 0
                            || (!mc.is_null() && !is_attached_window_alive(mc, host))
                            || user_left
                        {
                            break;
                        }

                        if let Some(layout) = layout_for_host(host) {
                            if let Some((nx, ny, nw, nh)) = stage_screen_rect(host, layout.stage) {
                                if (nw - surface.width).abs() > 4 || (nh - surface.height).abs() > 4
                                {
                                    break;
                                }
                                x = nx;
                                y = ny;
                            }
                        }

                        let t = (elapsed_ms as f32) / (REVEAL_DURATION_MS as f32);
                        let buf = std::slice::from_raw_parts_mut(surface.bits, len);
                        let frame = compute_reveal_frame(surface.width, surface.height, t);
                        paint_reveal_bgra(buf, surface.width, surface.height, frame);
                        let _ = push_layered_bitmap(
                            hwnd,
                            x,
                            y,
                            surface.width,
                            surface.height,
                            surface.mem_dc,
                        );
                        let z_target = if !mc.is_null() && IsWindow(mc) != 0 {
                            mc
                        } else {
                            host
                        };
                        raise_above(hwnd, z_target);

                        if t >= 1.0 {
                            completed = true;
                            break;
                        }
                    }

                    if IsWindow(hwnd) != 0 {
                        let _ = DestroyWindow(hwnd);
                    }
                    pump_thread_messages();
                    drop(surface);
                    session.veil_slot.clear_if_matches(id, &cancel_t);
                    if completed && !cancel_t.load(Ordering::Acquire) {
                        session.focus_requested.store(true, Ordering::Release);
                        if !last_mc.is_null() && should_hand_focus_to_game(host) {
                            focus_window(last_mc);
                        }
                    }
                }
            });

        if spawn.is_err() {
            return None;
        }
        match ready_rx.recv_timeout(Duration::from_millis(80)) {
            Ok(id) if id != 0 => Some(OpenVeilHandle {
                hwnd: id as usize as HWND,
                mc_slot,
                activated,
                cancel,
            }),
            _ => {
                cancel.store(true, Ordering::Release);
                None
            }
        }
    }

    pub fn activate_open_veil(handle: &OpenVeilHandle, mc: HWND) {
        handle.mc_slot.store(mc as usize as u64, Ordering::Release);
        handle.activated.store(true, Ordering::Release);
    }

    pub fn cancel_open_veil(handle: &OpenVeilHandle, slot: &VeilSlot) {
        handle.cancel.store(true, Ordering::Release);
        dismiss_veil_slot(slot);
        // SAFETY: posting WM_CLOSE gently closes `handle.hwnd` if still registered.
        unsafe {
            if !handle.hwnd.is_null() && IsWindow(handle.hwnd) != 0 {
                let _ = PostMessageW(handle.hwnd, WM_CLOSE, 0, 0);
            }
        }
    }

    pub fn create_close_veil(
        host: HWND,
        slot: Arc<VeilSlot>,
        rect: (i32, i32, i32, i32),
        snapshot: Option<CapturedStage>,
    ) -> bool {
        let (_, _, sw, sh) = rect;
        if sw < 16 || sh < 16 || !is_valid(host) {
            return false;
        }
        dismiss_veil_slot(&slot);
        let host_id = host as usize as u64;
        let cancel = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<bool>(1);
        let cancel_t = cancel.clone();

        let spawn = std::thread::Builder::new()
            .name("clade-embed-veil".to_owned())
            .spawn(move || {
                // SAFETY: `hwnd` and `surface` belong exclusively to this dedicated VSync thread.
                unsafe {
                    let host = host_id as usize as HWND;
                    let Some((hwnd, surface)) =
                        init_veil_window(host, rect, false, snapshot.as_ref())
                    else {
                        let _ = ready_tx.send(false);
                        return;
                    };
                    let id = hwnd as usize as u64;
                    slot.set_active(id, cancel_t.clone());
                    let _ = ready_tx.send(true);
                    tracing::info!(
                        width = surface.width,
                        height = surface.height,
                        has_snapshot = snapshot.is_some(),
                        "started native game close animation"
                    );

                    let len = (surface.width as usize).saturating_mul(surface.height as usize);
                    let start = std::time::Instant::now();
                    let mut x = rect.0;
                    let mut y = rect.1;

                    loop {
                        pump_thread_messages();
                        if cancel_t.load(Ordering::Acquire) || IsWindow(hwnd) == 0 {
                            break;
                        }
                        if DwmFlush() != 0 {
                            std::thread::sleep(Duration::from_millis(8));
                        }
                        pump_thread_messages();
                        if cancel_t.load(Ordering::Acquire) || IsWindow(hwnd) == 0 {
                            break;
                        }

                        let elapsed_ms = start.elapsed().as_millis() as u64;
                        let fg = GetForegroundWindow();
                        let user_left = elapsed_ms > CLOSE_HOLD_MS + 100
                            && !fg.is_null()
                            && fg != host
                            && GetWindow(fg, GW_OWNER) != host;
                        if !is_host_alive(host)
                            || IsIconic(host) != 0
                            || IsWindowVisible(host) == 0
                            || user_left
                        {
                            break;
                        }

                        if let Some(layout) = layout_for_host(host) {
                            if let Some((nx, ny, nw, nh)) = stage_screen_rect(host, layout.stage) {
                                if (nw - surface.width).abs() > 4 || (nh - surface.height).abs() > 4
                                {
                                    break;
                                }
                                x = nx;
                                y = ny;
                            }
                        }

                        let t = (elapsed_ms as f32) / (CLOSE_DURATION_MS as f32);
                        let buf = std::slice::from_raw_parts_mut(surface.bits, len);
                        let frame = compute_close_frame(surface.width, surface.height, t);
                        paint_close_bgra(
                            buf,
                            surface.width,
                            surface.height,
                            frame,
                            snapshot.as_ref(),
                        );
                        let _ = push_layered_bitmap(
                            hwnd,
                            x,
                            y,
                            surface.width,
                            surface.height,
                            surface.mem_dc,
                        );
                        raise_above(hwnd, host);

                        if t >= 1.0 {
                            break;
                        }
                    }

                    if IsWindow(hwnd) != 0 {
                        let _ = DestroyWindow(hwnd);
                    }
                    pump_thread_messages();
                    drop(snapshot);
                    drop(surface);
                    slot.clear_if_matches(id, &cancel_t);
                }
            });

        if spawn.is_err() {
            return false;
        }
        match ready_rx.recv_timeout(Duration::from_millis(80)) {
            Ok(true) => true,
            _ => {
                cancel.store(true, Ordering::Release);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mul_div_255_is_bit_exact_for_all_byte_pairs() {
        for c in 0u32..=255 {
            for a in 0u32..=255 {
                assert_eq!(mul_div_255(c, a), (c * a + 127) / 255, "c={c}, a={a}");
            }
        }
    }

    #[test]
    fn stage_bounds_clamp_dimensions_and_corner_radius() {
        assert_eq!(
            StageBounds::clamped(14, 44, 0, 20, 40),
            StageBounds {
                x: 14,
                y: 44,
                width: 64,
                height: 64,
                radius: 32,
            }
        );
        assert_eq!(
            StageBounds::clamped(-5, 0, 1280, 720, 16),
            StageBounds {
                x: -5,
                y: 0,
                width: 1280,
                height: 720,
                radius: 16,
            }
        );
    }

    #[test]
    fn compute_framed_layout_scales_cleanly_and_is_disjoint_across_dpis() {
        assert!(compute_framed_layout(0, 720, 96).is_none());
        assert!(compute_framed_layout(1280, 63, 96).is_none());
        assert!(compute_framed_layout(63, 720, 96).is_none());

        for (dpi, expected_header) in [(96, 44), (120, 55), (144, 66), (192, 88)] {
            let layout = compute_framed_layout(1280, 720, dpi).expect("valid layout");
            assert_eq!(layout.webview_width, 1280);
            assert_eq!(layout.webview_height, expected_header);
            assert_eq!(
                layout.stage,
                StageBounds {
                    x: 0,
                    y: expected_header,
                    width: 1280,
                    height: (720 - expected_header) as u32,
                    radius: 0,
                }
            );
            assert_eq!(layout.webview_height, layout.stage.y);
            assert_eq!(
                layout.webview_height as u32 + layout.stage.height,
                720,
                "rectangles must partition full host height cleanly at dpi {dpi}"
            );
        }
    }

    #[test]
    fn embed_state_tracks_early_detach_reattach_and_idempotent_finish() {
        let state = EmbedState::new();
        assert_eq!(state.status(), GameEmbedStatus::default());

        let s1 = state.begin("survival".into(), 1001);
        assert!(
            !state.is_active(),
            "must remain inactive until window attaches"
        );
        assert_eq!(
            state.status(),
            GameEmbedStatus {
                active: false,
                watching: true,
                instance_id: Some("survival".into()),
                maximized: false,
                closing: false,
            }
        );
        assert!(state.current_bounds().0.is_none());

        state.set_bounds(StageBounds::clamped(0, 44, 1280, 676, 0));
        let (bounds, seq1) = state.current_bounds();
        assert_eq!(
            bounds,
            Some(StageBounds {
                x: 0,
                y: 44,
                width: 1280,
                height: 676,
                radius: 0,
            })
        );
        assert_eq!(seq1, 1);

        s1.game_win.store(42, Ordering::Release);
        s1.attached.store(true, Ordering::Release);
        s1.ever_attached.store(true, Ordering::Release);
        s1.strip_active.store(true, Ordering::Release);
        s1.maximized.store(true, Ordering::Release);
        s1.prev_host_clipchildren.store(false, Ordering::Release);
        *s1.attach_win_rect.lock().unwrap() = Some((100, 100, 1280, 720));

        assert_eq!(
            state.status(),
            GameEmbedStatus {
                active: true,
                watching: false,
                instance_id: Some("survival".into()),
                maximized: true,
                closing: false,
            }
        );

        s1.close_attempts.store(1, Ordering::Release);
        assert!(state.status().closing);

        // Simulated early window destruction before JVM exits:
        assert!(s1.mark_window_detached());
        assert!(!s1.mark_window_detached(), "second detach call is a no-op");
        assert!(!state.is_active());
        assert_eq!(state.status(), GameEmbedStatus::default());
        assert_eq!(s1.game_win.load(Ordering::Acquire), 0);
        assert!(!s1.maximized.load(Ordering::Acquire));
        assert!(s1.ever_attached.load(Ordering::Acquire));
        assert_eq!(
            *s1.attach_win_rect.lock().unwrap(),
            Some((100, 100, 1280, 720))
        );

        assert!(state.finish("other").is_none());
        let ended = state.finish("survival").unwrap();
        assert!(!ended.was_attached);
        assert!(ended.ever_attached);
        assert!(!ended.strip_active);
        assert!(!ended.prev_host_clipchildren);
        assert!(ended.close_snapshot.is_none());
        assert_eq!(ended.attach_win_rect, Some((100, 100, 1280, 720)));
        assert!(s1.stop.load(Ordering::Acquire));
        assert!(!state.is_active());
        assert_eq!(state.status(), GameEmbedStatus::default());
    }

    #[test]
    fn stage_to_screen_rect_maps_positive_and_negative_monitor_origins() {
        let stage = StageBounds::clamped(0, 44, 1280, 676, 0);
        assert_eq!(stage_to_screen_rect(100, 200, stage), (100, 244, 1280, 676));
        assert_eq!(
            stage_to_screen_rect(-1920, 0, stage),
            (-1920, 44, 1280, 676)
        );
    }

    #[test]
    fn should_animate_embed_respects_reduced_motion_and_os_preference() {
        assert!(!should_animate_embed("on", Some(true)));
        assert!(!should_animate_embed("on", Some(false)));
        assert!(should_animate_embed("off", Some(false)));
        assert!(should_animate_embed("off", Some(true)));
        assert!(should_animate_embed("system", Some(true)));
        assert!(!should_animate_embed("system", Some(false)));
        assert!(should_animate_embed("system", None));
    }

    #[test]
    fn compute_reveal_frame_clamps_endpoints_and_expands_smoothly() {
        for p in [0.05, 0.25] {
            let hold = compute_reveal_frame(1280, 676, p);
            assert_eq!(hold.inner_alpha, 255);
            assert_eq!(hold.outer_alpha, 255);
            assert_eq!(hold.border_alpha, 0);
            assert_eq!(hold.shadow_alpha, 0);
            assert_eq!(hold.shadow_spread, 0);
            assert_eq!(hold.radius, 0);
            let mut hold_buf = vec![0u32; 32 * 24];
            let small_hold = compute_reveal_frame(32, 24, p);
            paint_reveal_bgra(&mut hold_buf, 32, 24, small_hold);
            assert!(hold_buf
                .iter()
                .all(|&px| px == premul_bgra(243, 237, 227, 255)));
        }

        let early = compute_reveal_frame(1280, 676, 0.40);
        assert!(early.inner_alpha < 255);
        assert!(early.inner_left > 0 && early.inner_right < 1280);
        assert!(early.radius > 0);
        assert!(early.shadow_alpha > 0);
        assert!(early.border_alpha > 0);

        let mid = compute_reveal_frame(1280, 676, 0.60);
        assert_eq!(mid.inner_alpha, 0);
        assert!(mid.inner_left < early.inner_left);
        assert!(mid.inner_right > early.inner_right);
        assert!(mid.radius < early.radius);

        let late = compute_reveal_frame(1280, 676, 0.90);
        assert_eq!(late.inner_alpha, 0);
        assert!(late.outer_alpha < 255 && late.outer_alpha > 0);

        let end = compute_reveal_frame(1280, 676, 1.5);
        assert_eq!(
            end,
            RevealFrame {
                inner_left: 0,
                inner_top: 0,
                inner_right: 1280,
                inner_bottom: 676,
                radius: 0,
                inner_alpha: 0,
                outer_alpha: 0,
                border_alpha: 0,
                shadow_alpha: 0,
                shadow_spread: 0,
            }
        );
    }

    #[test]
    fn compute_close_frame_holds_then_shrinks_and_fades_smoothly() {
        let hold = compute_close_frame(1280, 676, 0.10);
        assert_eq!(
            hold,
            CloseFrame {
                inner_left: 0,
                inner_top: 0,
                inner_right: 1280,
                inner_bottom: 676,
                radius: 0,
                card_alpha: 255,
                border_alpha: 0,
                shadow_alpha: 0,
                shadow_spread: 0,
            }
        );

        let mid = compute_close_frame(1280, 676, 0.60);
        assert!(mid.inner_left > 0 && mid.inner_right < 1280);
        assert!(mid.inner_top > 0 && mid.inner_bottom <= 676);
        assert!(mid.radius > 0);
        assert!(mid.card_alpha < 255 && mid.card_alpha > 0);
        assert!(mid.border_alpha > 0);
        assert!(mid.shadow_alpha > 0);
        assert!(mid.shadow_spread > 0);

        let end = compute_close_frame(1280, 676, 1.2);
        assert_eq!(
            end,
            CloseFrame {
                inner_left: 0,
                inner_top: 0,
                inner_right: 1280,
                inner_bottom: 676,
                radius: 0,
                card_alpha: 0,
                border_alpha: 0,
                shadow_alpha: 0,
                shadow_spread: 0,
            }
        );
    }

    #[test]
    fn paint_reveal_bgra_maintains_premultiplied_invariants_across_sizes() {
        for (w, h) in [(1, 1), (17, 13), (320, 200)] {
            let mut buf = vec![0xFFFF_FFFF; w * h];
            for p in [0.0, 0.35, 0.75, 0.94, 1.0] {
                let frame = compute_reveal_frame(w as i32, h as i32, p);
                paint_reveal_bgra(&mut buf, w as i32, h as i32, frame);
                for &px in &buf {
                    let a = (px >> 24) & 0xFF;
                    let r = (px >> 16) & 0xFF;
                    let g = (px >> 8) & 0xFF;
                    let b = px & 0xFF;
                    assert!(
                        r <= a && g <= a && b <= a,
                        "non-premultiplied pixel {px:#010x} at {w}x{h} p={p}"
                    );
                }
            }
        }
    }

    #[test]
    fn paint_close_bgra_maintains_premultiplied_invariants_with_and_without_snapshot() {
        let snap = CapturedStage {
            width: 80,
            height: 50,
            pixels: (0..(80 * 50))
                .map(|i| {
                    let r = ((i * 7) % 256) as u32;
                    let g = ((i * 13) % 256) as u32;
                    let b = ((i * 19) % 256) as u32;
                    (r << 16) | (g << 8) | b
                })
                .collect(),
        };
        for (w, h) in [(1, 1), (17, 13), (320, 200)] {
            let mut buf = vec![0xFFFF_FFFF; w * h];
            for with_snap in [None, Some(&snap)] {
                for p in [0.0, 0.25, 0.65, 1.0] {
                    let frame = compute_close_frame(w as i32, h as i32, p);
                    paint_close_bgra(&mut buf, w as i32, h as i32, frame, with_snap);
                    for &px in &buf {
                        let a = (px >> 24) & 0xFF;
                        let r = (px >> 16) & 0xFF;
                        let g = (px >> 8) & 0xFF;
                        let b = px & 0xFF;
                        assert!(
                            r <= a && g <= a && b <= a,
                            "non-premultiplied close pixel {px:#010x} at {w}x{h} p={p}"
                        );
                    }
                    if p >= 1.0 {
                        assert!(buf.iter().all(|&px| px == 0));
                    }
                }
            }
        }
        // Corners outside shrunk card stay transparent so Clade UI renders underneath.
        let mut big = vec![0xFFFF_FFFF; 320 * 200];
        let mid = compute_close_frame(320, 200, 0.65);
        paint_close_bgra(&mut big, 320, 200, mid, Some(&snap));
        assert_eq!(big[0], 0);
        assert_eq!(big[319], 0);
        assert_ne!(big[100 * 320 + 160], 0);
    }
}
