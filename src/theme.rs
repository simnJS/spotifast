//! Shared palette, typography, icons, and base widgets.
//!
//! Inter provides real font weights, and Lucide provides a consistent icon set.
//! All colors use [`Palette`] so light, dark, and album-art-tinted themes stay
//! consistent.

use crate::i18n::{Locale, gettext};
use egui::{Color32, CornerRadius, Response, Sense, Stroke, Vec2};
use std::borrow::Cow;

/// A palette file from the themes directory.
pub type CustomTheme = fastframe_theme::CustomTheme<Palette>;

/// The palette Spotifast bundles on top of fastframe-theme's shared ones.
const BUNDLED: &[(&str, &str)] = &[(
    "Spotify.json",
    include_str!("../assets/themes/Spotify.json"),
)];

/// The bundled Spotify palette, for tests and demo shots that draw with it.
#[cfg(any(test, feature = "demo"))]
pub(crate) fn spotify_palette() -> Palette {
    fastframe_theme::parse_palette(BUNDLED[0].1).expect("the bundled Spotify palette")
}

/// Writes each bundled palette into `directory`, creating it when missing,
/// unless something already has its name: a file the user edited, or a link,
/// is theirs and is never touched. Files go through a temporary file and a
/// rename, as fastframe-theme's own presets do.
///
/// # Errors
///
/// When the directory or a palette cannot be written.
pub fn install_bundled(directory: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    for (name, contents) in BUNDLED {
        let path = directory.join(name);
        if std::fs::symlink_metadata(&path).is_ok() {
            continue;
        }
        let temporary = directory.join(format!(".{name}.tmp"));
        let written =
            std::fs::write(&temporary, contents).and_then(|()| std::fs::rename(&temporary, &path));
        if let Err(error) = written {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
    }
    Ok(())
}

/// A scan waiting for the bundled palettes to be written first.
struct Install {
    done: std::sync::mpsc::Receiver<()>,
    directory: std::path::PathBuf,
    selected: Vec<String>,
    waker: fastframe_theme::Waker,
}

/// The palette files, and the Omarchy palette where the desktop has one.
///
/// fastframe-theme's catalogue, with the bundled palettes written into the
/// themes folder, off the UI thread, before each scan lists it.
#[derive(Default)]
pub struct Catalog {
    inner: fastframe_theme::Catalog<Palette>,
    install: Option<Install>,
}

impl std::ops::Deref for Catalog {
    type Target = fastframe_theme::Catalog<Palette>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl std::ops::DerefMut for Catalog {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl Catalog {
    /// A catalogue already holding `themes`, without touching the disk.
    #[must_use]
    pub fn preview(themes: Vec<CustomTheme>, follows_omarchy: bool) -> Self {
        Self {
            inner: fastframe_theme::Catalog::preview(themes, follows_omarchy),
            install: None,
        }
    }

    /// Writes the bundled palettes, then lists `directory` in the
    /// background; both happen off the calling thread. A request made while
    /// the palettes are being written replaces the waiting one.
    pub fn start(
        &mut self,
        directory: std::path::PathBuf,
        selected: impl IntoIterator<Item = String>,
        waker: &fastframe_theme::Waker,
    ) {
        let selected: Vec<String> = selected.into_iter().collect();
        if let Some(install) = &mut self.install {
            install.directory = directory;
            install.selected = selected;
            install.waker = waker.clone();
            return;
        }
        let (sender, done) = std::sync::mpsc::channel();
        let target = directory.clone();
        let wake = waker.clone();
        let spawned = std::thread::Builder::new()
            .name("spotifast-bundled-themes".to_owned())
            .spawn(move || {
                if let Err(error) = install_bundled(&target) {
                    log::warn!("unable to install the bundled palettes: {error}");
                }
                if sender.send(()).is_ok() {
                    wake.wake();
                }
            });
        match spawned {
            Ok(_) => {
                self.install = Some(Install {
                    done,
                    directory,
                    selected,
                    waker: waker.clone(),
                });
            }
            Err(error) => {
                log::warn!("unable to start the bundled palette installer: {error}");
                self.inner.start(directory, selected, waker);
            }
        }
    }

    /// Starts the waiting scan once the bundled palettes are written, and
    /// takes a finished scan's result. Returns `true` when the catalogue
    /// changed.
    pub fn poll(&mut self) -> bool {
        if let Some(install) = &self.install {
            // A closed channel means the writer ended without reporting.
            if matches!(
                install.done.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ) {
                return false;
            }
            if let Some(install) = self.install.take() {
                self.inner
                    .start(install.directory, install.selected, &install.waker);
            }
        }
        self.inner.poll()
    }

    /// Whether the bundled palettes or a scan are still being worked on.
    pub fn loading(&self) -> bool {
        self.install.is_some() || self.inner.loading()
    }

    /// What to say under the theme setting, given the selected filename.
    pub fn status(&self, selected: Option<&str>) -> Option<fastframe_theme::Status> {
        if self.install.is_some() {
            return Some(fastframe_theme::Status::Loading);
        }
        self.inner.status(selected)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Palette {
    pub dark: bool,
    pub window: Color32,
    pub panel: Color32,
    pub surface: Color32,
    pub surface_hover: Color32,
    pub surface_active: Color32,
    pub outline: Color32,
    pub text: Color32,
    pub secondary: Color32,
    pub dim: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub on_accent: Color32,
    pub danger: Color32,
    pub warning: Color32,
    pub overlay: Color32,
    pub shadow: Color32,
    /// The colour a page's header fades from when album art does not tint
    /// it; its alpha is the strength. Transparent leaves the plain window.
    #[serde(default)]
    pub tint: Color32,
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            dark: true,
            window: Color32::from_rgb(0x0f, 0x11, 0x14),
            panel: Color32::from_rgb(0x15, 0x18, 0x1c),
            surface: Color32::from_rgb(0x1d, 0x21, 0x27),
            surface_hover: Color32::from_rgb(0x26, 0x2b, 0x33),
            surface_active: Color32::from_rgb(0x2f, 0x35, 0x3f),
            outline: Color32::from_rgb(0x2a, 0x30, 0x38),
            text: Color32::from_rgb(0xf2, 0xf4, 0xf6),
            secondary: Color32::from_rgb(0xa9, 0xb1, 0xbc),
            dim: Color32::from_rgb(0x6e, 0x77, 0x84),
            accent: Color32::from_rgb(0x1e, 0xd7, 0x60),
            accent_hover: Color32::from_rgb(0x3c, 0xe8, 0x7a),
            on_accent: Color32::from_rgb(0x0a, 0x14, 0x0e),
            danger: Color32::from_rgb(0xf5, 0x71, 0x7f),
            warning: Color32::from_rgb(0xf2, 0xb8, 0x5c),
            overlay: Color32::from_rgb(0x22, 0x27, 0x2e),
            shadow: Color32::from_black_alpha(140),
            tint: Color32::TRANSPARENT,
        }
    }

    pub fn light() -> Self {
        Self {
            dark: false,
            window: Color32::from_rgb(0xf8, 0xf9, 0xfb),
            panel: Color32::from_rgb(0xff, 0xff, 0xff),
            surface: Color32::from_rgb(0xee, 0xf0, 0xf3),
            surface_hover: Color32::from_rgb(0xe3, 0xe6, 0xeb),
            surface_active: Color32::from_rgb(0xd7, 0xdb, 0xe1),
            outline: Color32::from_rgb(0xdd, 0xe1, 0xe6),
            text: Color32::from_rgb(0x14, 0x17, 0x1a),
            secondary: Color32::from_rgb(0x53, 0x5b, 0x66),
            dim: Color32::from_rgb(0x8b, 0x93, 0x9e),
            accent: Color32::from_rgb(0x15, 0xa6, 0x4a),
            accent_hover: Color32::from_rgb(0x12, 0x8f, 0x40),
            on_accent: Color32::WHITE,
            danger: Color32::from_rgb(0xd6, 0x3b, 0x4c),
            warning: Color32::from_rgb(0xb8, 0x7a, 0x14),
            overlay: Color32::from_rgb(0xff, 0xff, 0xff),
            shadow: Color32::from_black_alpha(50),
            tint: Color32::TRANSPARENT,
        }
    }

    /// A colour derived from album art, softened so it can sit behind text.
    pub fn tint_from_art(&self, rgb: [u8; 3]) -> Color32 {
        let [r, g, b] = rgb.map(|c| c as f32 / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let lightness = (max + min) / 2.0;
        let target = if self.dark { 0.30 } else { 0.72 };
        let (r, g, b) = if lightness < 0.01 {
            (target, target, target)
        } else {
            let scale = target / lightness;
            (
                (r * scale).min(1.0),
                (g * scale).min(1.0),
                (b * scale).min(1.0),
            )
        };
        Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
    }
}

impl fastframe_theme::Palette for Palette {
    fn base(base: fastframe_theme::Base) -> Self {
        match base {
            fastframe_theme::Base::Dark => Self::dark(),
            fastframe_theme::Base::Light => Self::light(),
        }
    }

    fn set(&mut self, name: &str, color: Color32) -> bool {
        match name {
            "window" => self.window = color,
            "panel" => self.panel = color,
            "surface" => self.surface = color,
            "surface_hover" => self.surface_hover = color,
            "surface_active" => self.surface_active = color,
            "outline" => self.outline = color,
            "text" => self.text = color,
            "secondary" => self.secondary = color,
            "dim" => self.dim = color,
            "accent" => self.accent = color,
            "accent_hover" => self.accent_hover = color,
            "on_accent" => self.on_accent = color,
            "danger" => self.danger = color,
            "warning" => self.warning = color,
            "overlay" => self.overlay = color,
            "shadow" => self.shadow = color,
            "tint" => self.tint = color,
            _ => return false,
        }
        true
    }

    /// Menus, tooltips and dialogs follow a file's window when it leaves
    /// `overlay` out, as Omarchy's template and the shared palettes do:
    /// otherwise they kept Spotifast's own blue-grey on every other theme.
    /// The proportions are the built-in palettes': a step above the window
    /// toward the text in dark ones, the panel in light ones.
    fn derive(&mut self, given: &std::collections::BTreeSet<&str>) {
        if given.contains("overlay") || !given.contains("window") {
            return;
        }
        self.overlay = if self.dark {
            self.window.lerp_to_gamma(self.text, 0.08)
        } else {
            self.panel
        };
    }
}

impl fastframe_theme::PaletteBase for Palette {
    fn palette_base(&self) -> fastframe_theme::Base {
        fastframe_theme::Base::from_dark(self.dark)
    }
}

/// The palettes Follow system shows in light and dark mode.
pub type ThemePair = fastframe_theme::Pair<Palette>;

/// Adds the desktop's palettes to a normal launch: the eight shared palettes,
/// installed into the themes folder as files on the first launch, and
/// Omarchy's on Linux, with the packaged template and hook installed for the
/// user.
pub fn enable_desktop_themes(catalog: &mut Catalog) {
    catalog.enable_desktop_themes(fastframe_theme::DesktopThemes {
        slug: "spotifast",
        omarchy_template: include_str!("../contrib/omarchy/spotifast.json.tpl"),
        // The template has not changed since it first shipped.
        omarchy_previous_templates: &[],
        presets: true,
    });
}

/// The status line under the Theme setting, empty when all is well.
pub fn catalog_detail(
    catalog: &Catalog,
    locale: Locale,
    selected: Option<&str>,
) -> Cow<'static, str> {
    status_detail(catalog.status(selected), locale)
}

/// The status line under a Follow system light or dark theme setting.
pub fn pair_detail(
    catalog: &Catalog,
    locale: Locale,
    pair: &ThemePair,
    base: fastframe_theme::Base,
) -> Cow<'static, str> {
    status_detail(pair.status(base, catalog), locale)
}

fn status_detail(status: Option<fastframe_theme::Status>, locale: Locale) -> Cow<'static, str> {
    use fastframe_theme::{Problem, Status};
    let Some(status) = status else {
        return Cow::Borrowed("");
    };
    match status {
        Status::Loading => gettext(locale, "Loading local themes…"),
        Status::SelectedUnavailable => gettext(
            locale,
            "The selected theme is unavailable. Keeping the last usable appearance. See the log for details.",
        ),
        Status::Problem(Problem::Unreadable) => gettext(
            locale,
            "The themes folder could not be read. See the log for details.",
        ),
        Status::Problem(Problem::TooManyEntries) => gettext(
            locale,
            "The themes folder has more than 512 entries. Keep fewer files there to list the custom palettes.",
        ),
        Status::Problem(Problem::TooManyThemes) => gettext(
            locale,
            "Only 128 custom palettes can be listed. Keep fewer JSON files in the themes folder to see the rest.",
        ),
        Status::Problem(Problem::OmarchyUnreadable) => gettext(
            locale,
            "The Omarchy palette could not be loaded. Keeping the last usable appearance. See the log for details.",
        ),
        Status::Problem(_) => gettext(
            locale,
            "Custom themes could not be loaded. Run spotifast reload-themes to try again.",
        ),
    }
}

pub const RADIUS: u8 = 8;
pub const RADIUS_SMALL: u8 = 4;
pub const ROW_HEIGHT: f32 = 56.0;
pub const COMPACT_ROW_HEIGHT: f32 = 48.0;
/// The compact track list: one line, no cover.
pub const THIN_ROW_HEIGHT: f32 = 36.0;
pub const PLAYER_BAR_HEIGHT: f32 = 88.0;
/// The narrowest either right-hand panel goes. The queue and the lyrics
/// take the same edge and swap places there, so a width that suits one
/// has to suit the other, or the window would jump on the swap.
pub const SIDE_PANEL_MIN_WIDTH: f32 = 280.0;
pub const TOP_BAR_HEIGHT: f32 = 56.0;
pub const CUSTOM_TOP_BAR_HEIGHT: f32 = 48.0;

/// Keep the custom Windows header close to its 36-point caption controls.
/// Linux's custom title bar keeps the ordinary height.
pub fn top_bar_height() -> f32 {
    if cfg!(windows) && fastframe_titlebar::enabled() {
        CUSTOM_TOP_BAR_HEIGHT
    } else {
        TOP_BAR_HEIGHT
    }
}

/// macOS hides the titlebar and draws the window content all the way to the
/// top edge, so whatever sits at the top of the window has to leave room for
/// the traffic lights. Zero everywhere else, and in fullscreen, where the
/// buttons are gone.
pub fn titlebar_inset(ctx: &egui::Context) -> f32 {
    if cfg!(target_os = "macos") && !ctx.input(|input| input.viewport().fullscreen.unwrap_or(false))
    {
        28.0
    } else {
        0.0
    }
}

pub fn regular(size: f32) -> egui::FontId {
    fastframe_fonts::Weight::Regular.font_id(size)
}

pub fn medium(size: f32) -> egui::FontId {
    fastframe_fonts::Weight::Medium.font_id(size)
}

pub fn semibold(size: f32) -> egui::FontId {
    fastframe_fonts::Weight::SemiBold.font_id(size)
}

pub fn bold(size: f32) -> egui::FontId {
    fastframe_fonts::Weight::Bold.font_id(size)
}

/// How the desktop renders text, read once per process.
///
/// Tests use the platform's default instead of asking the desktop, so they
/// neither wait on D-Bus nor depend on the machine's settings.
pub fn text_rendering() -> fastframe_text::TextRendering {
    static RENDERING: std::sync::OnceLock<fastframe_text::TextRendering> =
        std::sync::OnceLock::new();
    *RENDERING.get_or_init(|| {
        if cfg!(test) {
            fastframe_text::TextRendering::platform_default()
        } else {
            fastframe_text::detect()
        }
    })
}

/// Install fonts, icons, and the base style once.
pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    egui_extras::install_image_loaders(ctx);
    fastframe_icons::install::<Icon>(ctx);
    fastframe_titlebar::install(ctx);
    // Colour emoji over every text egui draws (see `crate::emoji`).
    ctx.add_plugin(fastframe_emoji::EmojiPlugin::default());
}

