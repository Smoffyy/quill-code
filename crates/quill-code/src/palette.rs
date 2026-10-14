use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Global, Hsla, Window, WindowAppearance, px, rgb, rgba, transparent_black};

pub const FONT: &str = "Open Sans";

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    pub bg: Hsla,
    pub sidebar: Hsla,
    pub surface_2: Hsla,
    pub surface_hover: Hsla,
    pub border: Hsla,
    pub border_soft: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_faint: Hsla,
    pub accent: Hsla,
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
            bg: hex(0x151515),
            sidebar: hex(0x111111),
            surface_2: hex(0x20201f),
            surface_hover: hex(0x262625),
            border: hexa(0xffffff1a),
            border_soft: hexa(0xffffff14),
            text: hex(0xffffff),
            text_muted: hex(0xc3c2b7),
            text_faint: hex(0x898781),
            accent: hex(0xc6613f),
            sidebar_text: hex(0xc3c2b7),
            row_hover: hexa(0xffffff13),
            row_active: hexa(0xffffff1a),
            composer_bg: hex(0x20201f),
            composer_ring: hexa(0xffffff1a),
            composer_ring_hover: hexa(0xffffff33),
            menu_bg: hex(0x20201f),
            fill: hexa(0xffffff0d),
            add: hex(0x32d74b),
            del: hex(0xff2c56),
            link: hex(0x6da7ec),
            focus_ring: hex(0x5598e7),
            danger: hex(0xe5484d),
            warning: hex(0xe2b03f),
            success: hex(0x4caf7d),
            selection: hex(0x3d3d3d),
            scroll_thumb: hexa(0xe1e0d959),
        }
    }

    pub fn light() -> Self {
        Palette {
            dark: false,
            bg: hex(0xf8f8f6),
            sidebar: hex(0xf7f7f4),
            surface_2: hex(0xefeee7),
            surface_hover: hex(0xefeeeb),
            border: hex(0xdddddd),
            border_soft: hexa(0x00000014),
            text: hex(0x121212),
            text_muted: hex(0x3d3d3a),
            text_faint: hex(0x7b7974),
            accent: hex(0xc9663f),
            sidebar_text: hex(0x3d3d3a),
            row_hover: hexa(0x0000000f),
            row_active: hexa(0x00000014),
            composer_bg: hex(0xffffff),
            composer_ring: hexa(0x0000001a),
            composer_ring_hover: hexa(0x00000033),
            menu_bg: hex(0xffffff),
            fill: hexa(0x1f1e1d0b),
            add: hex(0x15803d),
            del: hex(0xd1243f),
            link: hex(0x2a78d6),
            focus_ring: hex(0x5598e7),
            danger: hex(0xd1243f),
            warning: hex(0xb7791f),
            success: hex(0x15803d),
            selection: hex(0xd8d6cc),
            scroll_thumb: hexa(0x1f1e1d40),
        }
    }

    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Palette::dark(),
            WindowAppearance::Light | WindowAppearance::VibrantLight => Palette::light(),
        }
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
        colors.primary = palette.text;
        colors.primary_foreground = palette.bg;
        colors.secondary = palette.fill;
        colors.secondary_foreground = palette.text;
        colors.secondary_hover = palette.row_hover;
        colors.secondary_active = palette.row_active;
        colors.list = palette.bg;
        colors.list_hover = palette.row_hover;
        colors.list_active = palette.row_active;
        colors.list_active_border = transparent_black();
        colors.link = palette.link;
        colors.danger = palette.danger;
        colors.warning = palette.warning;
        colors.success = palette.success;
        colors.title_bar = palette.sidebar;
        colors.title_bar_border = palette.sidebar;
        colors.sidebar = palette.sidebar;
        colors.sidebar_foreground = palette.sidebar_text;
        colors.sidebar_border = palette.sidebar;
        colors.sidebar_accent = palette.row_hover;
        colors.sidebar_accent_foreground = palette.text;
        colors.window_border = palette.border;
        colors.scrollbar = transparent_black();
        colors.scrollbar_thumb = palette.scroll_thumb;
        colors.scrollbar_thumb_hover = palette.scroll_thumb;
    });
}