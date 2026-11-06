//! The now-playing bar along the bottom of the window.

use egui::{Align, Color32, Frame, Layout, Margin, Rect, Sense, UiBuilder, Vec2, pos2, vec2};

use crate::app::{App, NowPlaying};
use crate::i18n::gettext;
use crate::model::{Action, DragTrack, Page};
use crate::player::RepeatMode;
use crate::theme::{self, Icon};
use crate::util;

use super::widgets::{SliderEvent, thin_slider};

/// How much of the playing art's tint the bar's fill carries.
const TINT_STRENGTH: f32 = 0.12;
/// How long the bar takes to cross over to a new song's tint.
const TINT_FADE_SECONDS: f32 = 0.45;
const TINT_SESSION_ID: &str = "player-bar-tint-session";
/// The Add to playlist menu a liked song's heart opens.
const LIKED_POPUP_ID: &str = "player-bar-liked-playlists";
/// Forget this bar's animation session while the sign-in screen is shown.
pub(crate) fn end_tint_session(ctx: &egui::Context) {
    ctx.data_mut(|data| data.remove::<u64>(egui::Id::new(TINT_SESSION_ID)));
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let fill = eased_fill(ui.ctx(), palette.panel, app.now_playing_tint());
    egui::Panel::bottom("player-bar")
        .exact_size(theme::PLAYER_BAR_HEIGHT)
        .resizable(false)
        .show_separator_line(false)
        .frame(
            Frame::new()
                .fill(fill)
                .inner_margin(Margin::symmetric(16, 0)),
        )
        .show(ui, |ui| {
            let rect = ui.max_rect();
            let now = app.now_playing();
            ui.painter().hline(
                rect.x_range(),
                rect.top() + 0.5,
                egui::Stroke::new(1.0, palette.outline),
            );
            let width = rect.width();
            let side = (width * 0.3).clamp(200.0, 420.0);
            let cy = rect.center().y;
            let left = Rect::from_min_max(rect.min, pos2(rect.left() + side, rect.bottom()));
            let center = Rect::from_min_max(
                pos2(rect.left() + side, rect.top()),
                pos2(rect.right() - side, rect.bottom()),
            );

            // egui's cross-axis centring is unreliable across nested layouts of
            // mixed heights, so each region is placed in an explicit band that
            // is sized to its content and centred on the bar's midline.
            now_playing_block(app, ui, left, now.as_ref());

            transport(app, ui, now.as_ref(), center);

            let right_band =
                Rect::from_min_size(pos2(rect.right() - side, cy - 15.0), vec2(side, 30.0));
            let mut right_ui = ui.new_child(
                UiBuilder::new()
                    .max_rect(right_band)
                    .layout(Layout::right_to_left(Align::Center)),
            );
            let transport_right = center.center().x + transport_width() / 2.0;
            extras(app, &mut right_ui, now.as_ref(), transport_right);
        });
}

/// Ease the final fill's RGB. Untinted custom panels keep their alpha while
/// the colour returns to the panel colour.
fn eased_fill(ctx: &egui::Context, panel: Color32, tint: Option<Color32>) -> Color32 {
    let target = tint.map_or(panel, |tint| super::blend(panel, tint, TINT_STRENGTH));
    // Color32 stores premultiplied RGB, so interpolate unmultiplied channels.
    let [r, g, b, _] = target.to_srgba_unmultiplied();
    // A new pass after sign-out gets new ids without clearing other animations.
    let pass = ctx.cumulative_pass_nr();
    let session = ctx.data_mut(|data| {
        *data.get_temp_mut_or_insert_with(egui::Id::new(TINT_SESSION_ID), || pass)
    });
    let channel = |axis: &'static str, value: u8| {
        ctx.animate_value_with_time(
            egui::Id::new(("player-bar-tint", session, axis)),
            f32::from(value),
            crate::motion::seconds(ctx, TINT_FADE_SECONDS),
        )
        .round() as u8
    };
    let eased = [channel("r", r), channel("g", g), channel("b", b)];
    if eased == [r, g, b] {
        target
    } else {
        Color32::from_rgba_unmultiplied(eased[0], eased[1], eased[2], target.a())
    }
}

