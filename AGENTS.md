## quill-code

A terminal coding agent for local models. The binary is `quill`; the assistant is always Quill, and the model picker only chooses which model powers it. It is a full-screen terminal program: the conversation scrolls inside Quill above a live area that holds the activity line, the prompt, approvals, pickers and a status line, and the session is printed into the terminal's scrollback on exit. Rust and crossterm, for Windows, Linux and macOS. MIT licensed.

## Commands

| Command | Does |
| --- | --- |
| `cargo run -p quill-code` | Build and start `quill` in the current folder |
| `cargo run -p quill-code -- exec "<task>"` | One headless turn: text to stdout, progress to stderr, `--json` for the event stream |
| `cargo test --workspace` | Every unit test |
| `cargo clippy --workspace --all-targets` | Lints, must be clean |
| `cargo fmt --all` | Formatting (`rustfmt.toml`); strip the trailing newline it adds |
| `cargo deny check` | Licence and banned-crate policy (`deny.toml`) |
| `cargo run -p quill-core --example probe -- <url> <model> "<task>"` | Run one agent turn against a raw server address in `%TEMP%\quill-probe` |
| `vhs docs/demo/demo.tape` | Record the README's demo GIF again (`tour.tape` for its screenshots); needs VHS, ttyd and a running model |

CI (`.github/workflows/ci.yml`) builds, lints with `-D warnings` and tests on Windows, Linux and macOS, packs and installs a debug build with both install scripts, and runs `cargo deny check` once. All must stay green.

Releases (`.github/workflows/release.yml`) run on every push to `main` whose workspace version has no published release. The version needs a `## [<version>] - <YYYY-MM-DD>` section in `CHANGELOG.md`, whose body becomes the release notes. Windows (x64), Linux (x64 and ARM, static musl) and both macOS targets are built, packed by `scripts/package.sh` as `quill-<version>-<target>` with a checksum, and attached with `SHA256SUMS`, `install.sh` and `install.ps1` to a draft release named `<version>` and tagged `v<version>`. Its notes are the changelog section and the install commands, followed by GitHub's generated "What's Changed" and "Full Changelog". Merging again with the same version rebuilds and replaces the draft. It is published by hand, which runs `verify-release.yml`: it checks the release's attestation with `gh release verify` and `SHA256SUMS` against it with `gh release verify-asset`, installs the release on every system and checks `quill --version`, `quill update` and `quill versions`. The repository has immutable releases turned on, so a published release's files and tag can never change: everything must be attached while it is a draft, and a broken release is fixed with a new version. Keep the archive names and `SHA256SUMS` as they are, because `quill update <version>` installs old releases with the newest installer.

## Rules that do not bend

- **No telemetry.** No analytics, crash reporting or update pings. quill talks only to the model servers the user configured, to git remotes through the user's own `git`, and to GitHub for what the user asked for: creating a repository, opening a pull request, reading pull request status for branches whose origin is on GitHub, and listing or downloading releases for `quill update`, `quill versions` and `/version`. `deny.toml` bans the usual telemetry crates; do not add network calls anywhere else.
- **Permissive licences only.** The project is MIT. Dependencies must pass `cargo deny check` for every release target; the exceptions are the `webpki-roots` certificate data (CDLA-Permissive-2.0) that ureq's rustls pulls in, and `clipboard-win` and `error-code` (BSL-1.0) that arboard's Windows clipboard pulls in. Unmaintained-crate advisories are checked for direct dependencies only (`unmaintained = "workspace"`). Nothing GPL is bundled: Git features use the user's installed `git` and say so when it is missing.
- **GitHub sign-in is Git's.** quill never stores a token. `github.rs` asks `git credential fill` for one, so Git Credential Manager signs the user in the first time they publish. Background checks pass `GCM_INTERACTIVE=never` so they can never pop a sign-in window, and `GIT_TERMINAL_PROMPT=0` keeps Git from prompting inside the screen.
- **Models are detected, not configured.** Each server kind is read in its own way: OpenAI-compatible `/v1/models` gives the list, status and vision, and the window from llama.cpp's `meta.n_ctx` or `--ctx-size`, vLLM's `max_model_len`, then `loaded_context_length` or `context_length`; LM Studio's `/api/v1/models` gives loaded instances, their context and vision; Ollama's `/api/tags` and `/api/show` give capabilities and context length, and models without `completion` are left out. llama.cpp's `/props?model=<id>` gives the template's tool and vision support; reading it for an unloaded model loads it, so it is only read for the selected model once it is loaded or used. How a model controls its reasoning comes from `quill_providers::profiles` and the user's `models.profiles`. Settings (`models.overrides`) only fill in what a server does not report.
- **Keys stay with the server they belong to.** A server that needs a key gets one during setup or `/connect` (every kind except Ollama and GPT4All asks); it is saved as the provider's `key` in `settings.json` and sent only to that provider's address as a Bearer token.