/// Applies the palette to egui's own widgets so dialogs, menus, and text
/// fields agree with the custom views.
pub fn apply(ctx: &egui::Context, palette: &Palette) {
    let mut style = (*ctx.global_style()).clone();
    apply_to_style(&mut style, palette);
    ctx.set_global_style(style);
}

/// Applies a palette to this view and children without changing global style.
pub fn apply_local(ui: &mut egui::Ui, palette: &Palette) {
    apply_to_style(ui.style_mut(), palette);
}

fn apply_to_style(style: &mut egui::Style, palette: &Palette) {
    let visuals = &mut style.visuals;
    *visuals = if palette.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.dark_mode = palette.dark;
    // Glyph coverage, hinting and sub-pixel positions as the desktop draws
    // them: linear coverage in both themes on Linux, where egui's dark curve
    // (2c - c²) made text heavier than GTK's.
    text_rendering().apply_to_visuals(visuals);
    visuals.panel_fill = palette.panel;
    visuals.window_fill = palette.overlay;
    visuals.extreme_bg_color = palette.surface;
    visuals.faint_bg_color = palette.surface;
    visuals.code_bg_color = palette.surface;
    visuals.override_text_color = Some(palette.text);
    visuals.weak_text_color = Some(palette.secondary);
    visuals.hyperlink_color = palette.text;
    visuals.selection.bg_fill = palette.accent.gamma_multiply(0.35);
    visuals.selection.stroke = Stroke::new(1.0, palette.accent);
    visuals.window_stroke = Stroke::new(1.0, palette.outline);
    visuals.window_corner_radius = CornerRadius::same(RADIUS + 2);
    visuals.menu_corner_radius = CornerRadius::same(RADIUS);
    visuals.window_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: palette.shadow,
    };
    visuals.popup_shadow = egui::epaint::Shadow {
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: palette.shadow,
    };
    let corner = CornerRadius::same(RADIUS_SMALL + 2);
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = corner;
        widget.bg_stroke = Stroke::NONE;
        widget.fg_stroke = Stroke::new(1.0, palette.text);
        widget.expansion = 0.0;
    }
    visuals.widgets.noninteractive.corner_radius = corner;
    visuals.widgets.noninteractive.bg_fill = palette.panel;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.outline);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.text);
    visuals.widgets.inactive.bg_fill = palette.surface;
    visuals.widgets.inactive.weak_bg_fill = palette.surface;
    visuals.widgets.hovered.bg_fill = palette.surface_hover;
    visuals.widgets.hovered.weak_bg_fill = palette.surface_hover;
    visuals.widgets.active.bg_fill = palette.surface_active;
    visuals.widgets.active.weak_bg_fill = palette.surface_active;
    visuals.widgets.open.bg_fill = palette.surface_hover;
    visuals.widgets.open.weak_bg_fill = palette.surface_hover;
    visuals.text_cursor.stroke = Stroke::new(2.0, palette.accent);
    visuals.striped = false;
    visuals.slider_trailing_fill = true;
    visuals.handle_shape = egui::style::HandleShape::Circle;

    use egui::FontFamily::{Monospace, Proportional};
    use egui::{FontId, TextStyle};
    style.text_styles = [
        (TextStyle::Small, FontId::new(11.5, Proportional)),
        (TextStyle::Body, FontId::new(14.0, Proportional)),
        (TextStyle::Button, FontId::new(14.0, Proportional)),
        (TextStyle::Heading, FontId::new(22.0, Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, Monospace)),
    ]
    .into();
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.spacing.interact_size = Vec2::new(40.0, 28.0);
    style.spacing.menu_margin = egui::Margin::same(6);
    style.spacing.window_margin = egui::Margin::same(16);
    style.spacing.scroll = egui::style::ScrollStyle {
        bar_width: 8.0,
        floating_width: 6.0,
        floating_allocated_width: 0.0,
        handle_min_length: 28.0,
        bar_inner_margin: 3.0,
        bar_outer_margin: 2.0,
        dormant_background_opacity: 0.0,
        dormant_handle_opacity: 0.0,
        active_background_opacity: 0.0,
        active_handle_opacity: 0.55,
        interact_handle_opacity: 0.85,
        foreground_color: true,
        ..egui::style::ScrollStyle::floating()
    };
    style.interaction.selectable_labels = false;
    style.interaction.tooltip_delay = 0.4;
    style.animation_time = crate::motion::ANIMATION_TIME;
    style.url_in_tooltip = false;
}

