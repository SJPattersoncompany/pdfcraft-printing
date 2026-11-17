//! Where the main window opens, and the size it comes back at.
//!
//! The configured window is 1200×800 logical pixels with a minimum of 800×600, but the
//! config sets no position. Tao then passes `CW_USEDEFAULT` for both coordinates.
//! Before that window exists, the single-instance plugin has already created this
//! process's first overlapped window at (0, 0) with a 0×0 size (its hidden message
//! target). Windows applies `CW_USEDEFAULT` — and the startup show placement — only to
//! the first overlapped window. The real window is the second one, so it cascades off
//! that 0×0 window and is realized as the system minimum frame (caption buttons, a
//! sliver of the page) in the corner. The configured minimum does not correct it:
//! tao's first `WM_GETMINMAXINFO` arrives before window userdata exists and is handed
//! to `DefWindowProc`. Nothing stored the geometry, so the next launch repeats the
//! same creation and the size the user dragged is forgotten.
//!
//! This module places the window itself, in physical pixels, after the HWND exists and
//! before it is shown. The first launch (and any saved rect that is tiny or on no
//! monitor) is the configured size, centered on the primary work area and clamped to
//! it. A later launch restores the last normal — not maximized, not fullscreen, not
//! minimized — size and position when that rect still meets the minimum and intersects
//! a work area. It is then clamped fully inside that work area.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use tauri::{Manager, PhysicalPosition, PhysicalSize, Runtime, WebviewWindow, WindowEvent};

/// Configured inner size, logical pixels (`tauri.conf.json` / the print-test overlay).
pub const LOGICAL_WIDTH: f64 = 1200.0;
pub const LOGICAL_HEIGHT: f64 = 800.0;
/// Configured minimum inner size, logical pixels. The window stays resizable.
pub const LOGICAL_MIN_WIDTH: f64 = 800.0;
pub const LOGICAL_MIN_HEIGHT: f64 = 600.0;

const STATE_FILE: &str = "window-placement.json";
/// Title-bar and resize-border thickness we are willing to believe. A bogus reading
/// must not shrink the client to nothing.
const MAX_INSET: f64 = 200.0;
const SLOP: f64 = 1.0;

static PLACING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MonitorArea {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}

/// Outer top-left, inner size. Both in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Insets {
    pub width: f64,
    pub height: f64,
}

impl Insets {
    pub const ZERO: Self = Self {
        width: 0.0,
        height: 0.0,
    };
}

fn sane_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

fn sane_inset(value: f64) -> f64 {
    if value.is_finite() && (0.0..=MAX_INSET).contains(&value) {
        value
    } else {
        0.0
    }
}

pub fn sane_insets(insets: Insets) -> Insets {
    Insets {
        width: sane_inset(insets.width),
        height: sane_inset(insets.height),
    }
}

fn usable_monitor(monitor: &MonitorArea) -> bool {
    monitor.x.is_finite()
        && monitor.y.is_finite()
        && monitor.width.is_finite()
        && monitor.height.is_finite()
        && monitor.width > 1.0
        && monitor.height > 1.0
}

/// `monitors[0]` is the primary. An empty or fully invalid list becomes one
/// 1920×1080 monitor at the origin so a launch still has a normal size.
pub fn normalize_monitors(monitors: &[MonitorArea]) -> Vec<MonitorArea> {
    let mut out: Vec<MonitorArea> = monitors
        .iter()
        .copied()
        .filter(usable_monitor)
        .map(|mut monitor| {
            monitor.scale = sane_scale(monitor.scale);
            monitor
        })
        .collect();
    if out.is_empty() {
        out.push(MonitorArea {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
            scale: 1.0,
        });
    }
    out
}

fn inner_room(monitor: MonitorArea, insets: Insets) -> (f64, f64) {
    let insets = sane_insets(insets);
    (
        (monitor.width - insets.width).max(1.0),
        (monitor.height - insets.height).max(1.0),
    )
}

/// Smallest inner size we will treat as a real window on this monitor.
/// The configured minimum, unless the work area itself is smaller.
fn min_inner(monitor: MonitorArea, insets: Insets) -> (f64, f64) {
    let scale = sane_scale(monitor.scale);
    let (room_w, room_h) = inner_room(monitor, insets);
    (
        (LOGICAL_MIN_WIDTH * scale).min(room_w).max(1.0),
        (LOGICAL_MIN_HEIGHT * scale).min(room_h).max(1.0),
    )
}