fn now_playing_block(app: &mut App, ui: &mut egui::Ui, region: Rect, now: Option<&NowPlaying>) {
    let palette = app.palette;
    let cy = region.center().y;
    let cover_rect = Rect::from_min_size(pos2(region.left() + 4.0, cy - 28.0), Vec2::splat(56.0));

    let Some(now) = now else {
        super::widgets::paint_cover(ui, &palette, None, cover_rect, 6.0, Icon::Music, None);
        let text_left = cover_rect.right() + 12.0;
        let text_rect = Rect::from_min_size(
            pos2(text_left, cy - 17.0),
            vec2((region.right() - text_left - 8.0).max(40.0), 34.0),
        );
        let mut text_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(text_rect)
                .layout(Layout::top_down(Align::Min)),
        );
        text_ui.spacing_mut().item_spacing.y = 2.0;
        theme::text(
            &mut text_ui,
            gettext(app.locale, "Nothing playing"),
            theme::medium(14.0),
            palette.secondary,
        );
        theme::text(
            &mut text_ui,
            gettext(app.locale, "Pick a song, album, or playlist"),
            theme::regular(12.0),
            palette.dim,
        );
        return;
    };

    super::widgets::playing_cover(
        ui,
        &palette,
        app.backend.art(),
        "player-bar",
        app.small_art(now, cover_rect.width()),
        cover_rect,
        6.0,
    );
    let song = app.now_playing_item();
    let drag_sense = if song.is_some() {
        Sense::click_and_drag()
    } else {
        Sense::click()
    };
    let cover_response = ui
        .interact(cover_rect, egui::Id::new("now-playing-cover"), drag_sense)
        .on_hover_cursor(if song.is_some() {
            egui::CursorIcon::Grab
        } else {
            egui::CursorIcon::Default
        });
    // Hovering the cover offers to dock the art large at the sidebar's
    // bottom, the way Spotify expands it. (#92)
    let art_available = now.art_url.is_some() || now.art_small.is_some();
    let expand_rect = Rect::from_center_size(
        pos2(cover_rect.right() - 10.0, cover_rect.top() + 10.0),
        Vec2::splat(18.0),
    );
    let offer_expand = art_available && !app.settings.art_expanded && app.settings.sidebar_visible;
    let over_expand = offer_expand && ui.rect_contains_pointer(expand_rect);
    if cover_response.clicked() && !over_expand {
        if let Some(id) = &now.album_id {
            app.actions.push(Action::Open(Page::Album(id.clone())));
        } else if let Some(id) = &now.show_id {
            app.actions.push(Action::Open(Page::Show(id.clone())));
        }
    }
    if offer_expand && (cover_response.hovered() || over_expand) {
        let expand = ui.interact(
            expand_rect,
            egui::Id::new("now-playing-art-expand"),
            Sense::click(),
        );
        ui.painter()
            .circle_filled(expand_rect.center(), 9.0, palette.panel.gamma_multiply(0.9));
        Icon::ChevronUp.image(palette.text, 12.0).paint_at(
            ui,
            Rect::from_center_size(expand_rect.center(), Vec2::splat(12.0)),
        );
        if expand.clicked() {
            app.settings.art_expanded = true;
            app.actions.push(Action::SettingsChanged);
        }
    }
    let heart_width = if now.is_episode { 0.0 } else { 42.0 };
    let text_left = cover_rect.right() + 12.0;
    let text_width = (region.right() - text_left - heart_width).max(40.0);
    let displayed_text_width: f32 = {
        let title =
            ui.painter()
                .layout_no_wrap(now.title.clone(), theme::medium(14.0), palette.text);
        let subtitle = ui.painter().layout_no_wrap(
            now.subtitle.clone(),
            theme::regular(12.0),
            palette.secondary,
        );
        title.size().x.max(subtitle.size().x).min(text_width)
    };
    let info_rect = Rect::from_min_size(pos2(text_left, cy - 18.0), vec2(text_width, 36.0));

    let mut text_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(info_rect)
            .layout(Layout::top_down(Align::Min)),
    );
    text_ui.set_clip_rect(info_rect.intersect(ui.clip_rect()));
    text_ui.spacing_mut().item_spacing.y = 2.0;
    let title_response = theme::link(&mut text_ui, &now.title, theme::medium(14.0), palette.text)
        .interact(drag_sense);
    if title_response.clicked() {
        if let Some(id) = &now.album_id {
            app.actions.push(Action::Open(Page::Album(id.clone())));
        } else if let Some(id) = &now.show_id {
            app.actions.push(Action::Open(Page::Show(id.clone())));
        }
    }
    let mut responses = vec![cover_response, title_response];
    text_ui.horizontal_top(|ui| {
        if now.artists.is_empty() {
            let response = theme::link(ui, &now.subtitle, theme::regular(12.0), palette.secondary)
                .interact(drag_sense);
            if response.clicked()
                && let Some(id) = &now.show_id
            {
                app.actions.push(Action::Open(Page::Show(id.clone())));
            }
            responses.push(response);
        } else {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (index, artist) in now.artists.iter().enumerate() {
                if index > 0 {
                    theme::text(ui, ", ", theme::regular(12.0), palette.secondary);
                }
                let response = if artist.id.is_some() {
                    theme::link(ui, &artist.name, theme::regular(12.0), palette.secondary)
                } else {
                    theme::text(ui, &artist.name, theme::regular(12.0), palette.secondary)
                }
                .interact(drag_sense);
                if response.clicked()
                    && let Some(id) = &artist.id
                {
                    app.actions.push(Action::Open(Page::Artist(id.clone())));
                }
                responses.push(response);
            }
        }
    });
    if responses
        .iter()
        .any(|response| response.drag_started_by(egui::PointerButton::Primary))
        && let Some(item) = &song
    {
        egui::DragAndDrop::set_payload(
            ui.ctx(),
            DragTrack {
                title: item.name().to_string(),
                image: app.item_image(item, 64),
                items: vec![item.clone()],
                from: None,
                source_playlist: None,
            },
        );
    }

    // The playing thing answers the same right-click menu as a table row,
    // from the cover or the words.
    if let Some(item) = &song {
        let context = app.editable_context_playlist();
        for response in &responses {
            egui::Popup::context_menu(response)
                .frame(super::widgets::menu_frame(&palette))
                .show(|ui| super::widgets::item_menu(ui, app, item, context.as_ref(), None));
        }
    }

    if !now.is_episode {
        let saved = app.is_saved(&now.uri).unwrap_or(false);
        // A liked heart stays green under the pointer, like shuffle.
        let (icon, color, hover, tooltip) = if saved {
            (
                Icon::HeartFilled,
                palette.accent,
                palette.accent_hover,
                gettext(app.locale, "Add to playlist"),
            )
        } else {
            (
                Icon::Heart,
                palette.secondary,
                palette.text,
                gettext(app.locale, "Save to Liked Songs"),
            )
        };
        // Sit the heart just past the actual text, not at the region's far
        // edge, so it stays visually attached to the title.
        let heart_x = (text_left + displayed_text_width + 21.0).min(region.right() - 21.0);
        let heart_rect = Rect::from_center_size(pos2(heart_x, cy), Vec2::splat(30.0));
        let mut heart_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(heart_rect)
                .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
        );
        let response = theme::icon_button(&mut heart_ui, icon, 17.0, color, hover, &tooltip);
        let popup_id = egui::Id::new(LIKED_POPUP_ID);
        // A liked song's heart opens Add to playlist, as Spotify's does,
        // with Remove from Liked Songs below the playlists (#792). The first
        // click on a song not yet liked still likes it at once.
        match song.as_ref().filter(|_| saved) {
            None => {
                egui::Popup::close_id(ui.ctx(), popup_id);
                if response.clicked() {
                    app.actions.push(Action::ToggleSaved(now.uri.clone()));
                }
            }
            Some(item) => {
                let query_id = popup_id.with("query");
                let shown = egui::Popup::menu(&response)
                    .id(popup_id)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .frame(super::widgets::menu_frame(&palette))
                    .show(|ui| {
                        let stored = ui.data(|data| data.get_temp::<String>(query_id));
                        let fresh = stored.is_none();
                        let mut query = stored.unwrap_or_default();
                        let before = app.actions.len();
                        let field = super::widgets::playlist_picker(
                            ui,
                            app,
                            std::slice::from_ref(item),
                            &mut query,
                        );
                        if fresh {
                            field.request_focus();
                        }
                        ui.data_mut(|data| data.insert_temp(query_id, query));
                        super::widgets::menu_separator(ui, &palette);
                        if super::widgets::menu_item(
                            ui,
                            &palette,
                            Some(Icon::HeartFilled),
                            &gettext(app.locale, "Remove from Liked Songs"),
                        ) {
                            app.actions.push(Action::ToggleSaved(now.uri.clone()));
                        }
                        // A playlist, New playlist, or Remove closes it.
                        if app.actions.len() > before {
                            ui.close();
                        }
                    });
                if shown.is_none() {
                    ui.data_mut(|data| data.remove::<String>(query_id));
                }
            }
        }
    }
}

