use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Global, Hsla, Window, WindowAppearance, px, rgb, rgba, transparent_black};
use quill_config::Theme as ThemeChoice;

pub const FONT: &str = "Open Sans";

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    pub bg: Hsla,
    pub sidebar: Hsla,
    pub raised: Hsla,
    pub surface_2: Hsla,
    pub surface_hover: Hsla,
    pub border: Hsla,
    pub border_soft: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_faint: Hsla,
    pub accent: Hsla,
    pub accent_soft: Hsla,
    pub accent_text: Hsla,
    pub sidebar_text: Hsla,
    pub row_hover: Hsla,
    pub row_active: Hsla,
    pub composer_bg: Hsla,
    pub composer_ring: Hsla,
    pub composer_ring_hover: Hsla,
    pub menu_bg: Hsla,
    pub fill: Hsla,
    pub add: Hsla,
    pub del: Hsla,
    pub link: Hsla,
    pub focus_ring: Hsla,
    pub danger: Hsla,
    pub warning: Hsla,
    pub success: Hsla,
    pub selection: Hsla,
    pub scroll_thumb: Hsla,
}

impl Global for Palette {}

fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

fn hexa(value: u32) -> Hsla {
    rgba(value).into()
}

impl Palette {
    pub fn dark() -> Self {
        Palette {
            dark: true,
            bg: hex(0x131316),
            sidebar: hex(0x0c0c0e),
            raised: hex(0x1a1a1f),
            surface_2: hex(0x18181c),
            surface_hover: hex(0x202026),
            border: hexa(0xffffff1c),
            border_soft: hexa(0xffffff10),
            text: hex(0xececf1),
            text_muted: hex(0xb0b2ba),
            text_faint: hex(0x777983),
            accent: hex(0x8e9cff),
            accent_soft: hexa(0x8e9cff24),
            accent_text: hex(0x0c0c14),
            sidebar_text: hex(0xb0b2ba),
            row_hover: hexa(0xffffff0d),
            row_active: hexa(0xffffff15),
            composer_bg: hex(0x1a1a1f),
            composer_ring: hexa(0xffffff17),
            composer_ring_hover: hexa(0xffffff2b),
            menu_bg: hex(0x1c1c22),
            fill: hexa(0xffffff0a),
            add: hex(0x4ac26b),
            del: hex(0xf0616d),
            link: hex(0x9aa7ff),
            focus_ring: hex(0x8e9cff),
            danger: hex(0xf0616d),
            warning: hex(0xe8b558),
            success: hex(0x4cc38a),
            selection: hexa(0x8e9cff47),
            scroll_thumb: hexa(0xffffff33),
        }
    }

    pub fn light() -> Self {
        Palette {
            dark: false,
            bg: hex(0xffffff),
            sidebar: hex(0xf3f3f5),
            raised: hex(0xffffff),
            surface_2: hex(0xf6f6f8),
            surface_hover: hex(0xefeff2),
            border: hexa(0x0000001c),
            border_soft: hexa(0x00000010),
            text: hex(0x18181b),
            text_muted: hex(0x4b4d55),
            text_faint: hex(0x878992),
            accent: hex(0x5865e8),
            accent_soft: hexa(0x5865e81c),
            accent_text: hex(0xffffff),
            sidebar_text: hex(0x4b4d55),
            row_hover: hexa(0x0000000b),
            row_active: hexa(0x00000013),
            composer_bg: hex(0xffffff),
            composer_ring: hexa(0x0000001a),
            composer_ring_hover: hexa(0x0000002e),
            menu_bg: hex(0xffffff),
            fill: hexa(0x0000000a),
            add: hex(0x1a7f37),
            del: hex(0xcf222e),
            link: hex(0x4553d6),
            focus_ring: hex(0x5865e8),
            danger: hex(0xcf222e),
            warning: hex(0xb7791f),
            success: hex(0x1a7f37),
            selection: hexa(0x5865e833),
            scroll_thumb: hexa(0x00000033),
        }
    }

    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Palette::dark(),
            WindowAppearance::Light | WindowAppearance::VibrantLight => Palette::light(),
        }
    }
}

pub fn resolve(theme: ThemeChoice, system: WindowAppearance) -> WindowAppearance {
    match theme {
        ThemeChoice::System => system,
        ThemeChoice::Dark => WindowAppearance::Dark,
        ThemeChoice::Light => WindowAppearance::Light,
    }
}

pub fn palette(cx: &App) -> Palette {
    *cx.global::<Palette>()
}

pub fn apply(appearance: WindowAppearance, window: Option<&mut Window>, cx: &mut App) {
    let palette = Palette::for_appearance(appearance);
    cx.set_global(palette);
    let mode = if palette.dark { ThemeMode::Dark } else { ThemeMode::Light };
    Theme::change(mode, window, cx);
    Theme::update(cx, |theme| {
        theme.font_family = FONT.into();
        theme.font_size = px(14.);
        theme.radius = px(8.);
        theme.radius_lg = px(12.);
        theme.shadow = false;
        let colors = &mut theme.colors;
        colors.background = palette.bg;
        colors.foreground = palette.text;
        colors.border = palette.border;
        colors.input = palette.border;
        colors.caret = palette.text;
        colors.selection = palette.selection;
        colors.ring = palette.focus_ring;
        colors.muted = palette.surface_2;
        colors.muted_foreground = palette.text_faint;
        colors.accent = palette.row_hover;
        colors.accent_foreground = palette.text;
        colors.popover = palette.menu_bg;
        colors.popover_foreground = palette.text;
        colors.primary = palette.accent;
        colors.primary_foreground = palette.accent_text;
        colors.primary_hover = palette.accent.opacity(0.9);
        colors.primary_active = palette.accent.opacity(0.8);
        colors.secondary = palette.fill;
        colors.secondary_foreground = palette.text;
        colors.secondary_hover = palette.row_hover;
        colors.secondary_active = palette.row_active;
        colors.list = palette.bg;
        colors.list_hover = palette.row_hover;
        colors.list_active = palette.accent_soft;
        colors.list_active_border = transparent_black();
        colors.link = palette.link;
        colors.danger = palette.danger;
        colors.warning = palette.warning;
        colors.success = palette.success;
        colors.switch = palette.accent;
        colors.title_bar = palette.sidebar;
        colors.title_bar_border = palette.sidebar;
        colors.sidebar = palette.sidebar;
        colors.sidebar_foreground = palette.sidebar_text;
        colors.sidebar_border = palette.sidebar;
        colors.sidebar_accent = palette.row_active;
        colors.sidebar_accent_foreground = palette.text;
        colors.window_border = palette.border;
        colors.scrollbar = transparent_black();
        colors.scrollbar_thumb = palette.scroll_thumb;
        colors.scrollbar_thumb_hover = palette.scroll_thumb;
    });
}