pub fn is_tiny(rect: WindowRect, monitor: MonitorArea, insets: Insets) -> bool {
    if !rect.width.is_finite() || !rect.height.is_finite() {
        return true;
    }
    let (min_w, min_h) = min_inner(monitor, insets);
    rect.width + SLOP < min_w || rect.height + SLOP < min_h
}

fn outer_size(rect: WindowRect, insets: Insets) -> (f64, f64) {
    let insets = sane_insets(insets);
    (rect.width + insets.width, rect.height + insets.height)
}

fn overlap_area(rect: WindowRect, insets: Insets, monitor: MonitorArea) -> f64 {
    let (ow, oh) = outer_size(rect, insets);
    let left = rect.x.max(monitor.x);
    let top = rect.y.max(monitor.y);
    let right = (rect.x + ow).min(monitor.x + monitor.width);
    let bottom = (rect.y + oh).min(monitor.y + monitor.height);
    let w = right - left;
    let h = bottom - top;
    if w > 0.0 && h > 0.0 {
        w * h
    } else {
        0.0
    }
}

fn best_monitor(rect: WindowRect, monitors: &[MonitorArea], insets: Insets) -> Option<MonitorArea> {
    monitors
        .iter()
        .copied()
        .map(|monitor| (overlap_area(rect, insets, monitor), monitor))
        .filter(|(area, _)| *area > 1.0)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, monitor)| monitor)
}

fn fully_inside(rect: WindowRect, monitor: MonitorArea, insets: Insets) -> bool {
    let (ow, oh) = outer_size(rect, insets);
    rect.x >= monitor.x - SLOP
        && rect.y >= monitor.y - SLOP
        && rect.x + ow <= monitor.x + monitor.width + SLOP
        && rect.y + oh <= monitor.y + monitor.height + SLOP
}

/// Pull `rect` fully inside `monitor`. Shrinks an oversized window down to the
/// work area; does not grow a small one.
fn fit_inside(rect: WindowRect, monitor: MonitorArea, insets: Insets) -> WindowRect {
    let insets = sane_insets(insets);
    let (room_w, room_h) = inner_room(monitor, insets);
    let width = rect.width.clamp(1.0, room_w);
    let height = rect.height.clamp(1.0, room_h);
    let outer_w = width + insets.width;
    let outer_h = height + insets.height;
    let max_x = monitor.x + monitor.width - outer_w;
    let max_y = monitor.y + monitor.height - outer_h;
    WindowRect {
        x: rect.x.clamp(monitor.x.min(max_x), max_x.max(monitor.x)),
        y: rect.y.clamp(monitor.y.min(max_y), max_y.max(monitor.y)),
        width,
        height,
    }
}

pub fn default_placement(primary: MonitorArea, insets: Insets) -> WindowRect {
    let scale = sane_scale(primary.scale);
    let insets = sane_insets(insets);
    let (room_w, room_h) = inner_room(primary, insets);
    let (min_w, min_h) = min_inner(primary, insets);
    let width = (LOGICAL_WIDTH * scale).clamp(min_w, room_w.max(min_w));
    let height = (LOGICAL_HEIGHT * scale).clamp(min_h, room_h.max(min_h));
    let outer_w = width + insets.width;
    let outer_h = height + insets.height;
    WindowRect {
        x: primary.x + (primary.width - outer_w) / 2.0,
        y: primary.y + (primary.height - outer_h) / 2.0,
        width,
        height,
    }
}

/// Saved geometry worth showing again: it meets the minimum and actually
/// intersects a connected work area. The result is fully inside that area.
pub fn place_saved(
    saved: WindowRect,
    monitors: &[MonitorArea],
    insets: Insets,
) -> Option<WindowRect> {
    let monitors = normalize_monitors(monitors);
    let monitor = best_monitor(saved, &monitors, insets)?;
    if is_tiny(saved, monitor, insets) {
        return None;
    }
    let fitted = fit_inside(saved, monitor, insets);
    if is_tiny(fitted, monitor, insets) {
        None
    } else {
        Some(fitted)
    }
}

pub fn launch_placement(
    saved: Option<WindowRect>,
    monitors: &[MonitorArea],
    insets: Insets,
) -> WindowRect {
    let monitors = normalize_monitors(monitors);
    if let Some(saved) = saved {
        if let Some(placed) = place_saved(saved, &monitors, insets) {
            return placed;
        }
    }
    default_placement(monitors[0], insets)
}