## Layout

```
crates/
  quill-providers/  model access, no UI: the Provider trait, Model, Props, ChatRequest, streaming events with tool-call
                    deltas, Tokens, stop reasons
    openai.rs     OpenAI-compatible chat completions for llama.cpp, vLLM, Jan, GPT4All and the rest; /props
    lmstudio.rs   LM Studio's model list on top of the OpenAI-compatible chat
    ollama.rs     Ollama's native API: tags, show, chat
    profiles.rs   model families and how they control reasoning (effort, a toggle kwarg, or none), Tuning per request
    message.rs    chat messages with images, tool calls and reasoning (sent as reasoning_content, kept in the saved history)
  quill-core/    the agent engine, no UI
    protocol.rs   Event (serde, tagged) and the shared types: ToolView, Approval, Decision, PlanStep, Question, Outcome
    session.rs    one thread per session: Op queue (Turn, Steer, Decide, Answer, Compact, Shutdown), inbox, history,
                  TurnContext
    turn.rs       the turn loop: stream, collect calls, admit (prepare, policy, approval), run read-only batches in parallel
    policy.rs     Access presets, Effect, Rule and verdicts, read-only command detection, discards_work
    context.rs    token estimate, auto-compaction (summary under fixed headings, recent files read back), trimming old tool output
    options.rs    Limits, Toolsets and Shell, with their defaults
    prompt.rs     Quill's system prompt, built from the enabled tools
    tools/        the Tool trait and registry: find_files, grep, read_file, write_file, edit_file, shell, update_plan,
                  ask_user, commit_changes, open_pull_request; aliases, lenient arguments, path guard, .editorconfig
    diff.rs       line diffs grouped into hunks
    instructions.rs  the project's instruction files (AGENTS.md, CLAUDE.md with @imports, GEMINI.md, copilot instructions)
    examples/probe.rs  headless agent run against a real server
  quill-git/     the user's git: status snapshot, commit, branch, push, worktrees, remote parsing; github.rs for the GitHub API
  quill-config/  typed settings in layers (defaults, settings.json in the settings folder, a project's .quill/settings.json),
                 limits and rules for the engine, paths per system; models.rs (server kinds, providers, model references,
                 role models, overrides)
  quill-weave/   the weave mark's maths
  quill-code/    the `quill` binary
    main.rs       the command line (clap): the screen, exec, models, usage, update, versions, config; Msg, the one message
                  type every background thread reports through
    app.rs        the screen: keys, pastes, scrolling, session events and probes in one state machine; slash commands,
                  pickers, the trust prompt, approval and question cards, status line, the event loop
    term.rs       raw mode, the alternate screen, Layout::frame (transcript plus live area, row by row, scroll offset),
                  the input thread with paste detection, suspending the screen for an editor
    history.rs    turns as styled lines: prompt, tool rows with timings, grouped reads, searches and commands, diffs, command
                  output, plan, questions, reasoning, the ending; Progress adds each piece once it settles
    markdown.rs   markdown to wrapped lines: headings, lists, quotes, code blocks, tables, links
    text.rs       styled spans and lines, word and character wrapping, truncation, cleaning terminal noise from output
    composer.rs   the prompt editor: multi-line editing, word moves, prompt history, pasted paths as attachments
    picker.rs     a list picker with filtering, used by /model, /effort, /mode, /resume, /branch, /version, the trust prompt
                  and the commit flow
    setup.rs      the first run and /connect: which app runs the models, servers already running, address and key, models
    commit.rs     /commit: files, branch, message (the model writes it when empty), commit, push, pull request page, publish
    exec.rs       quill exec: one headless turn; approvals declined, questions answered "decide for yourself", Ctrl+C stops
    update.rs     quill update, quill versions and /version: GitHub releases, running the release's install script
    commands.rs   quill models, usage and config
    workspace.rs  threads, one session at a time, the run, approvals, questions, steering, isolated copies
    backend.rs    every server's connection and models, the selected model, its reasoning control (toggle or effort),
                  one-shot writers for a model, local server scan, props
    thread.rs     Thread, Turn and Item data (tools, plans, questions, steers, notes), history repair, older formats
    usage.rs      usage log (usage.jsonl, one line per finished turn, seeded from existing threads) and the /usage report
    mentions.rs   slash commands, @ file index and ranking
    attachments.rs  images as data URLs, text files as fenced blocks
    clipboard.rs  the clipboard's image as a PNG file, for Alt+V
    store.rs      threads in the settings folder's threads/, sent images in images/<thread>
    config.rs     the settings: load, save on every change, reload when the file changes, refuse to save over a broken file
    project.rs, weave.rs (the weave drawn in braille)
brand/       app icons and favicons (the weave mark)
docs/        images/ for the README; demo/ with the VHS tapes, setup.ps1 and the sample project they record
scripts/     package.sh packs a release archive
install.sh, install.ps1  the one-line installers
```