/// Inter at its four weights with the monochrome emoji face right behind it
/// (so every emoji wears the same style, ahead of egui's own pair), then the
/// installed faces for the scripts Inter lacks, drawn the way the desktop
/// renders text.
fn install_fonts(ctx: &egui::Context) {
    let emoji = egui::FontData::from_static(include_bytes!("../assets/fonts/NotoEmoji.ttf"));
    let mut fonts = fastframe_fonts::FontSetup::default()
        .companion("noto_emoji", std::sync::Arc::new(emoji))
        .definitions();
    text_rendering().apply_to(&mut fonts);
    ctx.set_fonts(fonts);
}

fastframe_icons::icons! {
    /// Every icon the interface draws. The shared Lucide icons come from
    /// fastframe-icons; the rest are Spotifast's own files.
    pub enum Icon {
        prefix: "spotifast-icon-",
        directory: "../assets/icons/",
        ArrowLeft => lucide "arrow-left",
        ArrowRight => "arrow-right",
        AudioLines => "audio-lines",
        BadgeCheck => "badge-check",
        Bookmark => "bookmark",
        BookmarkFilled => "bookmark-filled",
        Car => "car",
        Cast => "cast",
        Check => lucide "check",
        ChevronDown => lucide "chevron-down",
        ChevronLeft => lucide "chevron-left",
        ChevronRight => lucide "chevron-right",
        ChevronUp => lucide "chevron-up",
        CircleAlert => lucide "circle-alert",
        CircleCheck => lucide "circle-check",
        CirclePlay => "circle-play",
        CirclePlus => "circle-plus",
        CircleX => lucide "circle-x",
        Clock => lucide "clock",
        Compass => "compass",
        Copy => lucide "copy",
        Disc => "disc-3",
        Ellipsis => lucide "ellipsis",
        Expand => "expand",
        Eye => "eye",
        EyeOff => "eye-off",
        ExternalLink => lucide "external-link",
        Gamepad => "gamepad-2",
        Globe => "globe",
        GripVertical => "grip-vertical",
        Headphones => "headphones",
        Heart => "heart",
        HeartFilled => "heart-filled",
        House => "house",
        Info => lucide "info",
        Laptop => "laptop",
        Library => "library",
        LayoutGrid => "layout-grid",
        LayoutList => "layout-list",
        ListEnd => "list-end",
        ListMusic => "list-music",
        ListPlus => "list-plus",
        ListVideo => "list-video",
        Loader => "loader-circle",
        Lock => lucide "lock",
        LogOut => lucide "log-out",
        Mic => lucide "mic",
        Minus => lucide "minus",
        Monitor => lucide "monitor",
        Moon => lucide "moon",
        Music => "music",
        Pause => lucide "pause",
        PauseFilled => "pause-filled",
        PanelLeft => lucide "panel-left",
        Pin => lucide "pin",
        PinOff => lucide "pin-off",
        Pencil => lucide "pencil",
        Play => lucide "play",
        PlayFilled => "play-filled",
        Plus => lucide "plus",
        Radio => "radio",
        Refresh => lucide "refresh-cw",
        Repeat => "repeat",
        Repeat1 => "repeat-1",
        Search => lucide "search",
        Settings => lucide "settings",
        Shrink => "shrink",
        Shuffle => "shuffle",
        SkipBack => "skip-back",
        SkipBackFilled => "skip-back-filled",
        SkipForward => "skip-forward",
        SkipForwardFilled => "skip-forward-filled",
        Smartphone => lucide "smartphone",
        Sparkles => "sparkles",
        Speaker => "speaker",
        Square => "square",
        SquarePen => lucide "square-pen",
        Sun => lucide "sun",
        Tablet => "tablet",
        Trash => lucide "trash-2",
        TrendingUp => "trending-up",
        Tv => "tv",
        User => lucide "user",
        Users => lucide "users",
        Volume => "volume",
        Volume1 => "volume-1",
        Volume2 => lucide "volume-2",
        VolumeX => lucide "volume-x",
        Watch => "watch",
        WifiOff => "wifi-off",
        X => lucide "x",
        Zap => "zap",
    }
}