/// The transport's button widths: icon buttons occupy icon size + 12; the
/// disc is 36.
const TRANSPORT_WIDTHS: [f32; 5] = [29.0, 30.0, 36.0, 30.0, 29.0];
const TRANSPORT_GAP: f32 = 10.0;

/// The transport's buttons from Shuffle to Repeat, centred in the bar.
fn transport_width() -> f32 {
    TRANSPORT_WIDTHS.iter().sum::<f32>() + TRANSPORT_GAP * 4.0
}

fn transport(app: &mut App, ui: &mut egui::Ui, now: Option<&NowPlaying>, region: Rect) {
    let palette = app.palette;
    // Everything here is placed with explicit rects: egui's implicit rows
    // centre each widget in the row height known when it is added, which
    // left earlier icons riding high next to the play disc.
    //
    // The buttons row (36) and the progress row (~15, after a 6px gap) form
    // one cluster, centred as a group in the 88px bar: the buttons sit 8px
    // above the bar's midline and the progress row 23px below it. Measured
    // on screen this puts equal breathing room above and beneath the
    // cluster.
    let cy = region.center().y - 8.0;
    let enabled = now.is_some_and(|now| now.can_control) || app.is_connected();
    let playing = now.is_some_and(|now| now.playing);
    let loading = now.is_some_and(|now| now.loading);
    let shuffle = now.map_or_else(|| app.playing_context_shuffle(), |now| now.shuffle);
    let repeat = now.map(|now| now.repeat).unwrap_or_default();
    let dim = if enabled {
        palette.secondary
    } else {
        palette.dim
    };

    let widths = TRANSPORT_WIDTHS;
    let mut x = region.center().x - transport_width() / 2.0;
    let mut slot = |width: f32| {
        let rect = Rect::from_center_size(pos2(x + width / 2.0, cy), vec2(width, 36.0));
        x += width + TRANSPORT_GAP;
        rect
    };
    let centered = |ui: &mut egui::Ui, rect: Rect| {
        ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
        )
    };

    let shuffle_color = if shuffle { palette.accent } else { dim };
    let mut cell = centered(ui, slot(widths[0]));
    let shuffle_button = theme::icon_button(
        &mut cell,
        Icon::Shuffle,
        17.0,
        shuffle_color,
        if shuffle {
            palette.accent_hover
        } else {
            palette.text
        },
        &gettext(app.locale, "Shuffle"),
    );
    shuffle_button.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            cell.is_enabled(),
            shuffle,
            gettext(app.locale, "Shuffle"),
        )
    });
    if shuffle_button.clicked() {
        app.actions.push(Action::ToggleShuffle);
    }

    let mut cell = centered(ui, slot(widths[1]));
    if theme::icon_button(
        &mut cell,
        Icon::SkipBackFilled,
        18.0,
        dim,
        palette.text,
        &gettext(app.locale, "Previous"),
    )
    .clicked()
    {
        app.actions.push(Action::Previous);
    }

    let disc = slot(widths[2]);
    if loading || app.any_play_pending() {
        let mut cell = centered(ui, disc);
        theme::circle_spinner(
            &mut cell,
            36.0,
            palette.text,
            palette.window,
            &gettext(app.locale, "Loading…"),
        );
    } else {
        let icon = if playing {
            Icon::PauseFilled
        } else {
            Icon::PlayFilled
        };
        let hover = if palette.dark {
            egui::Color32::WHITE
        } else {
            palette.text
        };
        let mut cell = centered(ui, disc);
        if theme::circle_button(
            &mut cell,
            icon,
            36.0,
            palette.text,
            hover,
            palette.window,
            &if playing {
                gettext(app.locale, "Pause")
            } else {
                gettext(app.locale, "Play")
            },
        )
        .clicked()
        {
            app.actions.push(Action::TogglePlay);
        }
    }

    let mut cell = centered(ui, slot(widths[3]));
    if theme::icon_button(
        &mut cell,
        Icon::SkipForwardFilled,
        18.0,
        dim,
        palette.text,
        &gettext(app.locale, "Next"),
    )
    .clicked()
    {
        app.actions.push(Action::Next);
    }

    let (repeat_icon, repeat_color, tooltip) = match repeat {
        RepeatMode::Off => (Icon::Repeat, dim, gettext(app.locale, "Repeat")),
        RepeatMode::Context => (
            Icon::Repeat,
            palette.accent,
            gettext(app.locale, "Repeat one"),
        ),
        RepeatMode::Track => (
            Icon::Repeat1,
            palette.accent,
            gettext(app.locale, "Repeat off"),
        ),
    };
    let mut cell = centered(ui, slot(widths[4]));
    if theme::icon_button(
        &mut cell,
        repeat_icon,
        17.0,
        repeat_color,
        if repeat == RepeatMode::Off {
            palette.text
        } else {
            palette.accent_hover
        },
        &tooltip,
    )
    .clicked()
    {
        app.actions.push(Action::CycleRepeat);
    }

    // Progress row, just below the buttons (disc bottom + 6px gap + half of
    // the time text's line height).
    let row_cy = cy + 31.0;
    let slider_width = (region.width() - 120.0).clamp(120.0, 620.0);
    let (position, duration) = now
        .map(|now| (now.position_ms, now.duration_ms))
        .unwrap_or((0, 0));
    let shown_position = match app.seek_preview {
        Some(fraction) => (fraction * duration as f32) as u32,
        None => position,
    };
    let time_color = if now.is_some() {
        palette.secondary
    } else {
        palette.dim
    };
    let slider_left = region.center().x - slider_width / 2.0;
    ui.painter().text(
        pos2(slider_left - 8.0, row_cy),
        egui::Align2::RIGHT_CENTER,
        util::format_duration_ms(shown_position),
        theme::regular(11.5),
        time_color,
    );
    let slider_rect =
        Rect::from_center_size(pos2(region.center().x, row_cy), vec2(slider_width, 16.0));
    let mut slider_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(slider_rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    let fraction = if duration > 0 {
        position as f32 / duration as f32
    } else {
        0.0
    };
    match thin_slider(
        &mut slider_ui,
        &palette,
        egui::Id::new("seek-slider"),
        &gettext(app.locale, "Playback position (%)"),
        fraction,
        slider_width,
        None,
    ) {
        SliderEvent::Dragging(value) => app.seek_preview = Some(value),
        SliderEvent::Committed(value) => {
            app.seek_preview = None;
            if duration > 0 {
                app.actions
                    .push(Action::Seek((value * duration as f32) as u32));
            }
        }
        SliderEvent::None => {}
    }
    ui.painter().text(
        pos2(slider_left + slider_width + 8.0, row_cy),
        egui::Align2::LEFT_CENTER,
        util::format_duration_ms(duration),
        theme::regular(11.5),
        time_color,
    );
}

