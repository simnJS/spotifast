//! Navigation arrows, search, and the account menu above every page.

use std::sync::Arc;

use egui::{Align, CornerRadius, Galley, Layout, Sense, Vec2, pos2, vec2};

use crate::api::models::pick_image;
use crate::app::App;
use crate::i18n::gettext;
use crate::model::{Action, BackgroundActivity, Page};
use crate::theme::{self, Icon, Palette};

/// The gap the bar keeps between everything it lays out.
const ITEM_SPACING: f32 = 8.0;
/// The account button: a 48 px disc holding the 32 px avatar, as in
/// Spotify's top bar.
const AVATAR_SIZE: f32 = 48.0;
const AVATAR_IMAGE: f32 = 32.0;
/// The bell and the other square buttons: a 32 px box around an 18 px glyph.
const ICON_BUTTON_ICON: f32 = 18.0;
const ICON_BUTTON_SIZE: f32 = 32.0;
/// The menu, back and forward buttons at the left end.
const NAV_BUTTON: f32 = 32.0;
const NAV_ICON: f32 = 18.0;
/// The Home disc and the search pill beside it share one height, at most
/// Spotify's 48 px, and the pill's browse button sits behind a divider.
const PILL_HEIGHT: f32 = 48.0;
const PILL_BROWSE: f32 = 56.0;
const SPINNER_SIZE: f32 = 15.0;
/// A badge is as tall as its text plus this, and as wide as its text plus
/// the padding its own label needs.
const BADGE_PADDING_Y: f32 = 12.0;
/// The text starts 24 px in; leave 8 px after it to match the space before
/// the icon.
const UPDATE_BADGE_PADDING: f32 = 32.0;
/// The width the search field aims for, the most it ever takes, and the
/// least it shrinks to before the badges give up their labels instead.
const SEARCH_IDEAL: f32 = 200.0;
const SEARCH_MAX: f32 = 474.0;
const SEARCH_FLOOR: f32 = 130.0;
// After the badges collapse, a right panel can leave less than 130 points.
// Keep the original 80-point minimum inside the page's own toolbar.
const SEARCH_MIN: f32 = 80.0;
/// Everything at the right end whose width never changes: the page padding,
/// the avatar, the What's New bell, and the spacing between them. The cursor
/// stops at the left edge of the last button, so this counts one gap after
/// it. The spinner and the badges are measured on top of it because they
/// come and go.
const RIGHT_CONTROLS_WIDTH: f32 =
    super::widgets::PAGE_PADDING + AVATAR_SIZE + ITEM_SPACING + ICON_BUTTON_SIZE + ITEM_SPACING;

/// The Home disc and the gap after it, which come before the search field.
const HOME_WIDTH: f32 = PILL_HEIGHT + ITEM_SPACING;
/// What precedes the field until the bar has drawn once: the page padding,
/// the menu, back and forward buttons and the gaps after them, and Home.
const LEAD_GUESS: f32 =
    super::widgets::PAGE_PADDING + 3.0 * NAV_BUTTON + 3.0 * ITEM_SPACING + 4.0 + HOME_WIDTH;
/// A badge collapsed to its icon: a square as tall as its 12.5 pt label.
const BADGE_CHIP: f32 = 15.0 + BADGE_PADDING_Y;

/// The search pill's background, which spans the field, the clear button
/// and the browse button.
pub(crate) fn pill_id() -> egui::Id {
    egui::Id::new("global-search-pill")
}

fn lead_id() -> egui::Id {
    egui::Id::new("topbar-lead")
}

/// The narrowest the bar, and so the page under it, can be before its
/// controls run into each other: the narrowest field, with the spinner and
/// the update badge as an icon. Counting them even while they are away keeps
/// the panels and the window from changing width as they come and go.
pub fn least_width(ctx: &egui::Context) -> f32 {
    let lead = ctx
        .data(|data| data.get_temp(lead_id()))
        .unwrap_or(LEAD_GUESS);
    least_width_after(lead)
}

fn least_width_after(lead: f32) -> f32 {
    lead + SEARCH_MIN
        + RIGHT_CONTROLS_WIDTH
        + SPINNER_SIZE
        + ITEM_SPACING
        + ITEM_SPACING
        + BADGE_CHIP
}

/// How the top bar divides itself for one window width.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TopbarFit {
    /// How wide the search field may be.
    search: f32,
    /// Whether the badges have the room to spell themselves out.
    labels: bool,
}