/// A static icon.
pub fn icon(ui: &mut egui::Ui, icon: Icon, size: f32, color: Color32) -> Response {
    ui.add(icon.image(color, size))
}

/// Paints an icon centred in `rect` without allocating space.
pub fn paint_icon(ui: &egui::Ui, icon: Icon, rect: egui::Rect, size: f32, color: Color32) {
    let icon_rect = egui::Rect::from_center_size(
        rect.center() + play_glyph_offset(icon, size),
        Vec2::splat(size),
    );
    icon.image(color, size).paint_at(ui, icon_rect);
}

/// A bell, drawn in the line style of the Lucide icons, which have none in
/// fastframe-icons: a dome over a flared rim, and the clapper below it.
/// `size` is the icon's edge, as for [`paint_icon`].
pub fn paint_bell(painter: &egui::Painter, center: egui::Pos2, size: f32, color: Color32) {
    let unit = size / 24.0;
    let at = |x: f32, y: f32| center + Vec2::new((x - 12.0) * unit, (y - 12.0) * unit);
    let stroke = egui::Stroke::new((2.0 * unit).max(1.0), color);
    // Lucide's bell outline, its curves broken into short segments.
    let mut outline = Vec::new();
    // The left flare up to the dome.
    outline.push(at(3.3, 15.5));
    outline.push(at(6.0, 12.6));
    outline.push(at(6.0, 8.0));
    // The dome, a half circle of radius 6 around (12, 8).
    let steps = 16;
    for step in 1..steps {
        let angle = std::f32::consts::PI * (1.0 - step as f32 / steps as f32);
        outline.push(at(12.0 + 6.0 * angle.cos(), 8.0 - 6.0 * angle.sin()));
    }
    outline.push(at(18.0, 8.0));
    outline.push(at(18.0, 12.6));
    outline.push(at(20.7, 15.5));
    outline.push(at(20.7, 17.0));
    outline.push(at(3.3, 17.0));
    outline.push(at(3.3, 15.5));
    painter.add(egui::Shape::line(outline, stroke));
    // The clapper.
    let clapper = (0..=8)
        .map(|step| {
            let angle = std::f32::consts::PI * step as f32 / 8.0;
            at(12.0 - 1.8 * angle.cos(), 20.0 + 1.6 * angle.sin())
        })
        .collect();
    painter.add(egui::Shape::line(clapper, stroke));
}