/// What the right of the bar always holds, in points: the volume slider and
/// its button, the devices, queue and lyrics buttons, and the gaps.
const EXTRAS_WIDTH: f32 = 92.0 + 4.0 * (18.0 + theme::ICON_BUTTON_PAD) + 4.0 * 6.0 + 4.0;

/// Whether the full screen button has room beside the rest, keeping clear of
/// the transport, with `room` points to the right of it.
fn full_screen_button_fits(room: f32) -> bool {
    room >= EXTRAS_WIDTH + 18.0 + theme::ICON_BUTTON_PAD + 6.0 + TRANSPORT_GAP
}

fn extras(app: &mut App, ui: &mut egui::Ui, now: Option<&NowPlaying>, transport_right: f32) {
    let palette = app.palette;
    ui.spacing_mut().item_spacing.x = 6.0;
    let volume = now
        .map(|now| now.volume_percent)
        .unwrap_or_else(|| crate::app::volume_to_percent(app.local.volume));
    let shown = match app.volume_preview {
        Some(fraction) => (fraction * 100.0).round() as u8,
        None => volume,
    };
    // Some remote devices, such as a phone playing over Bluetooth, refuse
    // volume changes from other apps: show their volume but don't offer to
    // change it.
    let adjustable = !app.soloist_waiting() && now.is_none_or(|now| now.can_set_volume);
    // The bar's last control, right of the volume, like Spotify's. It gives
    // way before anything else, once Listening activity has; **F** stays.
    let room = ui.max_rect().right() - transport_right;
    if full_screen_button_fits(room) {
        let inside = app.lyrics_fullscreen.is_some();
        let (icon, tip) = if inside {
            (Icon::Shrink, gettext(app.locale, "Leave full screen (Esc)"))
        } else {
            (Icon::Expand, gettext(app.locale, "Full screen lyrics"))
        };
        if theme::icon_button(ui, icon, 18.0, palette.secondary, palette.text, &tip).clicked() {
            app.actions.push(Action::SetLyricsFullscreen(!inside));
        }
    }
    let controls = ui.add_enabled_ui(adjustable, |ui| {
        match thin_slider(
            ui,
            &palette,
            egui::Id::new("volume-slider"),
            &gettext(app.locale, "Volume (%)"),
            shown as f32 / 100.0,
            92.0,
            Some(0.05),
        ) {
            SliderEvent::Dragging(value) => {
                app.volume_preview = Some(value);
                app.actions
                    .push(Action::PreviewVolume((value * 100.0).round() as u8));
            }
            SliderEvent::Committed(value) => {
                app.volume_preview = None;
                app.actions
                    .push(Action::SetVolume((value * 100.0).round() as u8));
            }
            SliderEvent::None => {}
        }
        let volume_icon = match shown {
            0 => Icon::VolumeX,
            1..=33 => Icon::Volume,
            34..=66 => Icon::Volume1,
            _ => Icon::Volume2,
        };
        if theme::icon_button(
            ui,
            volume_icon,
            18.0,
            palette.secondary,
            palette.text,
            &if shown == 0 {
                gettext(app.locale, "Unmute")
            } else {
                gettext(app.locale, "Mute")
            },
        )
        .clicked()
        {
            app.actions.push(Action::ToggleMute);
        }
    });
    if !adjustable {
        ui.interact(
            controls.response.rect,
            egui::Id::new("volume-fixed"),
            egui::Sense::hover(),
        )
        .on_hover_text(gettext(
            app.locale,
            "This device's volume can't be changed from Spotifast",
        ));
    }
    ui.add_space(4.0);
    let remote = now.is_some_and(|now| !now.local) && !app.targets_own_soloist();
    // Playing elsewhere, the button shows that device's own icon.
    let icon = now
        .filter(|_| remote)
        .and_then(|now| now.device_kind.as_deref())
        .map_or(Icon::Speaker, super::devices::device_icon);
    let devices = theme::icon_button(
        ui,
        icon,
        18.0,
        if remote {
            palette.accent
        } else {
            palette.secondary
        },
        palette.text,
        &gettext(app.locale, "Connect to a device"),
    );
    ui.ctx().data_mut(|data| {
        data.insert_temp(egui::Id::new(super::devices::BUTTON_RECT_ID), devices.rect)
    });
    if devices.clicked() {
        app.actions.push(Action::ToggleDevicesPopup);
    }
    let queue_open = app.show_queue_panel || matches!(app.page(), Page::Queue);
    let queue_button = theme::icon_button(
        ui,
        Icon::ListVideo,
        18.0,
        if queue_open {
            palette.accent
        } else {
            palette.secondary
        },
        palette.text,
        &gettext(app.locale, "Queue"),
    );
    if queue_button.clicked() {
        app.actions.push(Action::ToggleQueuePanel);
    }
    // A dragged song dropped on the queue button queues it, same as the
    // "Add to queue" menu item.
    if let Some(track) = queue_button.dnd_release_payload::<DragTrack>() {
        app.actions.push(Action::QueueMany {
            songs: track
                .items
                .iter()
                .map(|item| (item.uri().to_string(), item.name().to_string()))
                .collect(),
        });
    }
    if theme::icon_button(
        ui,
        Icon::Mic,
        18.0,
        if app.show_lyrics_panel {
            palette.accent
        } else {
            palette.secondary
        },
        palette.text,
        &gettext(app.locale, "Lyrics"),
    )
    .clicked()
    {
        app.actions.push(Action::ToggleLyricsPanel);
    }
    // Spotify's "Now playing view" button sits left of Lyrics. It gives way
    // before the transport does, as Listening activity does.
    let left = ui.min_rect().left() - ui.spacing().item_spacing.x - (18.0 + theme::ICON_BUTTON_PAD);
    if left - transport_right >= TRANSPORT_GAP
        && theme::icon_button(
            ui,
            Icon::CirclePlay,
            18.0,
            if app.now_playing_panel_shown() {
                palette.accent
            } else {
                palette.secondary
            },
            palette.text,
            &gettext(app.locale, "Now playing view"),
        )
        .clicked()
    {
        app.actions.push(Action::ToggleNowPlayingPanel);
    }
    // The Listening activity button is the first to go in a narrow window,
    // so it never crowds the transport.
    let left = ui.min_rect().left() - ui.spacing().item_spacing.x - (18.0 + theme::ICON_BUTTON_PAD);
    if left - transport_right >= TRANSPORT_GAP
        && theme::icon_button(
            ui,
            Icon::Users,
            18.0,
            if app.show_friends_panel {
                palette.accent
            } else {
                palette.secondary
            },
            palette.text,
            &gettext(app.locale, "Listening activity"),
        )
        .clicked()
    {
        app.actions.push(Action::ToggleFriendsPanel);
    }
}