/// Divide the bar. The search field keeps the half it has always had, but
/// never so much that the right end has to reach over it, and the badges
/// fall back to their icons before the field shrinks past reading size.
///
/// `labelled` and `icons` are what the badges ask for with and without their
/// text, each already including the spacing that precedes it.
fn topbar_fit(room: f32, controls: f32, labelled: f32, icons: f32) -> TopbarFit {
    // SEARCH_IDEAL is above SEARCH_FLOOR, so the clamp below is well ordered.
    let ideal = (room * 0.5).clamp(SEARCH_IDEAL, SEARCH_MAX);
    let labels = room - controls - labelled >= SEARCH_FLOOR;
    let badges = if labels { labelled } else { icons };
    TopbarFit {
        search: (room - controls - badges).clamp(SEARCH_MIN, ideal),
        labels,
    }
}

/// What a badge asks of the bar, including the spacing before it.
fn badge_width(galley: Option<&Arc<Galley>>, padding: f32, labels: bool) -> f32 {
    galley.map_or(0.0, |galley| {
        ITEM_SPACING
            + if labels {
                galley.size().x + padding
            } else {
                galley.size().y + BADGE_PADDING_Y
            }
    })
}

/// A pill at the right end of the bar: an icon with its label, or the icon
/// alone once the bar is too narrow to spare the room for words.
fn badge(
    ui: &mut egui::Ui,
    palette: &Palette,
    icon: Icon,
    galley: Arc<Galley>,
    padding: f32,
    labels: bool,
) -> egui::Response {
    let height = galley.size().y + BADGE_PADDING_Y;
    let size = if labels {
        vec2(galley.size().x + padding, height)
    } else {
        Vec2::splat(height)
    };
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    // Collapsed to its icon the badge has no text on screen, so its label
    // reaches a screen reader as the widget's name.
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), galley.text())
    });
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(14),
        palette.accent.gamma_multiply(0.16),
    );
    let icon_center = if labels {
        pos2(rect.left() + 14.0, rect.center().y)
    } else {
        rect.center()
    };
    icon.image(palette.accent, 13.0).paint_at(
        ui,
        egui::Rect::from_center_size(icon_center, Vec2::splat(13.0)),
    );
    if labels {
        ui.painter().galley(
            pos2(rect.left() + 24.0, rect.center().y - galley.size().y / 2.0),
            galley,
            palette.accent,
        );
    }
    response
}

/// One of the plain glyph buttons at the left end: the menu, Back and
/// Forward. Spotify draws them without a disc, dimmed while they can't be
/// used.
fn nav_button(
    ui: &mut egui::Ui,
    palette: &Palette,
    icon: Icon,
    enabled: bool,
    tooltip: &str,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::splat(NAV_BUTTON),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, tooltip));
    if ui.is_rect_visible(rect) {
        let color = if !enabled {
            palette.dim
        } else if response.hovered() || response.has_focus() {
            palette.text
        } else {
            palette.secondary
        };
        let scale = if response.is_pointer_button_down_on() {
            0.92
        } else {
            1.0
        };
        theme::paint_icon(ui, icon, rect, NAV_ICON * scale, color);
    }
    theme::focus_ring(ui, &response);
    if enabled {
        response.on_hover_text(tooltip)
    } else {
        response
    }
}

/// Spotify's Home button: a disc the pill's height, filled like the pill,
/// its house bright while Home is the page.
fn home_button(
    ui: &mut egui::Ui,
    palette: &Palette,
    diameter: f32,
    current: bool,
    tooltip: &str,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tooltip)
    });
    if ui.is_rect_visible(rect) {
        let lifted = response.hovered() || response.has_focus();
        let grow = if response.is_pointer_button_down_on() {
            0.97
        } else if lifted {
            1.04
        } else {
            1.0
        };
        let fill = if lifted {
            palette.surface_hover
        } else {
            palette.surface
        };
        ui.painter()
            .circle_filled(rect.center(), diameter / 2.0 * grow, fill);
        let color = if current || lifted {
            palette.text
        } else {
            palette.secondary
        };
        theme::paint_icon(ui, Icon::House, rect, 24.0 * grow, color);
    }
    theme::focus_ring(ui, &response);
    response.on_hover_text(tooltip)
}