fn count_pill_galley(
    painter: &egui::Painter,
    palette: &Palette,
    count: usize,
) -> std::sync::Arc<egui::Galley> {
    let text = if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    };
    painter.layout_no_wrap(text, bold(10.5), palette.on_accent)
}

const COUNT_PILL_HEIGHT: f32 = 16.0;

/// How wide [`paint_count_pill`] draws `count`.
pub fn count_pill_width(painter: &egui::Painter, palette: &Palette, count: usize) -> f32 {
    (count_pill_galley(painter, palette, count).size().x + 10.0).max(COUNT_PILL_HEIGHT)
}

/// A count on an icon or beside a title, as Spotify's bell shows it: an
/// accent pill, `99+` past 99. `center` is the pill's middle.
pub fn paint_count_pill(
    painter: &egui::Painter,
    palette: &Palette,
    center: egui::Pos2,
    count: usize,
) {
    let galley = count_pill_galley(painter, palette, count);
    let size = Vec2::new(
        (galley.size().x + 10.0).max(COUNT_PILL_HEIGHT),
        COUNT_PILL_HEIGHT,
    );
    let rect = egui::Rect::from_center_size(center, size);
    painter.rect_filled(rect, COUNT_PILL_HEIGHT / 2.0, palette.accent);
    painter.galley(
        rect.center() - galley.size() / 2.0,
        galley,
        palette.on_accent,
    );
}

/// Make keyboard focus visible without changing the control's layout.
pub fn focus_ring(ui: &egui::Ui, response: &Response) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(2.0),
            4.0,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Outside,
        );
    }
    if response.gained_focus() {
        response.scroll_to_me(None);
    }
}

/// The padding [`icon_button`] keeps around its glyph on every side. A
/// caller that places an icon button inside a fixed box must give the box
/// at least `size + ICON_BUTTON_PAD`: in a tighter one, egui clamps the
/// centered layout to the box's top edge and the glyph lands off-center.
pub const ICON_BUTTON_PAD: f32 = 12.0;

/// A frameless icon control whose colour lifts on hover.
pub fn icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    size: f32,
    color: Color32,
    hover: Color32,
    tooltip: &str,
) -> Response {
    let edge = size + ICON_BUTTON_PAD;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(edge), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tooltip)
    });
    if ui.is_rect_visible(rect) {
        let tint = if response.hovered() || response.has_focus() {
            hover
        } else {
            color
        };
        let scale = if response.is_pointer_button_down_on() {
            0.92
        } else {
            1.0
        };
        paint_icon(ui, icon, rect, size * scale, tint);
    }
    focus_ring(ui, &response);
    if tooltip.is_empty() {
        response
    } else {
        response.on_hover_text(tooltip)
    }
}

/// Horizontal offset that optically centers play triangles.
///
/// Lucide includes a 1/24-width shift; a measured 3% shift centers the icon at
/// Spotifast's sizes. Use this everywhere instead of per-call adjustments.
pub fn play_glyph_offset(icon: Icon, icon_size: f32) -> Vec2 {
    if matches!(icon, Icon::PlayFilled | Icon::Play) {
        Vec2::new(icon_size * (0.03 - 1.0 / 24.0), 0.0)
    } else {
        Vec2::ZERO
    }
}

/// The app's mark, the same picture as the app icon: the polished green
/// disc with the play triangle, rasterised once per size by
/// `util::app_icon_rgba` and drawn wherever the app shows its logo.
pub fn logo(ui: &egui::Ui, center: egui::Pos2, diameter: f32) {
    let ppp = ui.ctx().pixels_per_point();
    // The raster keeps two pixels of margin on each side of the disc.
    let pixels = (diameter * ppp).round() as usize + 4;
    let id = egui::Id::new(("spotifast-logo", pixels));
    let texture = ui
        .ctx()
        .data(|data| data.get_temp::<egui::TextureHandle>(id))
        .unwrap_or_else(|| {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [pixels, pixels],
                &crate::util::app_icon_rgba(pixels),
            );
            let texture =
                ui.ctx()
                    .load_texture("spotifast-logo", image, egui::TextureOptions::LINEAR);
            ui.ctx()
                .data_mut(|data| data.insert_temp(id, texture.clone()));
            texture
        });
    let side = pixels as f32 / ppp;
    let rect = egui::Rect::from_center_size(center, Vec2::splat(side));
    ui.painter().image(
        texture.id(),
        rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
}

