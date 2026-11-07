//! The Stats page's charts, drawn with egui's painter in the theme's
//! colours: streams per day (or per week over a long range) and listening
//! by hour of day, each bar telling its figures under the pointer.

use egui::{Align2, CornerRadius, Rect, Sense, Ui, pos2, vec2};

use crate::i18n::{Locale, gettext};
use crate::stats::query::Count;
use crate::theme::{self, Palette};

use super::summary::{self, played};

/// The room the value axis takes on the left.
const AXIS: f32 = 36.0;
/// The room the labels take under the bars.
const LABELS: f32 = 20.0;

/// One bar: its height, what its tooltip says, and its label under the
/// axis, if any.
struct Bar {
    value: u32,
    tooltip: String,
    label: Option<String>,
}

/// A rounded "nice" ceiling for the axis: 1, 2 or 5 times a power of ten.
pub fn nice_ceiling(most: u32) -> u32 {
    let most = most.max(1);
    let mut step = 1u32;
    loop {
        for factor in [1, 2, 5] {
            let ceiling = step.saturating_mul(factor);
            if ceiling >= most {
                return ceiling;
            }
        }
        step = step.saturating_mul(10);
    }
}

fn bars(ui: &mut Ui, palette: &Palette, height: f32, bars: &[Bar], label: &str) {
    let width = ui.available_width().max(120.0);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, label));
    if !ui.is_rect_visible(rect) || bars.is_empty() {
        return;
    }
    let painter = ui.painter_at(rect.expand(2.0));
    let plot = Rect::from_min_max(
        pos2(rect.left() + AXIS, rect.top() + 6.0),
        pos2(rect.right(), rect.bottom() - LABELS),
    );
    let ceiling = nice_ceiling(bars.iter().map(|bar| bar.value).max().unwrap_or(0));
    let small = theme::regular(11.0);
    // Three gridlines: none, half, the ceiling.
    for (share, value) in [(0.0, 0), (0.5, ceiling / 2), (1.0, ceiling)] {
        let y = plot.bottom() - plot.height() * share;
        painter.hline(
            plot.x_range(),
            y,
            egui::Stroke::new(1.0, palette.outline.gamma_multiply(0.7)),
        );
        painter.text(
            pos2(plot.left() - 8.0, y),
            Align2::RIGHT_CENTER,
            summary::count(u64::from(value)),
            small.clone(),
            palette.secondary,
        );
    }
    let slot = plot.width() / bars.len() as f32;
    let gap = if slot >= 6.0 { (slot * 0.25).min(6.0) } else { 0.0 };
    let pointer = response.hover_pos();
    let mut hovered = None;
    for (index, bar) in bars.iter().enumerate() {
        let left = plot.left() + slot * index as f32;
        let column = Rect::from_min_max(pos2(left, plot.top()), pos2(left + slot, plot.bottom()));
        let is_hovered = pointer.is_some_and(|at| at.x >= column.left() && at.x < column.right());
        if is_hovered {
            hovered = Some(index);
            painter.rect_filled(column, CornerRadius::same(2), palette.surface_hover);
        }
        let share = bar.value as f32 / ceiling as f32;
        if bar.value > 0 {
            let top = plot.bottom() - (plot.height() * share).max(2.0);
            let shape = Rect::from_min_max(
                pos2(left + gap / 2.0, top),
                pos2(left + slot - gap / 2.0, plot.bottom()),
            );
            let radius = (shape.width() / 2.0).min(3.0) as u8;
            painter.rect_filled(
                shape,
                CornerRadius {
                    nw: radius,
                    ne: radius,
                    sw: 0,
                    se: 0,
                },
                if is_hovered {
                    palette.accent_hover
                } else {
                    palette.accent
                },
            );
        }
        if let Some(label) = &bar.label {
            painter.text(
                pos2(column.center().x, plot.bottom() + 4.0),
                Align2::CENTER_TOP,
                label,
                small.clone(),
                palette.secondary,
            );
        }
    }
    if let Some(index) = hovered {
        response.on_hover_text_at_pointer(&bars[index].tooltip);
    }
}

fn date(date: jiff::civil::Date) -> String {
    date.strftime("%b %-d").to_string()
}

/// Streams per day across the range, or per week (and longer) when the
/// days would be too thin to point at.
pub fn per_day(
    ui: &mut Ui,
    palette: &Palette,
    locale: Locale,
    days: &[crate::stats::query::Day],
) {
    let most = ((ui.available_width() - AXIS) / 5.0).clamp(12.0, 120.0) as usize;
    let buckets = summary::buckets(days, most);
    let every = buckets.len().div_ceil(6).max(1);
    let bars: Vec<Bar> = buckets
        .iter()
        .enumerate()
        .map(|(index, &(first, last, count))| {
            let when = if first == last {
                date(first)
            } else {
                format!("{} – {}", date(first), date(last))
            };
            Bar {
                value: count.streams,
                tooltip: format!("{when}\n{}", played(locale, count)),
                label: (index % every == 0).then(|| date(first)),
            }
        })
        .collect();
    self::bars(
        ui,
        palette,
        170.0,
        &bars,
        &gettext(locale, "Streams per day"),
    );
}

/// Streams by hour of day, midnight first.
pub fn per_hour(ui: &mut Ui, palette: &Palette, locale: Locale, hours: &[Count; 24]) {
    let bars: Vec<Bar> = hours
        .iter()
        .enumerate()
        .map(|(hour, count)| Bar {
            value: count.streams,
            tooltip: format!(
                "{hour:02}:00 – {:02}:00\n{}",
                (hour + 1) % 24,
                played(locale, *count)
            ),
            label: (hour % 6 == 0).then(|| format!("{hour:02}:00")),
        })
        .collect();
    self::bars(
        ui,
        palette,
        150.0,
        &bars,
        &gettext(locale, "Listening by hour of day"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_axis_rounds_up_to_a_readable_ceiling() {
        assert_eq!(nice_ceiling(0), 1);
        assert_eq!(nice_ceiling(1), 1);
        assert_eq!(nice_ceiling(3), 5);
        assert_eq!(nice_ceiling(7), 10);
        assert_eq!(nice_ceiling(11), 20);
        assert_eq!(nice_ceiling(420), 500);
        assert_eq!(nice_ceiling(501), 1000);
        assert!(nice_ceiling(u32::MAX) >= 1);
    }

    #[test]
    fn a_chart_draws_without_panicking_at_any_width() {
        let ctx = egui::Context::default();
        let start = jiff::civil::date(2024, 1, 1);
        let days: Vec<crate::stats::query::Day> = (0..400)
            .map(|offset| crate::stats::query::Day {
                date: start.checked_add(jiff::Span::new().days(offset)).unwrap(),
                count: Count {
                    streams: (offset % 9) as u32,
                    ms: 60_000,
                },
            })
            .collect();
        for width in [80.0, 360.0, 1400.0] {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.set_max_width(width);
                per_day(ui, &Palette::dark(), Locale::default(), &days);
                per_day(ui, &Palette::light(), Locale::default(), &[]);
                per_hour(ui, &Palette::dark(), Locale::default(), &[Count::default(); 24]);
            });
        }
    }
}