#[cfg(test)]
mod player_bar_tint_tests {
    use super::*;
    use crate::theme::Palette;

    /// Run one frame at `time` and report the bar's actual fill.
    fn frame(ctx: &egui::Context, time: f64, panel: Color32, tint: Option<Color32>) -> Color32 {
        let mut fill = Color32::PLACEHOLDER;
        let mut output = ctx.run_ui(
            egui::RawInput {
                time: Some(time),
                ..Default::default()
            },
            |ui| fill = eased_fill(ui.ctx(), panel, tint),
        );
        output.textures_delta.clear();
        fill
    }

    #[test]
    fn a_song_without_art_preserves_translucent_panel() {
        for panel in [
            Palette::dark().panel,
            Palette::light().panel,
            Color32::from_rgba_unmultiplied(80, 120, 180, 128),
        ] {
            let ctx = egui::Context::default();
            assert_eq!(frame(&ctx, 0.0, panel, None), panel);
            let art = Color32::from_rgb(200, 40, 90);
            frame(&ctx, 0.1, panel, Some(art));
            frame(&ctx, 0.6, panel, Some(art));
            assert_eq!(frame(&ctx, 0.7, panel, None).a(), panel.a());
            assert_eq!(frame(&ctx, 1.2, panel, None), panel);
        }
    }