pub fn circle_button(
    ui: &mut egui::Ui,
    icon: Icon,
    diameter: f32,
    fill: Color32,
    fill_hover: Color32,
    icon_color: Color32,
    tooltip: &str,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tooltip)
    });
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let grow = if hovered { 1.05 } else { 1.0 };
        let radius = diameter / 2.0 * grow;
        let fill = if hovered { fill_hover } else { fill };
        ui.painter().circle_filled(rect.center(), radius, fill);
        let icon_size = diameter * 0.46;
        let offset = play_glyph_offset(icon, icon_size);
        let icon_rect =
            egui::Rect::from_center_size(rect.center() + offset, Vec2::splat(icon_size));
        icon.image(icon_color, icon_size).paint_at(ui, icon_rect);
    }
    focus_ring(ui, &response);
    if tooltip.is_empty() {
        response
    } else {
        response.on_hover_text(tooltip)
    }
}

/// A disc the size of a [`circle_button`] whose icon is replaced by a
/// spinner: the pressed play button itself shows that Spotify is reacting.
pub fn circle_spinner(
    ui: &mut egui::Ui,
    diameter: f32,
    fill: Color32,
    spin: Color32,
    tooltip: &str,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::hover());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::ProgressIndicator,
            ui.is_enabled(),
            tooltip,
        )
    });
    if ui.is_rect_visible(rect) {
        ui.painter()
            .circle_filled(rect.center(), diameter / 2.0, fill);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(
            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
        ));
        spinner(&mut child, diameter * 0.55, spin);
    }
    if tooltip.is_empty() {
        response
    } else {
        response.on_hover_text(tooltip)
    }
}

/// A pill-shaped text button: filled for the primary action, outlined otherwise.
pub fn pill_button(ui: &mut egui::Ui, palette: &Palette, label: &str, primary: bool) -> Response {
    let font = semibold(13.0);
    let color = if primary {
        palette.on_accent
    } else {
        palette.text
    };
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, color);
    let padding = Vec2::new(18.0, 8.0);
    let size = galley.size() + padding * 2.0;
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let radius = rect.height() / 2.0;
        if primary {
            let fill = if hovered {
                palette.accent_hover
            } else {
                palette.accent
            };
            ui.painter().rect_filled(rect, radius, fill);
        } else {
            let stroke_color = if hovered { palette.text } else { palette.dim };
            ui.painter().rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, stroke_color),
                egui::StrokeKind::Inside,
            );
        }
        let pos = rect.center() - galley.size() / 2.0;
        ui.painter().galley(pos, galley, color);
    }
    focus_ring(ui, &response);
    response
}

/// A muted button with an icon and label, for row and header actions.
pub fn soft_button(
    ui: &mut egui::Ui,
    palette: &Palette,
    icon: Option<Icon>,
    label: &str,
    active: bool,
) -> Response {
    soft_button_inner(ui, palette, icon, label, active, false).0
}

/// A [`soft_button`] whose icon turns into a cross on hover, allowing the
/// entry to be dismissed. The returned flag reports clicks on that cross.
pub fn soft_button_dismiss(
    ui: &mut egui::Ui,
    palette: &Palette,
    icon: Icon,
    label: &str,
) -> (Response, bool) {
    soft_button_inner(ui, palette, Some(icon), label, false, true)
}

/// The width `soft_button` gives a button without an icon, for laying out
/// a row of them before drawing it.
pub fn soft_button_width(ui: &egui::Ui, label: &str) -> f32 {
    let galley = crate::bidi::layout_line(ui.painter(), label, medium(13.0), Color32::WHITE);
    galley.size().x + 24.0
}

fn soft_button_inner(
    ui: &mut egui::Ui,
    palette: &Palette,
    icon: Option<Icon>,
    label: &str,
    active: bool,
    dismissible: bool,
) -> (Response, bool) {
    let font = medium(13.0);
    let color = if active { palette.window } else { palette.text };
    let galley = crate::bidi::layout_line(ui.painter(), label, font, color);
    let icon_size = 15.0;
    let icon_width = if icon.is_some() { icon_size + 6.0 } else { 0.0 };
    let padding = Vec2::new(12.0, 7.0);
    let size = Vec2::new(galley.size().x + icon_width, galley.size().y) + padding * 2.0;
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + padding.x + icon_size / 2.0, rect.center().y),
        Vec2::splat(icon_size),
    );
    // Claimed after the button, so the cross sits on top and keeps its own click.
    let dismiss = (dismissible && icon.is_some()).then(|| {
        let dismiss = ui.interact(
            icon_rect.expand(3.0),
            response.id.with("dismiss"),
            Sense::click(),
        );
        dismiss.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                ui.is_enabled(),
                format!("Remove {label}"),
            )
        });
        dismiss
    });
    let dismissed = dismiss.as_ref().is_some_and(Response::clicked);
    let over_dismiss = dismiss
        .as_ref()
        .is_some_and(|dismiss| dismiss.hovered() || dismiss.has_focus());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() || over_dismiss;
        let fill = if active {
            palette.text
        } else if hovered {
            palette.surface_hover
        } else {
            palette.surface
        };
        ui.painter().rect_filled(rect, rect.height() / 2.0, fill);
        let mut x = rect.left() + padding.x;
        if let Some(icon) = icon {
            let icon = if dismiss.is_some() && hovered {
                Icon::X
            } else {
                icon
            };
            icon.image(color, icon_size).paint_at(ui, icon_rect);
            x += icon_width;
        }
        let pos = egui::pos2(x, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(pos, galley, color);
    }
    focus_ring(ui, &response);
    if let Some(dismiss) = dismiss {
        focus_ring(ui, &dismiss);
    }
    (response, dismissed)
}

/// An animated busy indicator paced independently of the graphics driver.
pub fn spinner(ui: &mut egui::Ui, size: f32, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
        let radius = size / 2.0 - 2.0;
        let start = ui.input(|input| input.time) * std::f64::consts::TAU * 1.2;
        let sweep = 250_f64.to_radians();
        let points = (0..20)
            .map(|index| {
                let angle = start + sweep * f64::from(index) / 19.0;
                let (sin, cos) = angle.sin_cos();
                rect.center() + radius * egui::vec2(cos as f32, sin as f32)
            })
            .collect();
        ui.painter()
            .add(egui::Shape::line(points, Stroke::new(2.0, color)));
    }
    response
}

