use std::io::{BufRead, IsTerminal, Write};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use quill_config::settings_path;
use quill_providers::context_label;

use crate::backend::{self, Backend, Connection};
use crate::config::{Config, app_dir, personal_instructions_path};
use crate::text::{Line, accent, bad, bold, dim, good, pad, plain, warn};
use crate::{Msg, project, store, usage};

fn print(line: &Line) {
    if std::io::stdout().is_terminal() {
        println!("{}", line.ansi());
    } else {
        println!("{}", line.text());
    }
}

fn ask(question: &str) -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    eprint!("{question} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().lock().read_line(&mut answer);
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

pub fn models(scan: bool) -> u8 {
    let mut config = Config::load();
    if let Some(error) = &config.error {
        eprintln!("{error}");
    }
    if scan {
        let found = backend::scan(&config.settings);
        if found.is_empty() {
            print(&Line::styled("No new model server answered on the usual ports.", dim()));
        } else {
            config.update(|settings| backend::add_servers(settings, &found));
            for (kind, url) in &found {
                print(&Line::styled(format!("Added {} at {url}", kind.label()), good()));
            }
        }
    }
    if config.settings.models.providers.is_empty() {
        print(&Line::styled("No model server is set up yet. Run quill to connect one, or quill models --scan.", dim()));
        return 0;
    }
    let (tx, rx) = channel();
    let mut backend = Backend::new(tx);
    backend.refresh(&config.settings);
    let deadline = Instant::now() + Duration::from_secs(30);
    while backend.sources.iter().any(|source| source.connection == Connection::Checking) && Instant::now() < deadline {
        if let Ok(Msg::Probed { generation, id, found }) = rx.recv_timeout(Duration::from_millis(200)) {
            backend.probed(generation, &id, found, &config.settings);
        }
    }
    let selected = config.settings.models.selected.clone();
    for source in &backend.sources {
        let tone = match source.connection {
            Connection::Online => good(),
            Connection::Off => dim(),
            _ => warn(),
        };
        print(&Line::styled(&source.config.name, bold()).with(format!(" ({}) ", source.config.id), dim()).with(source.status(), tone));
        let widest = source.models.iter().map(|model| model.label().chars().count()).max().unwrap_or(0);
        for model in &source.models {
            let target = quill_config::ModelRef::new(source.config.id.clone(), model.id.clone());
            let chosen = selected.as_ref() == Some(&target);
            let mut line = if chosen { Line::styled("  ● ", accent()) } else { Line::plain("    ") };
            line.push(pad(model.label(), widest), if chosen { bold() } else { plain() });
            let mut detail = vec![format!("{}/{}", source.config.id, model.id), model.status.label().to_string()];
            if let Some(context) = model.context {
                detail.push(format!("{} context", context_label(context)));
            }
            if model.vision {
                detail.push("vision".into());
            }
            if config.settings.models.pinned.contains(&target) {
                detail.push("pinned".into());
            }
            line.push(format!("  {}", detail.join(" · ")), dim());
            print(&line);
        }
    }
    print(&Line::default());
    print(&Line::styled("Pick one with /model in Quill Code, or pass --model provider/model to quill exec.", dim()));
    0
}

pub fn usage() -> u8 {
    let threads = store::load_threads();
    let records = usage::load(&threads);
    let width = crossterm::terminal::size().map_or(100, |(columns, _)| columns as usize).clamp(40, 160) - 1;
    for line in usage::report(None, &records, width) {
        print(&line);
    }
    0
}

pub fn show_config() -> u8 {
    let config = Config::load();
    let path = settings_path();
    let status = match &config.error {
        Some(error) => Line::styled(error.clone(), bad()),
        None => Line::styled("ok", good()),
    };
    print(&Line::plain(format!("Settings      {}  ", path.display())).with_line(status));
    let instructions = personal_instructions_path();
    let note = if instructions.is_file() { "" } else { "  (not written yet)" };
    print(&Line::plain(format!("Instructions  {}", instructions.display())).with(note, dim()));
    let threads = store::load_threads();
    print(&Line::plain(format!("Threads       {}", app_dir().join("threads").display())).with(format!("  ({})", threads.len()), dim()));
    print(&Line::plain(format!("Worktrees     {}", config.settings.worktrees_dir().display())));
    print(&Line::default());
    print(&Line::styled("A project can override any setting in .quill/settings.json at its root.", dim()));
    print(&Line::styled("quill config edit opens the settings, quill config instructions your personal instructions.", dim()));
    0
}

pub fn edit_config() -> u8 {
    let config = Config::load();
    if !settings_path().is_file()
        && let Err(error) = quill_config::save(&config.settings)
    {
        eprintln!("The settings file could not be written: {error}");
        return 1;
    }
    if let Err(error) = project::edit(&settings_path()) {
        eprintln!("{error}");
        return 1;
    }
    match Config::load().error {
        Some(error) => {
            eprintln!("{error}. Quill Code keeps using the defaults until it is fixed.");
            1
        }
        None => 0,
    }
}

pub fn edit_instructions() -> u8 {
    let path = personal_instructions_path();
    if !path.is_file() {
        let made = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&path, ""));
        if let Err(error) = made {
            eprintln!("{} could not be created: {error}", path.display());
            return 1;
        }
    }
    match project::edit(&path) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

pub fn reset_config() -> u8 {
    let mut config = Config::load();
    if !ask("Reset every setting except your providers and models?") {
        eprintln!("Nothing was changed.");
        return 1;
    }
    let fresh = config.settings.reset();
    match config.replace(fresh) {
        Ok(()) => {
            print(&Line::styled("Settings reset. Your providers and models were kept.", good()));
            0
        }
        Err(error) => {
            eprintln!("The settings could not be saved: {error}");
            1
        }
    }
}