/// Spotify's search pill: the magnifier, the field, a clear button once
/// there is text, and the browse button behind a divider. Returns the
/// field's response and whether browse was clicked.
#[allow(clippy::too_many_arguments)]
fn search_pill(
    ui: &mut egui::Ui,
    palette: &Palette,
    locale: crate::i18n::Locale,
    id: egui::Id,
    text: &mut String,
    hint: &str,
    width: f32,
    height: f32,
) -> (egui::Response, bool) {
    let (_, rect) = ui.allocate_space(vec2(width, height));
    // The pill's own id, so the bar's checks can find its full extent; the
    // field inside reports only the words' part of it. Only a click: the
    // field is the one stop Tab makes here, not a nameless pill around it.
    let back = ui.interact(rect, pill_id(), Sense::CLICK);
    let has_focus = ui.memory(|memory| memory.has_focus(id));
    let hovered = back.hovered() || ui.rect_contains_pointer(rect);
    let radius = height / 2.0;
    let fill = if has_focus || hovered {
        palette.surface_hover
    } else {
        palette.surface
    };
    ui.painter().rect_filled(rect, radius, fill);
    if has_focus {
        ui.painter().rect_stroke(
            rect,
            radius,
            egui::Stroke::new(2.0, palette.text),
            egui::StrokeKind::Inside,
        );
    } else if hovered {
        ui.painter().rect_stroke(
            rect,
            radius,
            egui::Stroke::new(1.0, palette.text.gamma_multiply(0.1)),
            egui::StrokeKind::Inside,
        );
    }
    let icon_color = if has_focus || hovered {
        palette.text
    } else {
        palette.secondary
    };
    let icon_rect = egui::Rect::from_center_size(
        pos2(rect.left() + height / 2.0, rect.center().y),
        Vec2::splat(24.0),
    );
    Icon::Search.image(icon_color, 24.0).paint_at(ui, icon_rect);
    // Too narrow a pill drops the browse button before the field shrinks
    // past reading size.
    let browse = width >= PILL_BROWSE + 160.0;
    let end = if browse {
        rect.right() - PILL_BROWSE
    } else {
        rect.right() - 12.0
    };
    let clear_width = if text.is_empty() { 0.0 } else { 32.0 };
    let field_rect = egui::Rect::from_min_max(
        pos2(rect.left() + height - 2.0, rect.top() + 1.0),
        pos2(
            (end - clear_width).max(rect.left() + height),
            rect.bottom() - 1.0,
        ),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(field_rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    // A right-to-left query is shown in reading order. The glyphs stay in
    // the buffer's order, flagged by direction, so the caret follows them.
    let text_color = palette.text;
    let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, _wrap_width: f32| {
        let mut galley = ui
            .painter()
            .layout_job(egui::text::LayoutJob::simple_singleline(
                buffer.as_str().to_owned(),
                theme::regular(16.0),
                text_color,
            ));
        crate::bidi::reorder(&mut galley);
        galley
    };
    let response = super::widgets::text_edit(
        &mut child,
        locale,
        egui::TextEdit::singleline(text)
            .id(id)
            .hint_text(egui::RichText::new(hint).color(palette.secondary))
            .font(theme::regular(16.0))
            .text_color(palette.text)
            .frame(egui::Frame::NONE)
            .desired_width(field_rect.width())
            .vertical_align(Align::Center)
            .layouter(&mut layouter),
    );
    ui.ctx()
        .accesskit_node_builder(response.id, |node| node.set_label(hint));
    // A click on the pill around the words still puts the caret there.
    if back.clicked() {
        response.request_focus();
    }
    if !text.is_empty() {
        let clear_rect = egui::Rect::from_center_size(
            pos2(end - clear_width / 2.0, rect.center().y),
            Vec2::splat(16.0 + theme::ICON_BUTTON_PAD),
        );
        let mut clear = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(clear_rect)
                .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
        );
        if theme::icon_button(
            &mut clear,
            Icon::X,
            16.0,
            palette.secondary,
            palette.text,
            &gettext(locale, "Clear"),
        )
        .clicked()
        {
            text.clear();
            ui.memory_mut(|memory| memory.request_focus(id));
        }
    }
    let mut browsed = false;
    if browse {
        let divider_x = rect.right() - PILL_BROWSE;
        ui.painter().vline(
            divider_x,
            (rect.center().y - 12.0)..=(rect.center().y + 12.0),
            egui::Stroke::new(1.0, palette.dim),
        );
        let browse_rect = egui::Rect::from_center_size(
            pos2(divider_x + PILL_BROWSE / 2.0, rect.center().y),
            Vec2::splat(24.0 + theme::ICON_BUTTON_PAD),
        );
        let mut cell = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(browse_rect)
                .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
        );
        browsed = theme::icon_button(
            &mut cell,
            Icon::LayoutGrid,
            22.0,
            palette.secondary,
            palette.text,
            &gettext(locale, "Search"),
        )
        .clicked();
    }
    (response, browsed)
}

