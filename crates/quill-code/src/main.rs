mod app;
mod attachments;
mod backend;
mod clipboard;
mod commands;
mod commit;
mod composer;
mod config;
mod exec;
mod history;
mod markdown;
mod mentions;
mod picker;
mod project;
mod setup;
mod store;
mod term;
mod text;
mod thread;
mod update;
mod usage;
mod weave;
mod workspace;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use crossterm::event::KeyEvent;
use quill_config::{ModelRef, ServerKind};
use quill_core::{Access, Event};
use quill_providers::{Model, Props};

pub enum Msg {
    Key(KeyEvent),
    Paste(String),
    Resize,
    Interrupt,
    Session(u64, Event),
    Probed { generation: usize, id: String, found: backend::Found },
    Props { model: ModelRef, props: Props },
    Repo(app::Repo),
    Job(commit::Job),
    Detected(Vec<(ServerKind, String)>),
    Connected(Result<Vec<Model>, String>),
    Releases(Result<Vec<quill_git::github::Release>, String>),
    Scroll(term::Scroll),
}

#[derive(Clone, Copy, ValueEnum)]
enum Mode {
    Ask,
    Edits,
    Auto,
}

impl From<Mode> for Access {
    fn from(mode: Mode) -> Access {
        match mode {
            Mode::Ask => Access::Supervised,
            Mode::Edits => Access::AutoEdits,
            Mode::Auto => Access::Full,
        }
    }
}

#[derive(Parser)]
#[command(
    name = "quill",
    bin_name = "quill",
    version,
    about = "Quill Code, a coding agent for your terminal that runs on your own models.",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[arg(short = 'C', long = "cd", value_name = "DIR", global = true, help = "Work in this folder instead of the current one")]
    dir: Option<PathBuf>,
    #[arg(short = 'c', long = "continue", help = "Continue the latest thread in this folder")]
    resume: bool,
    #[arg(short = 'w', long = "worktree", help = "Start the new thread in an isolated copy (a git worktree)")]
    isolate: bool,
    #[arg(value_name = "PROMPT", help = "Start with this prompt")]
    prompt: Vec<String>,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Run one task without the interactive screen; text goes to stdout, progress to stderr")]
    Exec {
        #[arg(value_name = "TASK", help = "The task, or - to read it from standard input")]
        prompt: Vec<String>,
        #[arg(short, long, value_name = "PROVIDER/MODEL", help = "Use this model instead of the selected one")]
        model: Option<String>,
        #[arg(short, long, value_enum, help = "How much the agent may do; anything that would ask is declined")]
        access: Option<Mode>,
        #[arg(long, help = "Print every event as a line of JSON")]
        json: bool,
        #[arg(short = 'c', long = "continue", help = "Continue the latest thread in this folder")]
        resume: bool,
        #[arg(short = 'w', long = "worktree", help = "Work in an isolated copy (a git worktree)")]
        isolate: bool,
    },
    #[command(about = "List model servers and the models they offer")]
    Models {
        #[arg(long, help = "Look for model servers on this machine and add them")]
        scan: bool,
    },
    #[command(about = "How many tokens Quill Code has used, today and over time")]
    Usage,
    #[command(about = "Update Quill Code to the newest release, or switch to the one you name")]
    Update {
        #[arg(value_name = "VERSION", help = "Install this release instead of the newest, for example 1.0.0")]
        version: Option<String>,
    },
    #[command(about = "List every published release of Quill Code")]
    Versions,
    #[command(about = "Where Quill Code keeps its settings, and ways to change them")]
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
}

#[derive(Subcommand, Clone, Copy)]
enum ConfigAction {
    #[command(about = "Open settings.json in your editor")]
    Edit,
    #[command(about = "Open your personal instructions, which every thread reads")]
    Instructions,
    #[command(about = "Reset every setting except your providers and models")]
    Reset,
}

fn main() -> ExitCode {
    update::tidy();
    let cli = Cli::parse();
    let root = match &cli.dir {
        Some(dir) => project::absolute(dir),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    if !root.is_dir() {
        eprintln!("{} is not a folder.", root.display());
        return ExitCode::from(2);
    }
    let code = match cli.command {
        Some(Commands::Exec { prompt, model, access, json, resume, isolate }) => match exec::read_prompt(&prompt) {
            Ok(prompt) => exec::run(exec::Options { prompt, root, model, access: access.map(Access::from), json, resume, isolate }),
            Err(error) => {
                eprintln!("{error}");
                2
            }
        },
        Some(Commands::Models { scan }) => commands::models(scan),
        Some(Commands::Usage) => commands::usage(),
        Some(Commands::Update { version }) => update::run(version),
        Some(Commands::Versions) => update::versions(),
        Some(Commands::Config { action: None }) => commands::show_config(),
        Some(Commands::Config { action: Some(ConfigAction::Edit) }) => commands::edit_config(),
        Some(Commands::Config { action: Some(ConfigAction::Instructions) }) => commands::edit_instructions(),
        Some(Commands::Config { action: Some(ConfigAction::Reset) }) => commands::reset_config(),
        None => app::run(root, cli.resume, cli.isolate, cli.prompt.join(" ")),
    };
    ExitCode::from(code)
}