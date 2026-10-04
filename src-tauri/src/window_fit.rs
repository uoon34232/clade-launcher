//! The launcher's initial window: the compact default size, shrunk only when the monitor's usable
//! area (the work area, which excludes the taskbar) cannot hold it, and centred there. Clade is a
//! windowed launcher: it is never maximized or fullscreen and nothing about its size is persisted,
//! so an old configuration can never place it partly off-screen.

/// The compact default (logical pixels) and the smallest size the layout supports.
pub const DEFAULT_SIZE: (f64, f64) = (1280.0, 720.0);
pub const MIN_SIZE: (f64, f64) = (960.0, 600.0);
/// Breathing room kept between the window and the work-area edges (logical pixels, per side).
pub const MARGIN: f64 = 24.0;

/// A monitor's usable area in physical pixels, plus its scale factor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkArea {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
}

/// Size (logical pixels) and top-left corner (physical pixels, as monitors are laid out).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub width: f64,
    pub height: f64,
    pub min_width: f64,
    pub min_height: f64,
    pub x: f64,
    pub y: f64,
}

/// Fits the default size into `area`, keeping the 16:9 proportions when it has to shrink and never
/// growing past the default. The minimum size is lowered if the area itself is smaller than it.
pub fn fit(area: WorkArea) -> Placement {
    let scale = if area.scale.is_finite() && area.scale > 0.0 {
        area.scale
    } else {
        1.0
    };
    let avail_w = (f64::from(area.width) / scale - 2.0 * MARGIN).max(1.0);
    let avail_h = (f64::from(area.height) / scale - 2.0 * MARGIN).max(1.0);
    let k = (avail_w / DEFAULT_SIZE.0)
        .min(avail_h / DEFAULT_SIZE.1)
        .min(1.0);
    let min_width = MIN_SIZE.0.min(avail_w);
    let min_height = MIN_SIZE.1.min(avail_h);
    let width = (DEFAULT_SIZE.0 * k).floor().max(min_width).min(avail_w);
    let height = (DEFAULT_SIZE.1 * k).floor().max(min_height).min(avail_h);
    let x = f64::from(area.x) + (f64::from(area.width) - width * scale) / 2.0;
    let y = f64::from(area.y) + (f64::from(area.height) - height * scale) / 2.0;
    Placement {
        width,
        height,
        min_width,
        min_height,
        x: x.round(),
        y: y.round(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(w: u32, h: u32, scale: f64) -> WorkArea {
        WorkArea {
            x: 0,
            y: 0,
            width: w,
            height: h,
            scale,
        }
    }
    fn fits(a: WorkArea, p: Placement) {
        let (pw, ph) = (p.width * a.scale, p.height * a.scale);
        assert!(p.x >= f64::from(a.x) && p.y >= f64::from(a.y), "{p:?}");
        assert!(p.x + pw <= f64::from(a.x) + f64::from(a.width), "{p:?}");
        assert!(p.y + ph <= f64::from(a.y) + f64::from(a.height), "{p:?}");
        assert!(p.min_width <= p.width && p.min_height <= p.height);
    }

    #[test]
    fn normal_desktops_keep_the_compact_default() {
        // 1080p and 1440p minus a taskbar, 4K at 150% and 200% scaling
        for a in [
            area(1920, 1032, 1.0),
            area(2560, 1392, 1.0),
            area(3840, 2064, 1.5),
            area(3840, 2064, 2.0),
        ] {
            let p = fit(a);
            assert_eq!((p.width, p.height), DEFAULT_SIZE, "{a:?}");
            assert_eq!((p.min_width, p.min_height), MIN_SIZE);
            fits(a, p);
        }
    }

    #[test]
    fn it_is_centred_in_the_work_area_not_the_screen() {
        let a = WorkArea {
            x: 0,
            y: 48,
            width: 1920,
            height: 1032,
            scale: 1.0,
        };
        let p = fit(a);
        assert_eq!(p.x, 320.0);
        assert_eq!(p.y, 48.0 + 156.0);
        // a second monitor to the left (negative x)
        let a = WorkArea {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1032,
            scale: 1.0,
        };
        assert_eq!(fit(a).x, -1920.0 + 320.0);
    }

    #[test]
    fn small_screens_shrink_the_window_to_fit_and_keep_proportions() {
        // 1366x768 with a taskbar: 1366x728 usable
        let a = area(1366, 728, 1.0);
        let p = fit(a);
        assert!(p.width < DEFAULT_SIZE.0 && p.height <= 680.0, "{p:?}");
        assert!((p.width / p.height - 16.0 / 9.0).abs() < 0.02);
        fits(a, p);
        // 1280x720 with a taskbar: 1280x680 usable
        let a = area(1280, 680, 1.0);
        let p = fit(a);
        assert!(p.height <= 632.0 && p.width <= 1232.0, "{p:?}");
        assert!(p.width >= MIN_SIZE.0 && p.height >= MIN_SIZE.1, "{p:?}");
        fits(a, p);
    }

    #[test]
    fn a_work_area_smaller_than_the_minimum_still_holds_the_window() {
        for a in [
            area(1024, 600, 1.0),
            area(800, 500, 1.0),
            area(1280, 720, 1.5),
        ] {
            fits(a, fit(a));
        }
    }

    #[test]
    fn nonsense_scale_factors_are_treated_as_one() {
        for s in [0.0, -2.0, f64::NAN, f64::INFINITY] {
            assert_eq!(fit(area(1920, 1032, s)), fit(area(1920, 1032, 1.0)));
        }
    }
}