## How a turn runs

Each thread gets a `quill_core` session the first time it sends, started with the thread's history; the screen keeps one session at a time and shuts it down when another thread is opened. `Workspace::send` builds a `TurnContext` (provider, model, summariser, tuning, context window, workdir, access, toolsets, personal and project instructions, limits, rules) and sends `Op::Turn` with a fresh cancel flag. A prompt sent while the selected server is still being probed, or while a stopped turn is still winding down, is queued and sent once it can be. The session owns the history: it pushes every message itself and reports it with `Event::Message`, which the workspace mirrors into the thread for saving. Events come back through the `Msg` channel and `Workspace::apply` turns them into `Item`s; every turn event carries its turn index, so a stopped run can never write into a newer turn. `history::Progress` then adds each item to the transcript once it settles: a tool when it ends, text paragraph by paragraph (never inside an open code fence), a run of reads, searches and commands when it is over, and finally the outcome, the changed files and a footer. Whatever has not settled is drawn in the live area.

Messages typed while a run is going become `Op::Steer` and are added before the next step. A tool is prepared first (so edits have their diff), then the policy decides; anything that needs the user emits `ApprovalRequested` and waits for `Op::Decide`, and `ask_user` waits for `Op::Answer`. Stop (Esc, or Ctrl+C) sets the cancel flag; a running command is killed with everything it started, and the engine answers every open tool call, because llama.cpp templates reject dangling tool calls. When the window fills past the compaction threshold, old tool output is trimmed and then earlier turns are summarised, keeping the request and its latest steps word for word and reading the most recent files back from disk; a turn compacts at most three times. `/compact` does it on demand and says why when there is nothing to compact (`Event::CompactFailed`). Threads are saved when a turn starts and finishes; a turn still running at shutdown is marked Stopped on load.

Access modes: Ask first runs read-only commands on its own (`permissions.safe_commands`, with no redirection or substitution) and asks before edits, other commands, commits and pull requests; Trust edits also makes edits on its own; Autopilot never asks. Commands that throw work away (`git checkout -- <path>`, `git restore`, `git reset --hard`, `git clean`, dropping stashes, a forced push, recursive deletes; `policy::discards_work`) ask in every mode unless a rule allows them. Rules (allow, ask or deny a tool, optionally for a command prefix) override the preset, the last matching one winning; the approval card's "Always allow" choices add rules for the session, and "Allow everything in this thread" raises the session to Autopilot. `quill exec --access` overrides the mode for one run, project settings included.

## Settings

Everything that can be tuned is a typed field in `quill_config::Settings` (`general`, `appearance`, `models`, `agent`, `tools`, `permissions`), saved as `settings.json` in the settings folder (`%APPDATA%\quill-code` on Windows, `~/Library/Application Support/quill-code` on macOS, `$XDG_CONFIG_HOME/quill-code` or `~/.config/quill-code` on Linux); nothing tunable is hard-coded elsewhere. Saving keeps keys this version does not know, so going back and forward between versions loses nothing. Personal instructions live in `~/.quill-code/AGENTS.md` and isolated copies in `~/.quill-code/worktrees` unless `general.worktrees_dir` says otherwise. The screen changes the common settings itself (`/model` and its pins, `/effort`, `/mode`, `/connect`, the trust prompt), and `quill config edit` opens the file in `VISUAL`, `EDITOR` or the system's editor. A turn uses the user's settings with the project's `.quill/settings.json` merged on top (objects merge, values and lists replace), so a project can set its own rules, tools or limits. `Settings::limits()` turns them into the engine's `Limits`. A new setting needs a field with a default in `quill-config`; give it a slash command too if people will change it often.