/// `None` means the window already has a normal on-screen size; leave it.
/// A tiny or off-screen window is replaced, preferring the saved normal rect.
pub fn focus_placement(
    current: WindowRect,
    saved: Option<WindowRect>,
    monitors: &[MonitorArea],
    insets: Insets,
) -> Option<WindowRect> {
    let monitors = normalize_monitors(monitors);
    if let Some(monitor) = best_monitor(current, &monitors, insets) {
        if !is_tiny(current, monitor, insets) && fully_inside(current, monitor, insets) {
            return None;
        }
        if !is_tiny(current, monitor, insets) {
            return Some(fit_inside(current, monitor, insets));
        }
    }
    if let Some(saved) = saved {
        if let Some(placed) = place_saved(saved, &monitors, insets) {
            return Some(placed);
        }
    }
    Some(default_placement(monitors[0], insets))
}

pub fn parse_saved(text: &str) -> Option<WindowRect> {
    let rect: WindowRect = serde_json::from_str(text).ok()?;
    if rect.x.is_finite()
        && rect.y.is_finite()
        && rect.width.is_finite()
        && rect.height.is_finite()
        && rect.width > 1.0
        && rect.height > 1.0
        && rect.width < 100_000.0
        && rect.height < 100_000.0
    {
        Some(rect)
    } else {
        None
    }
}

pub fn read_saved(path: &Path) -> Option<WindowRect> {
    let text = fs::read_to_string(path).ok()?;
    parse_saved(&text)
}

pub fn write_saved_file(path: &Path, rect: &WindowRect) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    if fs::create_dir_all(parent).is_err() {
        return false;
    }
    let Ok(text) = serde_json::to_string(rect) else {
        return false;
    };
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, text).is_err() {
        return false;
    }
    fs::rename(&tmp, path).is_ok()
}

fn state_path<R: Runtime>(window: &WebviewWindow<R>) -> Option<PathBuf> {
    window
        .app_handle()
        .path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join(STATE_FILE))
}

fn area_from(monitor: &tauri::Monitor) -> MonitorArea {
    let work = monitor.work_area();
    MonitorArea {
        x: work.position.x as f64,
        y: work.position.y as f64,
        width: work.size.width as f64,
        height: work.size.height as f64,
        scale: monitor.scale_factor(),
    }
}

fn same_area(a: &MonitorArea, b: &MonitorArea) -> bool {
    (a.x - b.x).abs() <= SLOP
        && (a.y - b.y).abs() <= SLOP
        && (a.width - b.width).abs() <= SLOP
        && (a.height - b.height).abs() <= SLOP
}

fn collect_monitors<R: Runtime>(window: &WebviewWindow<R>) -> Vec<MonitorArea> {
    let available = window.available_monitors().unwrap_or_default();
    let mut areas: Vec<MonitorArea> = available.iter().map(area_from).collect();
    if let Ok(Some(primary)) = window.primary_monitor() {
        let primary = area_from(&primary);
        if let Some(index) = areas.iter().position(|area| same_area(area, &primary)) {
            areas.swap(0, index);
        } else {
            areas.insert(0, primary);
        }
    }
    normalize_monitors(&areas)
}

fn measure_insets<R: Runtime>(window: &WebviewWindow<R>) -> Insets {
    match (window.outer_size(), window.inner_size()) {
        (Ok(outer), Ok(inner)) => sane_insets(Insets {
            width: outer.width.saturating_sub(inner.width) as f64,
            height: outer.height.saturating_sub(inner.height) as f64,
        }),
        _ => Insets::ZERO,
    }
}

fn current_rect<R: Runtime>(window: &WebviewWindow<R>) -> Option<WindowRect> {
    let pos = window.outer_position().ok()?;
    let size = window.inner_size().ok()?;
    Some(WindowRect {
        x: pos.x as f64,
        y: pos.y as f64,
        width: size.width as f64,
        height: size.height as f64,
    })
}

fn apply_rect<R: Runtime>(window: &WebviewWindow<R>, rect: WindowRect) {
    PLACING.store(true, Ordering::SeqCst);
    let width = rect.width.round().max(1.0) as u32;
    let height = rect.height.round().max(1.0) as u32;
    let _ = window.set_size(PhysicalSize::new(width, height));
    let _ = window.set_position(PhysicalPosition::new(
        rect.x.round() as i32,
        rect.y.round() as i32,
    ));
    PLACING.store(false, Ordering::SeqCst);
}