/// Where the Home disc and the search pill start: centred on the whole bar
/// as Spotify's are, but never over what is left of them (`least`) or what
/// is right of them (`most`, the furthest they may start).
fn centred_start(bar_center: f32, group: f32, least: f32, most: f32) -> f32 {
    (bar_center - group / 2.0).min(most).max(least)
}

/// The bar's height across a window `width` wide: its controls with the
/// panel gap under them, the room macOS's traffic lights take above, and
/// the caption row when the window is too narrow to share it.
pub fn height(ctx: &egui::Context, width: f32) -> f32 {
    super::window_controls_reservation(ctx, width).top + content_height(ctx)
}

/// The bar's own row: Spotify's 48-point pill with 8 points above and
/// below it, which is also the gap down to the panels.
fn content_height(ctx: &egui::Context) -> f32 {
    theme::top_bar_height() + super::PANEL_GAP + theme::titlebar_inset(ctx)
}

/// Spotify's global navigation bar across the top of the window: the menu,
/// Back and Forward, Home and the search pill in the middle, then the bell
/// and the account, beside the window's own buttons.
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let locale = app.locale;
    let width = ui.available_width();
    let bar_left = ui.cursor().min.x;
    let window_controls = super::window_controls_reservation(ui.ctx(), width);
    // Where the titlebar used to be: the bar grows upwards into that space and
    // its empty parts drag the window.
    let content_height = content_height(ui.ctx());
    if fastframe_titlebar::enabled() {
        fastframe_titlebar::drag(
            ui,
            egui::Rect::from_min_size(
                ui.cursor().min,
                vec2(width, content_height + window_controls.top),
            ),
        );
    }
    // Balance the visible Windows header, including the gap below the
    // control row.
    let top_padding = if cfg!(windows) && fastframe_titlebar::enabled() {
        4.0
    } else {
        0.0
    };
    // The Home disc and the pill fill the bar's height, up to Spotify's 48.
    let pill_height = (theme::top_bar_height() - top_padding - 8.0).clamp(32.0, PILL_HEIGHT);
    ui.add_space(window_controls.top + top_padding);
    ui.allocate_ui_with_layout(
        vec2(width, content_height - top_padding),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.add_space(window_controls.left + super::widgets::PAGE_PADDING);
            ui.spacing_mut().item_spacing.x = ITEM_SPACING;
            if !app.settings.sidebar_visible {
                if nav_button(
                    ui,
                    &palette,
                    Icon::PanelLeft,
                    true,
                    super::keys::platform_shortcut(
                        &gettext(locale, "Show sidebar (Ctrl+B)"),
                        &gettext(locale, "Show sidebar (Cmd+B)"),
                    ),
                )
                .clicked()
                {
                    app.actions.push(Action::ToggleSidebar);
                }
                ui.add_space(2.0);
            }
            app_menu(app, ui);
            if nav_button(
                ui,
                &palette,
                Icon::ChevronLeft,
                app.can_go_back(),
                &gettext(locale, "Back"),
            )
            .clicked()
            {
                app.actions.push(Action::Back);
            }
            if nav_button(
                ui,
                &palette,
                Icon::ChevronRight,
                app.can_go_forward(),
                &gettext(locale, "Forward"),
            )
            .clicked()
            {
                app.actions.push(Action::Forward);
            }
            ui.add_space(4.0);

            // The badge sits at the right end but grows with its text, so
            // measure it here, before the search field takes its share.
            let update = app.update.clone();
            let update_galley = update.as_ref().map(|update| {
                let label = match &app.update_download {
                    crate::updates::DownloadState::Ready(_) => {
                        gettext(locale, "Update ready").into_owned()
                    }
                    crate::updates::DownloadState::Downloading { .. } => {
                        gettext(locale, "Downloading update…").into_owned()
                    }
                    _ => {
                        // Translators: {version} is a version number such as 1.2.0.
                        gettext(locale, "Update to {version}").replace("{version}", &update.version)
                    }
                };
                ui.painter()
                    .layout_no_wrap(label, theme::medium(12.5), palette.accent)
            });
            // Ask once, so the bar reserves room for exactly the spinner it
            // then draws.
            let activity = app.background_activity();
            let busy = activity.is_some();
            let badges =
                |labels: bool| badge_width(update_galley.as_ref(), UPDATE_BADGE_PADDING, labels);
            let controls = RIGHT_CONTROLS_WIDTH
                + if !app.network_reachable {
                    ICON_BUTTON_SIZE + 4.0
                } else {
                    0.0
                }
                + if busy {
                    SPINNER_SIZE + ITEM_SPACING
                } else {
                    0.0
                };

            // Home comes before the field, so the field divides what is left.
            let search_room = (ui.available_width() - window_controls.right - HOME_WIDTH).max(0.0);
            // What sits before the field: the page padding, the navigation
            // buttons, Home, and any window buttons. The panels beside the
            // page keep room for it (`least_width`).
            let lead = width - ui.available_width() + window_controls.right + HOME_WIDTH;
            ui.ctx().data_mut(|data| data.insert_temp(lead_id(), lead));
            let fit = topbar_fit(search_room, controls, badges(true), badges(false));
            let search_width = fit.search;
            // Centre Home and the pill on the bar, as far as the controls
            // either side allow.
            let group = HOME_WIDTH + search_width;
            let least = ui.cursor().min.x;
            let most =
                bar_left + width - window_controls.right - controls - badges(fit.labels) - group;
            let start = centred_start(bar_left + width / 2.0, group, least, most.max(least));
            ui.add_space(start - least);
            if home_button(
                ui,
                &palette,
                pill_height,
                matches!(app.page(), Page::Home),
                &gettext(locale, "Home"),
            )
            .clicked()
            {
                app.actions.push(Action::Open(Page::Home));
            }
            let id = egui::Id::new("global-search");
            let before = app.search.query.clone();
            let selected = egui::TextEdit::load_state(ui.ctx(), id)
                .and_then(|state| state.cursor.char_range())
                .is_some_and(|range| !range.is_empty());
            let (response, browsed) = search_pill(
                ui,
                &palette,
                app.locale,
                id,
                &mut app.search.query,
                &gettext(locale, "What do you want to play?"),
                search_width,
                pill_height,
            );
            if browsed && !matches!(app.page(), Page::Search) {
                app.actions.push(Action::Open(Page::Search));
            }
            if app.search.focus_requested {
                app.search.focus_requested = false;
                response.request_focus();
            }
            // Clear empties the field and hands it focus in the same frame.
            // Neither should leave the page: only typing a query does.
            let cleared = app.search.query.is_empty() && !before.is_empty();
            if response.gained_focus() && !cleared && !matches!(app.page(), Page::Search) {
                app.actions.push(Action::Open(Page::Search));
            }
            if app.search.query != before {
                // Typing over a selection, or emptying the field, starts a
                // new query rather than editing the last one.
                if selected || app.search.query.is_empty() {
                    app.actions.push(Action::EndSearchRun);
                }
                app.search.typed_at = Some(std::time::Instant::now());
                if !cleared && !matches!(app.page(), Page::Search) {
                    app.actions.push(Action::Open(Page::Search));
                }
            }
            if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                let query = app.search.query.clone();
                app.actions.push(Action::Search(query));
            }
            if response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                response.surrender_focus();
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(window_controls.right);
                ui.add_space(super::widgets::PAGE_PADDING);
                // Account.
                let (name, avatar) = app
                    .user
                    .as_ref()
                    .map(|user| {
                        (
                            user.name().to_string(),
                            pick_image(&user.images, app.art_px(AVATAR_IMAGE)).map(str::to_string),
                        )
                    })
                    .unwrap_or_default();
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::splat(AVATAR_SIZE), Sense::click());
                let account_label = if name.is_empty() {
                    gettext(locale, "Account")
                } else {
                    name.as_str().into()
                };
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        ui.is_enabled(),
                        &account_label,
                    )
                });
                if ui.is_rect_visible(rect) {
                    let fill = if response.hovered() || response.has_focus() {
                        palette.surface_hover
                    } else {
                        palette.surface
                    };
                    ui.painter()
                        .circle_filled(rect.center(), AVATAR_SIZE / 2.0, fill);
                    let inner =
                        egui::Rect::from_center_size(rect.center(), Vec2::splat(AVATAR_IMAGE));
                    match avatar.as_deref() {
                        Some(url) => super::widgets::paint_cover(
                            ui,
                            &palette,
                            Some(url),
                            inner,
                            AVATAR_IMAGE / 2.0,
                            Icon::User,
                            Some(app.backend.art()),
                        ),
                        None => {
                            let initial = name
                                .chars()
                                .next()
                                .unwrap_or('?')
                                .to_uppercase()
                                .to_string();
                            ui.painter().circle_filled(
                                inner.center(),
                                AVATAR_IMAGE / 2.0,
                                palette.accent,
                            );
                            ui.painter().text(
                                inner.center(),
                                egui::Align2::CENTER_CENTER,
                                initial,
                                theme::bold(14.0),
                                palette.on_accent,
                            );
                        }
                    }
                }
                theme::focus_ring(ui, &response);
                let response = response.on_hover_text(&name);
                egui::Popup::menu(&response)
                    .frame(super::widgets::menu_frame(&palette))
                    .align(egui::RectAlign::BOTTOM_END)
                    .show(|ui| {
                        ui.set_width(200.0);
                        ui.add_space(4.0);
                        // The plan sits right under the name, as one block:
                        // a row of its own each would put a control's height
                        // between them (#1038).
                        ui.horizontal(|ui| {
                            ui.add_space(10.0);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                theme::text(ui, &name, theme::semibold(14.0), palette.text);
                                if let Some(product) =
                                    app.user.as_ref().and_then(|user| user.product.clone())
                                {
                                    theme::text(
                                        ui,
                                        capitalize(&product),
                                        theme::regular(12.0),
                                        palette.secondary,
                                    );
                                }
                            });
                        });
                        super::widgets::menu_separator(ui, &palette);
                        if super::widgets::menu_item(
                            ui,
                            &palette,
                            Some(Icon::TrendingUp),
                            &gettext(locale, "Your stats"),
                        ) {
                            app.actions.push(Action::Open(Page::Stats));
                        }
                        if super::widgets::menu_item(
                            ui,
                            &palette,
                            Some(Icon::Settings),
                            &gettext(locale, "Settings"),
                        ) {
                            app.actions.push(Action::Open(Page::Settings));
                        }
                        if super::widgets::menu_item(
                            ui,
                            &palette,
                            Some(Icon::Info),
                            &gettext(locale, "Keyboard shortcuts"),
                        ) {
                            app.actions
                                .push(Action::ShowDialog(crate::model::Dialog::Shortcuts));
                        }
                        super::widgets::menu_separator(ui, &palette);
                        if super::widgets::menu_item(
                            ui,
                            &palette,
                            Some(Icon::LogOut),
                            &gettext(locale, "Sign out"),
                        ) {
                            app.actions.push(Action::SignOut);
                        }
                    });
                whats_new_bell(app, ui);
                if !app.network_reachable {
                    ui.add_space(4.0);
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::splat(ICON_BUTTON_SIZE), Sense::hover());
                    if ui.is_rect_visible(rect) {
                        let fill = if response.hovered() {
                            if palette.dark {
                                egui::Color32::from_rgb(38, 110, 205)
                            } else {
                                egui::Color32::from_rgb(66, 133, 244)
                            }
                        } else if palette.dark {
                            egui::Color32::from_rgb(28, 92, 178)
                        } else {
                            egui::Color32::from_rgb(41, 121, 255)
                        };
                        ui.painter().circle_filled(rect.center(), 14.5, fill);
                        theme::paint_icon(
                            ui,
                            Icon::WifiOff,
                            rect,
                            ICON_BUTTON_ICON,
                            egui::Color32::WHITE,
                        );
                    }
                    response.on_hover_text(gettext(
                        locale,
                        "Spotifast is offline. Reconnecting automatically when connection returns.",
                    ));
                }
                // A quiet spinner once the app has been at something in the
                // background for a while, long enough that fast requests
                // never flash it.
                if let Some(activity) = activity {
                    let hint = match activity {
                        BackgroundActivity::Library => gettext(locale, "Updating your library…"),
                        BackgroundActivity::Updates => gettext(locale, "Checking for updates…"),
                        BackgroundActivity::Spotify => gettext(locale, "Waiting for Spotify…"),
                    };
                    theme::spinner(ui, SPINNER_SIZE, palette.secondary)
                        .on_hover_text(hint.as_ref());
                }
                // A newer release. Most people never visit a releases page,
                // so the app says so, quietly, until they do.
                if let (Some(galley), Some(update)) = (update_galley, update)
                    && badge(
                        ui,
                        &palette,
                        Icon::Info,
                        galley,
                        UPDATE_BADGE_PADDING,
                        fit.labels,
                    )
                    .on_hover_text(
                        // Translators: {version} is a version number such as 1.2.0.
                        gettext(locale, "Version {version} is available.")
                            .replace("{version}", &update.version),
                    )
                    .clicked()
                {
                    app.actions.push(Action::ShowUpdate);
                }
            });
        },
    );
}

