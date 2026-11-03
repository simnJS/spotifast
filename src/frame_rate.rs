//! The frame rates the visualizers draw at: the default, the range the
//! Settings dial goes through, its stops, and the screen rates it learns.

/// How many frames a second the visualizers draw by default.
pub const DEFAULT_FPS: u32 = 60;
/// Configurable frame-rate range. Zero is handled separately as uncapped.
pub const FPS_RANGE: std::ops::RangeInclusive<u32> = 10..=360;
/// Whether a refresh rate a screen reports is a rate at all: Windows reports
/// 0 or 1 Hz for "the hardware's default", which says nothing. Screens
/// faster than the dial goes still count.
pub fn is_screen_rate(hz: u32) -> bool {
    hz >= *FPS_RANGE.start()
}
/// Frame-rate slider stops: 30, 60, screen rate, current rate, and uncapped.
pub fn fps_stops(screen: u32, current: u32) -> Vec<u32> {
    let mut rates = vec![30, 60];
    let screen = if is_screen_rate(screen) { screen } else { 0 };
    for extra in [screen, current] {
        if extra > 0 && !rates.contains(&extra) {
            rates.push(extra);
        }
    }
    rates.sort_unstable();
    rates.push(0);
    rates
}

/// Label for a frame-rate stop, marking the screen's refresh rate.
pub fn fps_label(locale: crate::i18n::Locale, rate: u32, screen: u32) -> String {
    use crate::i18n::gettext;
    match rate {
        0 => gettext(locale, "Uncapped").into_owned(),
        rate if rate == screen => gettext(
            locale,
            // Translators: A frame rate that matches the display. Keep {rate}.
            "{rate} fps, your screen",
        )
        .replace("{rate}", &rate.to_string()),
        rate => format!("{rate} fps"),
    }
}

/// A screen's refresh rate in whole hertz, from the millihertz a monitor
/// reports, when it is a rate at all (see [`is_screen_rate`]).
pub fn screen_hz(millihertz: u32) -> Option<u32> {
    let hz = (f64::from(millihertz) / 1000.0).round() as u32;
    is_screen_rate(hz).then_some(hz)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hardware_default_is_no_screen_rate() {
        assert!(!is_screen_rate(0));
        assert!(!is_screen_rate(1), "Windows's hardware default");
        assert!(is_screen_rate(60));
        assert!(is_screen_rate(165));
        assert!(is_screen_rate(480), "faster than the dial goes");
        assert_eq!(fps_stops(1, 60), vec![30, 60, 0], "no stop for it");
    }

    /// The dial stops at the two rates everyone knows, the screen's own
    /// when it is neither of them, and uncapped, in that order.
    #[test]
    fn the_frame_rate_stops_where_it_is_worth_stopping() {
        assert_eq!(fps_stops(144, 144), vec![30, 60, 144, 0]);
        assert_eq!(fps_stops(0, 60), vec![30, 60, 0], "no screen has said");
        assert_eq!(fps_stops(60, 60), vec![30, 60, 0], "and no stop twice");
        assert_eq!(
            fps_stops(50, 60),
            vec![30, 50, 60, 0],
            "a slower screen takes its place in the order"
        );
        assert_eq!(
            fps_stops(144, 90),
            vec![30, 60, 90, 144, 0],
            "a rate set by hand keeps a stop of its own"
        );
        assert_eq!(fps_label(crate::i18n::Locale::English, 0, 144), "Uncapped");
        assert_eq!(
            fps_label(crate::i18n::Locale::English, 144, 144),
            "144 fps, your screen"
        );
        assert_eq!(fps_label(crate::i18n::Locale::English, 30, 144), "30 fps");
    }

    /// A monitor's millihertz round to the hertz the dial shows, the default
    /// Windows reports instead of a rate is no rate at all, and a screen
    /// faster than the dial goes keeps its rate.
    #[test]
    fn a_screen_reports_its_rate_in_millihertz() {
        assert_eq!(screen_hz(59_940), Some(60));
        assert_eq!(screen_hz(143_981), Some(144));
        assert_eq!(screen_hz(164_000), Some(164));
        assert_eq!(screen_hz(0), None);
        assert_eq!(screen_hz(1_000), None, "Windows's hardware default");
        assert_eq!(screen_hz(480_000), Some(480), "faster than the dial goes");
    }
}