fn clear_max_fullscreen<R: Runtime>(window: &WebviewWindow<R>) {
    if window.is_fullscreen().unwrap_or(false) {
        let _ = window.set_fullscreen(false);
    }
    if window.is_maximized().unwrap_or(false) {
        let _ = window.unmaximize();
    }
}

fn show_and_focus<R: Runtime>(window: &WebviewWindow<R>) {
    let _ = window.show();
    if window.set_focus().is_err() {
        let _ = window.request_user_attention(Some(tauri::UserAttentionType::Informational));
    }
}

fn persist_rect<R: Runtime>(window: &WebviewWindow<R>, rect: &WindowRect) {
    if let Some(path) = state_path(window) {
        let _ = write_saved_file(&path, rect);
    }
}

fn load_saved<R: Runtime>(window: &WebviewWindow<R>) -> Option<WindowRect> {
    state_path(window).and_then(|path| read_saved(&path))
}

fn persist_from_window<R: Runtime>(window: &WebviewWindow<R>) {
    if PLACING.load(Ordering::SeqCst) {
        return;
    }
    if window.is_minimized().unwrap_or(true)
        || window.is_maximized().unwrap_or(true)
        || window.is_fullscreen().unwrap_or(true)
    {
        return;
    }
    let monitors = collect_monitors(window);
    let insets = measure_insets(window);
    let Some(rect) = current_rect(window) else {
        return;
    };
    let monitor = best_monitor(rect, &monitors, insets).unwrap_or(monitors[0]);
    if is_tiny(rect, monitor, insets) {
        return;
    }
    persist_rect(window, &rect);
}

fn install_persistence<R: Runtime>(window: WebviewWindow<R>) {
    let tracked = window.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Moved(_) | WindowEvent::Resized(_) | WindowEvent::CloseRequested { .. } => {
            persist_from_window(&tracked);
        }
        _ => {}
    });
}

/// First show of this process. Hidden until the normal rect is applied.
pub fn on_launch<R: Runtime>(window: &WebviewWindow<R>) {
    clear_max_fullscreen(window);
    let monitors = collect_monitors(window);
    let saved = load_saved(window);
    // The frame thickness is only meaningful once the HWND exists, and the first
    // ShowWindow can still be the one Windows lets STARTUPINFO override. Measure,
    // place, show, then place again so a show-command cannot leave the tiny rect.
    let insets = measure_insets(window);
    let rect = launch_placement(saved, &monitors, insets);
    apply_rect(window, rect);
    let insets = measure_insets(window);
    let rect = launch_placement(saved, &monitors, insets);
    apply_rect(window, rect);
    show_and_focus(window);
    let insets = measure_insets(window);
    let rect = launch_placement(saved, &monitors, insets);
    apply_rect(window, rect);
    install_persistence(window.clone());
    persist_rect(window, &rect);
}