/// The bell beside the account: it opens What's New, with the count of
/// what arrived since it was last opened on a small accent pill.
fn whats_new_bell(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let locale = app.locale;
    let unread = app.unread_whats_new();
    let open = matches!(app.page(), Page::WhatsNew);
    let tooltip = gettext(locale, "What's new");
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(ICON_BUTTON_SIZE), Sense::click());
    response.widget_info(|| {
        let label = if unread == 0 {
            tooltip.clone().into_owned()
        } else {
            let count = u32::try_from(unread).unwrap_or(u32::MAX);
            crate::i18n::ngettext(
                locale,
                // Translators: Keep {count} exactly as written. It becomes a number of new items.
                "What's new, {count} new item",
                "What's new, {count} new items",
                count,
            )
            .replace("{count}", &count.to_string())
        };
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if ui.is_rect_visible(rect) {
        let color = if response.hovered() || response.has_focus() || open {
            palette.text
        } else {
            palette.secondary
        };
        let painter = ui.painter();
        theme::paint_bell(painter, rect.center(), ICON_BUTTON_ICON, color);
        if unread > 0 {
            theme::paint_count_pill(painter, &palette, rect.center() + vec2(7.0, -8.0), unread);
        }
    }
    theme::focus_ring(ui, &response);
    if response.on_hover_text(tooltip.as_ref()).clicked() {
        app.actions.push(Action::Open(Page::WhatsNew));
    }
}

/// Spotify's "…" menu at the bar's left end: the app's own commands, each
/// still reachable from its shortcut and from where it lived before.
fn app_menu(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let locale = app.locale;
    let response = nav_button(ui, &palette, Icon::Ellipsis, true, &gettext(locale, "More"));
    egui::Popup::menu(&response)
        .frame(super::widgets::menu_frame(&palette))
        .align(egui::RectAlign::BOTTOM_START)
        .show(|ui| {
            ui.set_width(220.0);
            let sidebar = if app.settings.sidebar_visible {
                super::keys::platform_shortcut(
                    &gettext(locale, "Hide sidebar (Ctrl+B)"),
                    &gettext(locale, "Hide sidebar (Cmd+B)"),
                )
                .to_owned()
            } else {
                super::keys::platform_shortcut(
                    &gettext(locale, "Show sidebar (Ctrl+B)"),
                    &gettext(locale, "Show sidebar (Cmd+B)"),
                )
                .to_owned()
            };
            if super::widgets::menu_item(ui, &palette, Some(Icon::PanelLeft), &sidebar) {
                app.actions.push(Action::ToggleSidebar);
            }
            if super::widgets::menu_item(
                ui,
                &palette,
                Some(Icon::Sparkles),
                &gettext(locale, "What's new"),
            ) {
                app.actions.push(Action::Open(Page::WhatsNew));
            }
            super::widgets::menu_separator(ui, &palette);
            if super::widgets::menu_item(
                ui,
                &palette,
                Some(Icon::TrendingUp),
                &gettext(locale, "Your stats"),
            ) {
                app.actions.push(Action::Open(Page::Stats));
            }
            if super::widgets::menu_item(
                ui,
                &palette,
                Some(Icon::Settings),
                &gettext(locale, "Settings"),
            ) {
                app.actions.push(Action::Open(Page::Settings));
            }
            if super::widgets::menu_item(
                ui,
                &palette,
                Some(Icon::Info),
                &gettext(locale, "Keyboard shortcuts"),
            ) {
                app.actions
                    .push(Action::ShowDialog(crate::model::Dialog::Shortcuts));
            }
        });
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod topbar_fit_tests {
    use super::*;

    // What badges measure on a bar showing "Update to 0.7.1", and a longer
    // one such as "Downloading update…", each including the spacing before
    // it. The device now has the player bar's strip, not a badge.
    const DEVICE: f32 = ITEM_SPACING + 176.0;
    const UPDATE: f32 = ITEM_SPACING + 152.0;
    // Collapsed, a badge is a square chip as tall as its text.
    const CHIP: f32 = ITEM_SPACING + 15.0 + BADGE_PADDING_Y;

    /// The narrowest bar the app can produce: a 760 px window, its sidebar,
    /// the navigation buttons and Home all taken out. The "…" menu and Home
    /// take 84 px more than the two buttons that used to lead the bar.
    const NARROWEST_BAR: f32 = 398.0 - (LEAD_GUESS - 112.0);

    fn right_end(room: f32, labelled: f32, icons: f32) -> f32 {
        let fit = topbar_fit(room, RIGHT_CONTROLS_WIDTH, labelled, icons);
        let badges = if fit.labels { labelled } else { icons };
        RIGHT_CONTROLS_WIDTH + badges - (room - fit.search)
    }

    #[test]
    fn a_wide_bar_keeps_the_field_it_always_had() {
        let fit = topbar_fit(2000.0, RIGHT_CONTROLS_WIDTH, DEVICE + UPDATE, CHIP * 2.0);
        assert_eq!(fit.search, SEARCH_MAX);
        assert!(fit.labels);
        // Half the room, as before, while half still fits.
        let fit = topbar_fit(700.0, RIGHT_CONTROLS_WIDTH, 0.0, 0.0);
        assert_eq!(fit.search, 350.0);
    }

    #[test]
    fn the_right_end_never_reaches_over_the_search_field() {
        let mut room = NARROWEST_BAR;
        while room <= 2400.0 {
            for (labelled, icons) in [
                (0.0, 0.0),
                (DEVICE, CHIP),
                (UPDATE, CHIP),
                (DEVICE + UPDATE, CHIP * 2.0),
            ] {
                let over = right_end(room, labelled, icons);
                assert!(
                    over <= 0.0,
                    "badges overlap the field by {over} px on a {room} px bar"
                );
            }
            room += 1.0;
        }
    }

    #[test]
    fn a_right_panel_can_narrow_search_after_the_badges_collapse() {
        let room = RIGHT_CONTROLS_WIDTH + CHIP * 2.0 + 100.0;
        let fit = topbar_fit(room, RIGHT_CONTROLS_WIDTH, DEVICE + UPDATE, CHIP * 2.0);
        assert!(!fit.labels);
        assert_eq!(fit.search, 100.0);
        assert_eq!(right_end(room, DEVICE + UPDATE, CHIP * 2.0), 0.0);
    }

    #[test]
    fn a_narrow_bar_trades_the_badge_labels_for_their_icons() {
        assert!(topbar_fit(1400.0, RIGHT_CONTROLS_WIDTH, DEVICE, CHIP).labels);
        // The 1080 px window of the report that started this.
        assert!(topbar_fit(952.0, RIGHT_CONTROLS_WIDTH, DEVICE + UPDATE, CHIP * 2.0).labels);
        assert!(!topbar_fit(NARROWEST_BAR, RIGHT_CONTROLS_WIDTH, DEVICE, CHIP).labels);
    }

    /// At the least width the panels leave it, the bar still holds the
    /// spinner and the badge beside the narrowest field (#624).
    #[test]
    fn the_least_width_holds_every_control_beside_the_field() {
        let lead = LEAD_GUESS;
        let room = least_width_after(lead) - lead;
        let controls = RIGHT_CONTROLS_WIDTH + SPINNER_SIZE + ITEM_SPACING;
        let fit = topbar_fit(room, controls, UPDATE, CHIP);
        assert!(!fit.labels);
        assert_eq!(fit.search, SEARCH_MIN);
        assert!(controls + CHIP + fit.search <= room);
    }

    /// Home and the pill sit in the middle of the bar while the bar has the
    /// room, and slide aside rather than cover what is either side of them.
    #[test]
    fn home_and_search_centre_on_the_bar_without_covering_its_ends() {
        // A wide bar: centred.
        assert_eq!(centred_start(600.0, 400.0, 150.0, 900.0), 400.0);
        // The left controls reach past the centred start: right after them.
        assert_eq!(centred_start(300.0, 400.0, 150.0, 900.0), 150.0);
        // The right controls would be covered: as far right as they allow.
        assert_eq!(centred_start(600.0, 400.0, 150.0, 350.0), 350.0);
        // No room either way: the left end wins, as the field shrinks first.
        assert_eq!(centred_start(600.0, 400.0, 150.0, 150.0), 150.0);
    }

    #[test]
    fn the_field_stays_readable_however_tight_the_bar_gets() {
        let mut room = NARROWEST_BAR;
        while room <= 2400.0 {
            // The update badge is the only one the bar still holds: where
            // playback is moved to the player bar's strip.
            let fit = topbar_fit(room, RIGHT_CONTROLS_WIDTH, UPDATE, CHIP);
            assert!(fit.search >= SEARCH_FLOOR, "field is {} px", fit.search);
            assert!(fit.search <= SEARCH_MAX);
            room += 1.0;
        }
    }
}
