#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod add_project;
mod assets;
mod attachments;
mod backend;
mod commit_dialog;
mod composer;
mod config;
mod context_ring;
mod icons;
mod mentions;
mod model_settings;
mod motion;
mod palette;
mod panel;
mod project;
mod settings;
mod shell;
mod smooth_scroll;
mod store;
mod terminal;
mod thread;
mod thread_view;
mod ui;
mod usage;
mod version;
mod weave;
mod workspace;

use std::borrow::Cow;

use gpui_kit::component::TitleBar;
use gpui_kit::{App, Bounds, WindowBounds, WindowOptions, prelude::*, px, size};

fn load_fonts(cx: &App) {
    let fonts = vec![
        Cow::Borrowed(include_bytes!("../../../assets/fonts/OpenSans-Light.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../../../assets/fonts/OpenSans-Regular.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../../../assets/fonts/OpenSans-SemiBold.ttf").as_slice()),
    ];
    cx.text_system().add_fonts(fonts).expect("bundled fonts load");
}

fn main() {
    gpui_kit::application().with_assets(assets::Assets).run(|cx| {
        gpui_kit::init(cx);
        load_fonts(cx);
        cx.set_global(config::Config::load());
        let appearance = config::Config::get(cx).appearance.clone();
        motion::apply(appearance.motion, cx);
        palette::apply(palette::resolve(appearance.theme, cx.window_appearance()), None, cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1280.), px(820.)), cx))),
            window_min_size: Some(size(px(720.), px(480.))),
            ..TitleBar::window_options()
        };
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| shell::Shell::new(window, cx))).expect("open the main window");
        cx.activate(true);
    });
}