/// A second launch (another PDF, or a file opened from the shell). Restores a
/// minimized window to a normal size and never leaves it maximized or fullscreen.
pub fn on_second_instance<R: Runtime>(window: &WebviewWindow<R>) {
    clear_max_fullscreen(window);
    if window.is_minimized().unwrap_or(false) {
        let _ = window.unminimize();
    }
    // Unminimize can restore a maximized show-state. Drop that too.
    clear_max_fullscreen(window);

    let monitors = collect_monitors(window);
    let insets = measure_insets(window);
    let current = current_rect(window).unwrap_or(WindowRect {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
    });
    if let Some(rect) = focus_placement(current, load_saved(window), &monitors, insets) {
        apply_rect(window, rect);
        show_and_focus(window);
        // SW_RESTORE / the first ShowWindow can put the tiny normal-rect back.
        apply_rect(window, rect);
        persist_rect(window, &rect);
    } else {
        show_and_focus(window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(w: f64, h: f64, scale: f64) -> MonitorArea {
        MonitorArea {
            x: 0.0,
            y: 0.0,
            width: w,
            height: h,
            scale,
        }
    }

    fn at(x: f64, y: f64, w: f64, h: f64, scale: f64) -> MonitorArea {
        MonitorArea {
            x,
            y,
            width: w,
            height: h,
            scale,
        }
    }

    #[test]
    fn first_launch_is_configured_size_centered_on_primary() {
        let placed = launch_placement(None, &[screen(1920.0, 1080.0, 1.0)], Insets::ZERO);
        assert_eq!(
            placed,
            WindowRect {
                x: 360.0,
                y: 140.0,
                width: 1200.0,
                height: 800.0,
            }
        );
    }

    #[test]
    fn first_launch_shrinks_to_the_work_area() {
        let placed = launch_placement(None, &[screen(1000.0, 700.0, 1.0)], Insets::ZERO);
        assert_eq!(
            placed,
            WindowRect {
                x: 0.0,
                y: 0.0,
                width: 1000.0,
                height: 700.0,
            }
        );
    }

    #[test]
    fn first_launch_uses_physical_pixels_at_the_monitor_scale() {
        // 1200×800 logical at 1.5 is 1800×1200 physical, centered in 2560×1440.
        let placed = launch_placement(None, &[screen(2560.0, 1440.0, 1.5)], Insets::ZERO);
        assert_eq!(
            placed,
            WindowRect {
                x: 380.0,
                y: 120.0,
                width: 1800.0,
                height: 1200.0,
            }
        );
    }

    #[test]
    fn work_area_smaller_than_the_minimum_still_fits() {
        let placed = launch_placement(None, &[screen(640.0, 480.0, 1.0)], Insets::ZERO);
        assert_eq!(placed.width, 640.0);
        assert_eq!(placed.height, 480.0);
        assert!(placed.width + SLOP < LOGICAL_MIN_WIDTH);
    }

    #[test]
    fn zero_scale_does_not_collapse_the_window() {
        let placed = launch_placement(None, &[screen(1920.0, 1080.0, 0.0)], Insets::ZERO);
        assert_eq!(placed.width, 1200.0);
        assert_eq!(placed.height, 800.0);
    }

    #[test]
    fn insets_are_included_when_centering() {
        let insets = Insets {
            width: 16.0,
            height: 40.0,
        };
        let placed = launch_placement(None, &[screen(1920.0, 1080.0, 1.0)], insets);
        assert_eq!(placed.width, 1200.0);
        assert_eq!(placed.height, 800.0);
        assert_eq!(placed.x, (1920.0 - 1216.0) / 2.0);
        assert_eq!(placed.y, (1080.0 - 840.0) / 2.0);
    }

    #[test]
    fn saved_normal_rect_is_restored() {
        let saved = WindowRect {
            x: 100.0,
            y: 80.0,
            width: 1400.0,
            height: 900.0,
        };
        let placed = launch_placement(Some(saved), &[screen(1920.0, 1080.0, 1.0)], Insets::ZERO);
        assert_eq!(placed, saved);
    }

    #[test]
    fn tiny_saved_rect_falls_back_to_the_configured_size() {
        let saved = WindowRect {
            x: 0.0,
            y: 0.0,
            width: 120.0,
            height: 40.0,
        };
        let placed = launch_placement(Some(saved), &[screen(1920.0, 1080.0, 1.0)], Insets::ZERO);
        assert_eq!(placed.width, 1200.0);
        assert_eq!(placed.height, 800.0);
        assert_eq!(placed.x, 360.0);
    }

    #[test]
    fn offscreen_saved_rect_is_not_restored() {
        let saved = WindowRect {
            x: 5000.0,
            y: 5000.0,
            width: 1200.0,
            height: 800.0,
        };
        let placed = launch_placement(Some(saved), &[screen(1920.0, 1080.0, 1.0)], Insets::ZERO);
        assert_eq!(placed.x, 360.0);
        assert_eq!(placed.y, 140.0);
    }

    #[test]
    fn partial_overhang_is_pulled_back_onto_the_work_area() {
        let saved = WindowRect {
            x: 1500.0,
            y: -40.0,
            width: 1000.0,
            height: 700.0,
        };
        let placed = launch_placement(Some(saved), &[screen(1920.0, 1080.0, 1.0)], Insets::ZERO);
        assert_eq!(
            placed,
            WindowRect {
                x: 920.0,
                y: 0.0,
                width: 1000.0,
                height: 700.0,
            }
        );
    }

    #[test]
    fn oversized_saved_rect_shrinks_to_the_work_area() {
        let saved = WindowRect {
            x: 10.0,
            y: 10.0,
            width: 3000.0,
            height: 2000.0,
        };
        let placed = launch_placement(Some(saved), &[screen(1600.0, 900.0, 1.0)], Insets::ZERO);
        assert_eq!(
            placed,
            WindowRect {
                x: 0.0,
                y: 0.0,
                width: 1600.0,
                height: 900.0,
            }
        );
    }

    #[test]
    fn saved_rect_on_the_second_monitor_stays_there() {
        let monitors = [
            screen(1920.0, 1080.0, 1.0),
            at(1920.0, 0.0, 1920.0, 1080.0, 1.0),
        ];
        let saved = WindowRect {
            x: 2100.0,
            y: 120.0,
            width: 1100.0,
            height: 800.0,
        };
        let placed = launch_placement(Some(saved), &monitors, Insets::ZERO);
        assert_eq!(placed, saved);
    }

    #[test]
    fn monitor_with_more_overlap_wins() {
        let monitors = [
            screen(1920.0, 1080.0, 1.0),
            at(1800.0, 0.0, 1920.0, 1080.0, 1.0),
        ];
        let saved = WindowRect {
            x: 1700.0,
            y: 100.0,
            width: 1000.0,
            height: 800.0,
        };
        let placed = launch_placement(Some(saved), &monitors, Insets::ZERO);
        assert!(placed.x >= 1800.0);
        assert!(placed.x + placed.width <= 1800.0 + 1920.0 + SLOP);
    }

    #[test]
    fn focus_keeps_a_normal_onscreen_window() {
        let current = WindowRect {
            x: 40.0,
            y: 50.0,
            width: 1100.0,
            height: 750.0,
        };
        assert_eq!(
            focus_placement(current, None, &[screen(1920.0, 1080.0, 1.0)], Insets::ZERO),
            None
        );
    }

    #[test]
    fn focus_replaces_a_tiny_window_with_the_saved_normal_size() {
        let current = WindowRect {
            x: 0.0,
            y: 0.0,
            width: 140.0,
            height: 32.0,
        };
        let saved = WindowRect {
            x: 200.0,
            y: 100.0,
            width: 1280.0,
            height: 820.0,
        };
        assert_eq!(
            focus_placement(
                current,
                Some(saved),
                &[screen(1920.0, 1080.0, 1.0)],
                Insets::ZERO
            ),
            Some(saved)
        );
    }

    #[test]
    fn focus_of_a_tiny_window_without_a_save_uses_the_configured_size() {
        let current = WindowRect {
            x: 0.0,
            y: 0.0,
            width: 80.0,
            height: 24.0,
        };
        let placed = focus_placement(current, None, &[screen(1920.0, 1080.0, 1.0)], Insets::ZERO)
            .expect("replaced");
        assert_eq!(placed.width, 1200.0);
        assert_eq!(placed.height, 800.0);
        assert_eq!(placed.x, 360.0);
    }

    #[test]
    fn focus_clamps_a_large_window_that_hangs_off_the_work_area() {
        let current = WindowRect {
            x: 1400.0,
            y: 40.0,
            width: 1000.0,
            height: 800.0,
        };
        let placed = focus_placement(current, None, &[screen(1920.0, 1080.0, 1.0)], Insets::ZERO)
            .expect("clamped");
        assert_eq!(placed.width, 1000.0);
        assert_eq!(placed.x, 920.0);
        assert_eq!(placed.y, 40.0);
    }

    #[test]
    fn high_dpi_minimum_rejects_a_physically_small_window() {
        // 900×600 physical at scale 2 is 450×300 logical, under 800×600.
        let saved = WindowRect {
            x: 0.0,
            y: 0.0,
            width: 900.0,
            height: 600.0,
        };
        let placed = launch_placement(Some(saved), &[screen(3840.0, 2160.0, 2.0)], Insets::ZERO);
        assert_eq!(placed.width, 2400.0);
        assert_eq!(placed.height, 1600.0);
    }

    #[test]
    fn saved_file_round_trip_and_rejection() {
        let dir = std::env::temp_dir().join(format!(
            "pdfcraft-placement-{}-{}",
            std::process::id(),
            "round"
        ));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("window-placement.json");
        let rect = WindowRect {
            x: 12.0,
            y: 34.0,
            width: 1100.0,
            height: 700.0,
        };
        assert!(write_saved_file(&path, &rect));
        assert_eq!(read_saved(&path), Some(rect));
        assert!(parse_saved("{\"x\":0,\"y\":0,\"width\":0,\"height\":10}").is_none());
        assert!(parse_saved("not json").is_none());
        assert!(parse_saved("{\"x\":1,\"y\":2,\"width\":null,\"height\":10}").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_monitor_list_still_produces_a_normal_window() {
        let placed = launch_placement(None, &[], Insets::ZERO);
        assert_eq!(placed.width, 1200.0);
        assert_eq!(placed.height, 800.0);
    }
}