## Usage

Every model call reports `Tokens` (prompt, completion, cache read), summed on the turn and shown in its footer. Each finished or stopped turn is appended to `usage.jsonl` in the settings folder, so overall totals survive deleting threads; `/usage` and `quill usage` print them for today, 7 days, 30 days and all time.

## Status line

The last line of the screen shows the access mode, the model, its reasoning setting (thinking or direct, or the effort level), the project (and "(isolated)" for a worktree) with its branch, uncommitted (●) and unpushed (↑) counts, the pull request number and state, the share of the context window used with "compacts soon" when it is close, and key hints when there is room. Repo state comes from one `git status --porcelain=v2 --branch` on a background thread, refreshed when the screen starts, when a run ends, and after commits, branch changes and opening a thread; pull request state is fetched after it with the same refresh.

## Terminal notes

- The screen uses the alternate screen with mouse capture and bracketed paste. `term::Layout::frame` keeps the whole transcript, picks the rows that fit above the live area for the current scroll offset, and repaints only the rows that changed, inside synchronized updates. While scrolled up, the last row says how many lines are below. `Screen::finish` prints the transcript into the normal scrollback on exit, and `Screen::suspend` leaves the screen while an editor runs.
- Keep every line narrower than the terminal (wrap to `Layout::room()`); `frame` truncates anything longer, which cuts text off.
- crossterm on Windows reports a resize as the console buffer size plus one, so the size is read again with `terminal::size()`. Windows also reports key releases; they are dropped in `term::translate`.
- Windows consoles deliver a paste as a burst of key events, so `term::translate` turns a burst of typed keys into one paste, where Enter is a new line rather than a send.
- crossterm drops colours when `NO_COLOR` is set; unset it when recording the demo or checking colours.
- Child processes run with stdin closed and their output piped (and `CREATE_NO_WINDOW` on Windows), so they never read the keyboard or draw over the screen. Commands run in the shell from `tools.shell` (Windows PowerShell, pwsh, cmd, bash, which is Git Bash on Windows, or sh). Stopping a command kills its whole tree: `taskkill /T` on Windows, the command's own process group on Unix.
- Every line is built by pure functions (`history`, `markdown`, `composer`, `picker`, `setup`), so they are tested without a terminal, and `term.rs` checks its frames against a small virtual terminal.
- Rust 2024 captures every lifetime in `impl Trait` returns; keep helpers returning owned lines.

## Look and feel

- Colours come from the terminal's own palette, so the screen follows any light or dark theme: `text::accent`, `good`, `bad` and `warn` are the dark ANSI cyan, green, red and yellow, plus `dim` and `bold`; diff lines are shaded with 256-colour backgrounds. Never hard-code RGB.
- Prompts start with `›`. Tool rows look like Claude Code's: a `●` (green when done, red when failed, a braille spinner while running) before `Name(target)` with the whole target wrapped, and the result hangs under `└`. A run of finished reads, searches and shell commands collapses into one dim summary line. List selections are marked `▸` and the current choice `✓`. Diffs show one line number with additions in green and removals in red. Notices start with `·`, problems with `!`.
- A dim rule separates the conversation from the prompt; the status line sits at the bottom.
- The weave is drawn live from `quill-weave` as braille on the welcome banner and stays monochrome; `quill-weave` is pinned by tests to exact reference frames.
- Keys: Enter sends (and steers while Quill works), Shift+Enter, Alt+Enter or Ctrl+J add a line, Shift+Tab switches the access mode, Ctrl+O shows thoughts in full or as a summary, Page Up, Page Down and the mouse wheel scroll (Ctrl+Home and Ctrl+End jump), Esc stops or closes, Ctrl+C clears, stops, or quits when pressed twice, Ctrl+D quits on an empty prompt, ↑ and ↓ recall earlier prompts, Tab takes a suggestion, Alt+V attaches an image from the clipboard.

## Conventions

- Match the surrounding code. Keep functions short and readable, avoid clever abstractions.
- Keep `cargo build` and clippy warning-free on every platform.
- Work on `dev`; `main` only receives merges for releases. Add user-facing changes to `## [Unreleased]` in `CHANGELOG.md`.