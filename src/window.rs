//! Checks saved window positions before restoring them.
//!
//! eframe already places the window on-screen. The session can still refer
//! to a monitor that was unplugged, so check before moving the window there.

/// The download guide, which documents the OpenGL 2.0 requirement and is
/// the help page the no-window dialog offers.
pub const DOWNLOAD_PAGE: &str = "https://spotifast.rocks/download/";

/// The dialog fastframe-graphics shows when no window could open, in the
/// interface language: Windows on ARM is pointed at Microsoft's
/// compatibility pack, where its OpenGL comes from; anything else at the
/// graphics driver and the download guide.
pub fn no_window_dialog(
    locale: crate::i18n::Locale,
    trouble: &fastframe_graphics::Trouble<'_>,
) -> fastframe_graphics::Dialog {
    use crate::i18n::gettext;
    let english = fastframe_graphics::Dialog::english(trouble);
    fastframe_graphics::Dialog {
        title: gettext(locale, "Spotifast cannot open its window").into_owned(),
        body: no_window_body(locale, trouble.cause, trouble.help_url)
            .unwrap_or_else(|| english.body.clone()),
        open_label: gettext(locale, "Open page").into_owned(),
        close_label: gettext(locale, "Close").into_owned(),
        ..english
    }
}

/// The dialog's explanation for `cause`, or `None` for a cause fastframe
/// added later, which keeps fastframe's English.
fn no_window_body(
    locale: crate::i18n::Locale,
    cause: fastframe_graphics::Cause,
    help_url: Option<&str>,
) -> Option<String> {
    use crate::i18n::gettext;
    Some(match cause {
        fastframe_graphics::Cause::WindowsOnArm => gettext(
            locale,
            "On Windows on ARM, the OpenGL that Spotifast draws with comes from Microsoft's OpenCL, OpenGL & Vulkan Compatibility Pack. Open its Microsoft Store page to install or repair it?",
        )
        .into_owned(),
        fastframe_graphics::Cause::NoGraphicsDriver => gettext(
            locale,
            // Translators: Keep {url} exactly as written. It becomes the address
            // of the Spotifast download page.
            "Spotifast needs a graphics driver with OpenGL 2.0 or newer, and this PC or virtual machine does not provide one. Install the graphics driver, or turn on GPU acceleration for the virtual machine. Open {url} to read more?",
        )
        .replace("{url}", help_url.unwrap_or(DOWNLOAD_PAGE)),
        _ => return None,
    })
}

/// Minimize the active window without leaving egui's macOS viewport flag stale
/// when the user later restores it from the Dock.
pub fn minimize_window(ctx: &egui::Context) {
    #[cfg(target_os = "macos")]
    {
        use objc2::{MainThreadMarker, sel};
        use objc2_app_kit::NSApplication;
        use objc2_foundation::NSObjectNSDelayedPerforming;

        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        let window = app.currentEvent().and_then(|event| event.window(mtm));
        if let Some(window) = window.or_else(|| app.keyWindow()) {
            // Let egui finish this frame before AppKit minimizes the window.
            // SAFETY: NSWindow's selector takes one optional sender argument.
            unsafe {
                window.performSelector_withObject_afterDelay(sel!(miniaturize:), None, 0.0);
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    #[cfg(target_os = "macos")]
    let _ = ctx;
}

/// The main window's smallest size with no side panel open. The page and
/// its top bar fit beside the narrowest sidebar from here.
pub const MAIN_MIN_SIZE: [f32; 2] = [760.0, 520.0];

/// A remembered main window size to open at, or `None` for the default. Under
/// the minimum is not a size anyone chose, since the window cannot be dragged
/// that small, and opening at it leaves a window too small to find (#704).
pub fn usable_size(size: Option<[f32; 2]>) -> Option<[f32; 2]> {
    size.filter(|size| size[0] >= MAIN_MIN_SIZE[0] && size[1] >= MAIN_MIN_SIZE[1])
}

static FIXED_SIZE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the main window keeps one size it was given, as demo shots ask,
/// so the interface must not raise its minimum.
pub fn fixed_size() -> bool {
    FIXED_SIZE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Pins the main window to the size it is created with.
pub fn set_fixed_size(on: bool) {
    FIXED_SIZE.store(on, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_window_points_arm_at_the_store_pack_and_others_at_the_guide() {
        use fastframe_graphics::{Cause, Trouble};
        let english = crate::i18n::Locale::English;
        let details = "egui_glow requires opengl 2.0+";
        let arm = no_window_dialog(
            english,
            &Trouble::new(
                "Spotifast",
                Cause::WindowsOnArm,
                details,
                Some(DOWNLOAD_PAGE),
            ),
        );
        assert!(arm.body.contains("Compatibility Pack"));
        assert!(
            arm.link
                .as_deref()
                .is_some_and(|link| link.starts_with("ms-windows-store:"))
        );

        let x64 = no_window_dialog(
            english,
            &Trouble::new(
                "Spotifast",
                Cause::NoGraphicsDriver,
                details,
                Some(DOWNLOAD_PAGE),
            ),
        );
        assert_eq!(x64.title, "Spotifast cannot open its window");
        assert_eq!(x64.link.as_deref(), Some(DOWNLOAD_PAGE));
        assert!(x64.body.contains("OpenGL 2.0"));
        assert!(x64.body.contains("virtual machine"));
        assert!(x64.body.contains(DOWNLOAD_PAGE));
        assert!(!x64.body.contains("{url}"));
        assert_eq!(
            (x64.open_label.as_str(), x64.close_label.as_str()),
            ("Open page", "Close")
        );
    }

    /// A remembered size under the minimum is not one anyone chose, since the
    /// window cannot be dragged that small. It is not restored, so the window
    /// opens at its default size instead.
    #[test]
    fn a_remembered_size_below_the_minimum_is_not_restored() {
        assert_eq!(usable_size(Some([64.0, 64.0])), None);
        assert_eq!(usable_size(Some([MAIN_MIN_SIZE[0] - 1.0, 800.0])), None);
        assert_eq!(usable_size(Some([1240.0, MAIN_MIN_SIZE[1] - 1.0])), None);
        assert_eq!(usable_size(None), None);
        for size in [MAIN_MIN_SIZE, [1240.0, 800.0], [1920.0, 1129.0]] {
            assert_eq!(usable_size(Some(size)), Some(size));
        }
    }
}