    #[test]
    fn the_first_frame_shows_the_tint_without_fading_in() {
        let ctx = egui::Context::default();
        let panel = Palette::dark().panel;
        let art = Color32::from_rgb(200, 40, 90);
        assert_eq!(
            frame(&ctx, 0.0, panel, Some(art)),
            super::super::blend(panel, art, TINT_STRENGTH)
        );
    }

    #[test]
    fn first_art_after_an_untinted_frame_fades_in() {
        let ctx = egui::Context::default();
        let panel = Palette::dark().panel;
        let art = Color32::from_rgb(220, 140, 160);
        let target = super::super::blend(panel, art, TINT_STRENGTH);
        let fade = f64::from(TINT_FADE_SECONDS);

        assert_eq!(frame(&ctx, 0.0, panel, None), panel);
        assert_eq!(frame(&ctx, 0.1, panel, Some(art)), panel);
        let middle = frame(&ctx, 0.1 + fade / 2.0, panel, Some(art));
        assert_ne!(middle, panel);
        assert_ne!(middle, target);
        assert_eq!(frame(&ctx, 0.1 + fade, panel, Some(art)), target);
    }

    #[test]
    fn changing_songs_mid_fade_continues_from_the_visible_colour() {
        let ctx = egui::Context::default();
        let panel = Palette::dark().panel;
        let first = Color32::from_rgb(20, 40, 60);
        let second = Color32::from_rgb(220, 140, 160);
        let third = Color32::from_rgb(40, 230, 30);
        let first_fill = super::super::blend(panel, first, TINT_STRENGTH);
        let second_fill = super::super::blend(panel, second, TINT_STRENGTH);
        let third_fill = super::super::blend(panel, third, TINT_STRENGTH);
        let fade = f64::from(TINT_FADE_SECONDS);

        assert_eq!(frame(&ctx, 0.0, panel, Some(first)), first_fill);
        assert_eq!(frame(&ctx, 0.05, panel, Some(second)), first_fill);
        let middle = frame(&ctx, 0.2, panel, Some(second));
        assert_ne!(middle, first_fill);
        assert_ne!(middle, second_fill);
        assert_eq!(frame(&ctx, 0.2, panel, Some(third)), middle);
        assert_ne!(frame(&ctx, 0.2 + fade / 2.0, panel, Some(third)), middle);
        assert_eq!(frame(&ctx, 0.2 + fade, panel, Some(third)), third_fill);
    }

    #[test]
    fn a_new_session_does_not_reuse_the_previous_tint() {
        let ctx = egui::Context::default();
        let panel = Palette::dark().panel;
        let first = Color32::from_rgb(20, 40, 60);
        let second = Color32::from_rgb(220, 140, 160);
        let next_session = Color32::from_rgb(40, 230, 30);

        frame(&ctx, 0.0, panel, Some(first));
        frame(&ctx, 0.1, panel, Some(second));
        assert_ne!(
            frame(&ctx, 0.2, panel, Some(second)),
            super::super::blend(panel, second, TINT_STRENGTH)
        );

        end_tint_session(&ctx);
        assert_eq!(
            frame(&ctx, 0.21, panel, Some(next_session)),
            super::super::blend(panel, next_session, TINT_STRENGTH)
        );

        end_tint_session(&ctx);
        assert_eq!(frame(&ctx, 0.22, panel, None), panel);
    }

    /// A window too narrow for it loses the button, not the volume.
    #[test]
    fn the_full_screen_button_goes_when_the_bar_is_tight() {
        assert!(full_screen_button_fits(320.0));
        assert!(!full_screen_button_fits(200.0));
    }
}