/// Truncated single-line text in a given font and colour.
pub fn text(
    ui: &mut egui::Ui,
    text: impl Into<String>,
    font: egui::FontId,
    color: Color32,
) -> Response {
    let text = text.into();
    if crate::bidi::is_rtl(&text) {
        // Laid out here so a cut lands at the reading end, on the left.
        let galley = crate::bidi::layout(
            ui.painter(),
            &text,
            font,
            color,
            ui.available_width(),
            1,
            Some(crate::bidi::ELLIPSIS),
        );
        return ui.add(egui::Label::new(galley).selectable(false));
    }
    ui.add(
        egui::Label::new(egui::RichText::new(text).font(font).color(color))
            .truncate()
            .selectable(false),
    )
}

/// Single-line text that acts like a link: underlines on hover, clickable.
pub fn link(
    ui: &mut egui::Ui,
    text: impl Into<String>,
    font: egui::FontId,
    color: Color32,
) -> Response {
    let text = text.into();
    let response = if crate::bidi::is_rtl(&text) {
        let galley = crate::bidi::layout(
            ui.painter(),
            &text,
            font,
            color,
            ui.available_width(),
            1,
            Some(crate::bidi::ELLIPSIS),
        );
        ui.add(
            egui::Label::new(galley)
                .selectable(false)
                .sense(Sense::click()),
        )
    } else {
        ui.add(
            egui::Label::new(egui::RichText::new(text.clone()).font(font).color(color))
                .truncate()
                .selectable(false)
                .sense(Sense::click()),
        )
    };
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Link, ui.is_enabled(), &text));
    focus_ring(ui, &response);
    if response.hovered() {
        let rect = response.rect;
        ui.painter()
            .hline(rect.x_range(), rect.bottom() - 1.0, Stroke::new(1.0, color));
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn section_title(ui: &mut egui::Ui, palette: &Palette, label: &str) -> Response {
    text(ui, label, bold(17.0), palette.text)
}

pub fn subtle(ui: &mut egui::Ui, palette: &Palette, label: &str) -> Response {
    text(ui, label, regular(13.0), palette.secondary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "spotifast-theme-{name}-{:016x}",
            rand::random::<u64>()
        ))
    }

    #[test]
    fn the_bundled_spotify_palette_is_a_valid_dark_one() {
        // #given the file shipped in the binary
        let (name, text) = BUNDLED[0];

        // #when it is read as a palette
        let palette: Palette = fastframe_theme::parse_palette(text).expect("a valid palette");

        // #then it is the dark Spotify one
        assert_eq!(name, "Spotify.json");
        assert!(palette.dark);
        assert_eq!(palette.window, Color32::from_rgb(0, 0, 0));
        assert_eq!(palette.accent, Color32::from_rgb(0x1e, 0xd7, 0x60));
        assert_eq!(palette.overlay, Color32::from_rgb(0x28, 0x28, 0x28));
    }

    #[test]
    fn bundled_palettes_are_written_when_missing_and_never_overwritten() {
        // #given a themes folder that does not exist yet
        let directory = scratch("install").join("themes");

        // #when the bundled palettes are installed
        install_bundled(&directory).expect("installed");

        // #then Spotify.json is there, whole
        let path = directory.join("Spotify.json");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            include_str!("../assets/themes/Spotify.json")
        );

        // #when the user edits it and the app starts again
        std::fs::write(&path, "{\"base\":\"dark\",\"colors\":{}}").unwrap();
        install_bundled(&directory).expect("installed again");

        // #then their file is kept
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"base\":\"dark\",\"colors\":{}}"
        );
        let _ = std::fs::remove_dir_all(directory.parent().unwrap());
    }

    #[test]
    fn a_fresh_launch_lists_spotify_after_the_scan() {
        // #given a catalogue started on an empty themes folder
        let directory = scratch("scan").join("themes");
        let mut catalog = Catalog::default();
        catalog.start(
            directory.clone(),
            Vec::new(),
            &fastframe_theme::Waker::default(),
        );
        assert!(catalog.loading());

        // #when the install and the scan finish
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while catalog.loading() && std::time::Instant::now() < deadline {
            catalog.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        // #then Spotify is a listed dark palette
        let spotify = catalog.find("Spotify.json").expect("Spotify is listed");
        assert!(spotify.palette.dark);
        let _ = std::fs::remove_dir_all(directory.parent().unwrap());
    }

    /// The logo drawn in the app is the app icon's own picture, not a
    /// disc in the theme's accent colour: it uploads the icon's pixels.
    #[test]
    fn the_logo_in_the_app_is_the_app_icon() {
        // #given the logo drawn 40 points wide at twice the pixel density
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(2.0);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            logo(ui, egui::pos2(40.0, 40.0), 40.0);
        });

        // #then the frame uploads the icon rasterised at that size
        let uploaded = output
            .textures_delta
            .set
            .values()
            .flat_map(|deltas| deltas.iter())
            .find_map(|delta| match &delta.image {
                egui::ImageData::Color(image) if image.size == [84, 84] => Some(image.clone()),
                _ => None,
            })
            .expect("the logo's texture");
        let icon = crate::util::app_icon_rgba(84);
        for (x, y) in [(42, 10), (47, 42), (42, 4)] {
            let expected = &icon[(y * 84 + x) * 4..(y * 84 + x) * 4 + 4];
            let got = uploaded.pixels[y * 84 + x].to_srgba_unmultiplied();
            assert_eq!(&got[..], expected, "pixel {x},{y}");
        }
        output.textures_delta.clear();
    }

    /// Palette files name the sixteen colours every app shares and
    /// Spotifast's own `tint`, and only those: a typo is an invalid file,
    /// not an ignored colour.
    #[test]
    fn palette_files_set_every_base_colour() {
        use fastframe_theme::Palette as _;
        for name in fastframe_theme::BASE_COLORS.iter().chain(&["tint"]) {
            let mut palette = Palette::dark();
            assert!(palette.set(name, Color32::from_rgb(1, 2, 3)), "{name}");
            assert_ne!(palette, Palette::dark(), "{name}");
        }
        assert!(!Palette::dark().set("typo", Color32::RED));
        assert_eq!(Palette::dark().tint, Color32::TRANSPARENT);
        assert_eq!(Palette::light().tint, Color32::TRANSPARENT);
        let light: Palette =
            fastframe_theme::parse_palette(r##"{"base":"light","colors":{"accent":"#8c3fa5"}}"##)
                .unwrap();
        assert!(!light.dark);
        assert_eq!(light.accent, Color32::from_rgb(140, 63, 165));
        assert_eq!(light.window, Palette::light().window);
    }

    /// Ristretto's brown window must not open blue-grey menus.
    #[test]
    fn menus_follow_a_palette_that_leaves_overlay_out() {
        let shared: Palette = fastframe_theme::presets::themes()
            .find(|theme: &CustomTheme| theme.filename == "Ristretto.json")
            .unwrap()
            .palette;
        let [r, _, b, _] = shared.overlay.to_array();
        assert!(r > b, "{:?}", shared.overlay);
        assert_ne!(shared.overlay, Palette::dark().overlay);

        let latte: Palette = fastframe_theme::parse_palette(
            r##"{"base":"light","colors":{"window":"#eff1f5","panel":"#edeff3"}}"##,
        )
        .unwrap();
        assert_eq!(latte.overlay, latte.panel);

        let own: Palette = fastframe_theme::parse_palette(
            r##"{"colors":{"window":"#2c2525","overlay":"#010203"}}"##,
        )
        .unwrap();
        assert_eq!(own.overlay, Color32::from_rgb(1, 2, 3));
        let accent_only: Palette =
            fastframe_theme::parse_palette(r##"{"colors":{"accent":"#8c3fa5"}}"##).unwrap();
        assert_eq!(accent_only.overlay, Palette::dark().overlay);
    }

    /// Compared line by line: a Windows checkout may turn the files' line
    /// endings into CRLF, which no Linux package ships.
    #[test]
    fn the_shipped_omarchy_files_are_the_shared_ones() {
        let lines = |text: &str| text.replace("\r\n", "\n");
        assert_eq!(
            lines(include_str!("../contrib/omarchy/spotifast-theme")),
            lines(&fastframe_theme::omarchy::hook_script("spotifast"))
        );
        assert_eq!(
            lines(include_str!("../contrib/omarchy/spotifast.json.tpl")),
            lines(fastframe_theme::omarchy::BASE_TEMPLATE)
        );
    }

    #[test]
    fn every_catalog_problem_has_a_sentence() {
        let catalog = Catalog::default();
        assert_eq!(catalog_detail(&catalog, Locale::English, None), "");
        assert!(
            catalog_detail(&catalog, Locale::English, Some("gone.json")).contains("last usable")
        );
    }

    /// A palette replaces egui's visuals, which must not take back the
    /// desktop's text rendering: on Linux, linear coverage in both themes.
    #[test]
    fn palettes_keep_the_desktops_text_rendering() {
        for palette in [Palette::dark(), Palette::light()] {
            let ctx = egui::Context::default();
            apply(&ctx, &palette);
            let options = ctx.global_style().visuals.text_options;
            assert_eq!(
                options.color_transfer_function,
                text_rendering().color_transfer_function(palette.dark),
                "dark: {}",
                palette.dark
            );
            if cfg!(target_os = "linux") {
                assert_eq!(
                    options.color_transfer_function,
                    egui::epaint::FontColorTransferFunction::Off
                );
            }
            assert!(options.subpixel_binning);
        }
    }

    /// Native desktop apps keep the arrow over buttons and switch to the
    /// hand only over links (#508).
    #[test]
    fn only_links_show_the_hand_cursor() {
        let ctx = egui::Context::default();
        install(&ctx);
        let palette = Palette::dark();
        let draw = |ctx: &egui::Context, pointer: Option<egui::Pos2>| {
            let mut rects = (egui::Rect::NOTHING, egui::Rect::NOTHING);
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 200.0),
                )),
                events: pointer
                    .map(|at| vec![egui::Event::PointerMoved(at)])
                    .unwrap_or_default(),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                rects.0 = pill_button(ui, &palette, "Play", true).rect;
                rects.1 = link(ui, "Bonobo", regular(14.0), palette.text).rect;
            });
            output.textures_delta.clear();
            (rects, output.platform_output.cursor_icon)
        };
        draw(&ctx, None);
        let ((button, link_rect), _) = draw(&ctx, None);
        let (_, over_button) = draw(&ctx, Some(button.center()));
        assert_eq!(over_button, egui::CursorIcon::Default);
        let (_, over_link) = draw(&ctx, Some(link_rect.center()));
        assert_eq!(over_link, egui::CursorIcon::PointingHand);
    }

    #[test]
    fn a_local_palette_keeps_spotifasts_widget_style_local() {
        let ctx = egui::Context::default();
        apply(&ctx, &Palette::light());
        let dark = Palette::dark();
        let mut output = ctx.run_ui(Default::default(), |ui| {
            apply_local(ui, &dark);
            let style = ui.style();
            assert!(style.visuals.dark_mode);
            assert_eq!(
                style.visuals.widgets.inactive.corner_radius,
                CornerRadius::same(RADIUS_SMALL + 2)
            );
            assert_eq!(
                style.visuals.selection.stroke,
                Stroke::new(1.0, dark.accent)
            );
            assert_eq!(style.spacing.button_padding, Vec2::new(12.0, 6.0));
        });
        output.textures_delta.clear();
        assert!(!ctx.global_style().visuals.dark_mode);
    }

    #[test]
    fn fonts_install_and_layout_emojis() {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let galley = ui.painter().layout_no_wrap(
                "Rosewood 🔥 Otomo 🎵 ❤️ 🚀".to_string(),
                regular(14.0),
                Color32::WHITE,
            );
            assert!(galley.rows[0].glyphs.len() >= 5);
        });
        output.textures_delta.clear();
    }

    /// The monochrome emoji face comes right after Inter at every weight
    /// and in the monospace family, ahead of egui's own emoji pair.
    #[test]
    fn the_emoji_face_follows_inter_everywhere() {
        let ctx = egui::Context::default();
        install(&ctx);
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        let fonts = ctx.fonts(|fonts| fonts.definitions().clone());
        for weight in fastframe_fonts::Weight::ALL {
            let family = &fonts.families[&weight.family()];
            assert_eq!(family[..2], [weight.name(), "noto_emoji"], "{weight:?}");
        }
        assert_eq!(
            fonts.families[&egui::FontFamily::Monospace][1],
            "noto_emoji"
        );
    }

    #[test]
    fn fonts_install_and_layout_mixed_cjk() {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let galley = ui.painter().layout_no_wrap(
                "Track 87: 恋におちて -Fall in love- (Live)".to_string(),
                regular(14.0),
                Color32::WHITE,
            );
            assert!(!galley.rows.is_empty());
            assert!(galley.rows[0].glyphs.len() >= 10);
        });
        output.textures_delta.clear();
    }
}
