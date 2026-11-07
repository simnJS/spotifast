//! The main window's rounded corners.
//!
//! On Linux the window has no desktop frame, so the app rounds its own
//! corners: the window is transparent (`with_transparent`), and after every
//! layer is painted the pixels outside the rounded outline are cleared back
//! to transparent, then a one-point outline in the palette's colour is drawn
//! along the edge. egui clips only to rectangles, so the corners are cleared
//! with the renderer itself, which also takes in dialogs' dimming and the
//! theme fade. Maximized and full screen windows keep square corners.
//!
//! On Windows the desktop rounds the frameless window itself (see
//! `crate::window::round_corners`, Windows only).

use egui::{Context, CornerRadius, Id, LayerId, Order, Stroke, StrokeKind};

use crate::theme::Palette;

/// The corner radius, in points, close to GNOME's own windows.
pub const RADIUS: u8 = 12;

/// Whether the window rounds its corners now: Linux, drawing its own title
/// bar, and neither maximized nor full screen.
pub fn rounded(ctx: &Context) -> bool {
    let (maximized, fullscreen) = ctx.input(|input| {
        let viewport = input.viewport();
        (
            viewport.maximized.unwrap_or(false),
            viewport.fullscreen.unwrap_or(false),
        )
    });
    rounded_for(
        cfg!(target_os = "linux"),
        fastframe_titlebar::enabled(),
        maximized,
        fullscreen,
    )
}

const fn rounded_for(linux: bool, own_titlebar: bool, maximized: bool, fullscreen: bool) -> bool {
    linux && own_titlebar && !maximized && !fullscreen
}

/// Whether the main window opens transparent, so its corners can be rounded.
pub fn transparent_window() -> bool {
    cfg!(target_os = "linux") && fastframe_titlebar::enabled()
}

/// Rounds the window's corners over everything painted this frame. Call it
/// once per frame; it paints in the topmost layer.
pub fn paint(ctx: &Context, palette: &Palette) {
    if !rounded(ctx) {
        return;
    }
    let screen = ctx.content_rect();
    if !screen.is_positive() {
        return;
    }
    let painter = ctx.layer_painter(LayerId::new(Order::Debug, Id::new("window-corners")));
    painter.add(egui::PaintCallback {
        rect: screen,
        callback: std::sync::Arc::new(eframe::egui_glow::CallbackFn::new(|info, painter| {
            clear_corners(painter.gl(), info.screen_size_px, info.pixels_per_point);
        })),
    });
    painter.rect_stroke(
        screen,
        CornerRadius::same(RADIUS),
        Stroke::new(1.0, palette.outline),
        StrokeKind::Inside,
    );
}

/// Clears to transparent each pixel whose centre lies outside the rounded
/// outline, one scissored row at a time. The outline drawn after it smooths
/// the edge.
fn clear_corners(gl: &eframe::glow::Context, screen_px: [u32; 2], pixels_per_point: f32) {
    use eframe::glow::HasContext as _;
    let [width, height] = screen_px.map(|side| side as i32);
    let cuts = corner_cuts(f32::from(RADIUS) * pixels_per_point);
    // SAFETY: called by egui_glow while it paints this window, on its
    // context and thread; egui_glow restores its own state afterwards.
    unsafe {
        gl.enable(eframe::glow::SCISSOR_TEST);
        gl.color_mask(true, true, true, true);
        gl.clear_color(0.0, 0.0, 0.0, 0.0);
        for (row, &cut) in cuts.iter().enumerate() {
            let cut = (cut as i32).min(width / 2);
            let row = row as i32;
            if cut <= 0 || row >= height / 2 {
                continue;
            }
            // OpenGL counts rows from the bottom.
            for y in [height - 1 - row, row] {
                for x in [0, width - cut] {
                    gl.scissor(x, y, cut, 1);
                    gl.clear(eframe::glow::COLOR_BUFFER_BIT);
                }
            }
        }
    }
}

/// For each pixel row from a corner's edge inwards, how many pixels from the
/// side lie outside a circle of `radius` pixels.
fn corner_cuts(radius: f32) -> Vec<u32> {
    if radius <= 0.0 {
        return Vec::new();
    }
    let rows = radius.ceil() as usize;
    (0..rows)
        .map(|row| {
            let dy = radius - (row as f32 + 0.5);
            let inside = (radius * radius - dy * dy).max(0.0).sqrt();
            // A pixel is cleared when its centre is outside the circle.
            let edge = radius - inside;
            (edge + 0.5).floor().max(0.0) as u32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_floating_linux_window_rounds_its_corners() {
        assert!(rounded_for(true, true, false, false));
        assert!(!rounded_for(true, true, true, false), "maximized");
        assert!(!rounded_for(true, true, false, true), "full screen");
        assert!(!rounded_for(true, false, false, false), "desktop frame");
        assert!(!rounded_for(false, true, false, false), "Windows' DWM");
    }

    #[test]
    fn the_cut_narrows_from_the_corner_to_the_straight_edge() {
        let cuts = corner_cuts(12.0);
        assert_eq!(cuts.len(), 12);
        assert!(cuts[0] >= 7, "the corner row loses most of the arc");
        assert_eq!(*cuts.last().unwrap(), 0, "the arc meets the side");
        assert!(cuts.windows(2).all(|pair| pair[0] >= pair[1]));
        // Twice the pixels at twice the scale.
        let hidpi = corner_cuts(24.0);
        assert_eq!(hidpi.len(), 24);
        assert!(hidpi[0] >= 2 * cuts[0] - 1);
        assert!(corner_cuts(0.0).is_empty());
    }
}
