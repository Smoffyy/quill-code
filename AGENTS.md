## quill-code

A terminal coding agent for local models. The binary is `quill`; the assistant is always Quill, and the model picker only chooses which model powers it. It runs inline like other coding CLIs: finished work is printed once into the terminal's own scrollback, and a small live area at the bottom holds the activity line, the prompt, approvals, pickers and a status line. Rust and crossterm, for Windows, Linux and macOS. MIT licensed.

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

CI (`.github/workflows/ci.yml`) builds, lints and tests on Windows, Linux and macOS, and runs `cargo deny check` once. All must stay green.

Releases (`.github/workflows/release.yml`) run when `main` gets a workspace version that has no release yet. The version needs a `## <version>` section in `CHANGELOG.md`, whose heading becomes the release title and whose body the notes. Windows, Linux and both macOS targets are built, packed as `quill-<version>-<target>` with the `quill` binary, and attached to a draft release tagged `v<version>`, which is published by hand.

## Rules that do not bend

- **No telemetry.** No analytics, crash reporting or update pings. quill talks only to the model providers the user configured (local servers, OpenAI-compatible services, Anthropic), to the Claude Code program the user installed, to git remotes through the user's own `git`, and to the GitHub API for what the user asked for: creating a repository, opening a pull request, and reading pull request status for branches whose origin is on GitHub. `deny.toml` bans the usual telemetry crates; do not add network calls anywhere else.
- **Permissive licences only.** The project is MIT. Dependencies must pass `cargo deny check` for every release target; the exceptions are the `webpki-root-certs` data crate (CDLA-Permissive-2.0) that ureq's `native-tls` feature pulls in, and `clipboard-win` and `error-code` (BSL-1.0) that arboard's Windows clipboard pulls in. Unmaintained-crate advisories are checked for direct dependencies only (`unmaintained = "workspace"`). Nothing GPL is bundled: Git features use the user's installed `git` and say so when it is missing.
- **GitHub sign-in is Git's.** quill never stores a token. `github.rs` asks `git credential fill` for one, so Git Credential Manager signs the user in the first time they publish. Background checks pass `GCM_INTERACTIVE=never` so they can never pop a sign-in window, and `GIT_TERMINAL_PROMPT=0` keeps Git from prompting inside the screen.
- **Models are detected, not configured.** `/v1/models` gives the list, status, vision and the window size (llama.cpp `meta.n_ctx` or `--ctx-size`, vLLM `max_model_len`, LM Studio `loaded_context_length`, then `context_length`). `/props?model=<id>` gives llama.cpp's template capabilities; reading it for an unloaded model loads it, so it is only read for the selected model once it is loaded or used. Anthropic's `/v1/models` gives window, output limit, vision, effort levels and server-side compaction. Claude Code models are the aliases listed for it (`fable`, `opus`, `sonnet`, `haiku`); the exact version each one resolves to is read from Claude Code's init event and cached in `claude-models.json` in the settings folder, so the picker says "Claude Haiku 5.5". Settings only fill in what a provider does not report.
- **Keys live in the system keychain.** API keys are stored with the `keyring` crate (Windows Credential Manager, the macOS Keychain, the Secret Service on Linux) under the service `quill-code` and the provider's id, never in `settings.json` or a thread; `quill key set <provider>` saves one. `OPENAI_API_KEY` and `ANTHROPIC_API_KEY` are used only for providers pointing at the official addresses. Claude Code always runs on the user's own sign-in: Quill never reads its credentials and removes `ANTHROPIC_API_KEY` from its environment so a stray key does not bill the API instead of the user's plan.

## Layout

```
crates/
  quill-providers/  model access, no UI: the Provider trait, Message, Effort, streaming events with tool-call deltas, usage, stop reasons
    openai.rs     OpenAI-compatible chat completions: llama.cpp and other local servers (props, template thinking), and cloud
                  services with a Bearer key and reasoning_effort
    anthropic.rs  the Messages API over raw HTTP: Models API, adaptive thinking with effort, tool use with eager input
                  streaming, prompt caching, refusal fallbacks, server-side compaction, native content replay
    message.rs    chat messages with images, tool calls and reasoning (sent as reasoning_content, kept in the saved history), stored in OpenAI shape plus the provider's native content
  quill-core/    the agent engine, no UI
    protocol.rs   Event (serde, tagged) and the shared types: ToolView, Approval, Decision, PlanStep, Outcome
    session.rs    one thread per session: Op queue (Turn, Steer, Decide, Answer, Compact, Shutdown), inbox, history; the Engine
                  (a Provider, or Claude Code) is chosen per turn
    claude_code.rs  drives the user's `claude` CLI over stream-json: one process per thread, permission prompts through
                  Quill's policy and approval card, tool rows and diffs, TodoWrite as the plan, steering, interrupt, resume,
                  handoff of earlier history when another model ran in between
    turn.rs       the turn loop: stream, collect calls, admit (prepare, policy, approval), run read-only batches in parallel
    policy.rs     Access presets, Effect, Rule and verdicts, read-only command detection
    context.rs    token estimate, auto-compaction (summary of earlier turns), trimming old tool output
    options.rs    Limits, Toolsets and Shell, with their defaults
    prompt.rs     Quill's system prompt, built from the enabled tools
    tools/        the Tool trait and registry: list_dir, find_files, grep, read_file, write_file, edit_file, apply_patch,
                  shell, update_plan, ask_user, commit_changes, open_pull_request; aliases, lenient arguments, path guard
    diff.rs       line diffs grouped into hunks
    instructions.rs  AGENTS.md, CLAUDE.md (with @imports), GEMINI.md, copilot instructions
    examples/probe.rs  headless agent run against a real server
  quill-git/     the user's git: status snapshot, commit, branch, push, worktrees, remote parsing; github.rs for the GitHub API
  quill-config/  typed settings in layers (defaults, settings.json in the settings folder, a project's .quill/settings.json),
                 migration from the old config.json, limits and rules for the engine, app paths per system;
                 models.rs (providers, model references, role models, per-model overrides), secrets.rs (API keys in the keychain)
  quill-weave/   the weave mark's maths
  quill-code/    the `quill` binary
    main.rs       the command line (clap): the interactive screen, exec, models, key, usage, config; Msg, the one message
                  type every background thread reports through
    app.rs        the interactive screen: keys, pastes, session events and probes in one state machine; slash commands,
                  pickers, approval and question cards, status line, the event loop
    term.rs       raw mode, the inline renderer (Layout::frame rewrites the live area in place under the scrollback), the
                  input thread with paste detection, hidden key input
    history.rs    turns as styled lines: prompt, tool rows with timings, grouped reads, searches and commands, diffs, command output, plan,
                  questions, reasoning, the ending; Progress prints each piece once it settles and previews the rest live
    markdown.rs   markdown to wrapped lines: headings, lists, quotes, code blocks, tables, links
    text.rs       styled spans and lines, word and character wrapping, truncation, cleaning terminal noise from output
    composer.rs   the prompt editor: multi-line editing, word moves, prompt history, pasted paths as attachments
    picker.rs     a list picker with filtering, used by /model, /think, /mode, /resume, /branch and the commit flow
    commit.rs     /commit: files, branch, message (the model writes it when empty), commit, push, pull request page, publish
    exec.rs       quill exec: one headless turn; approvals declined, questions answered "decide for yourself", Ctrl+C stops
    commands.rs   quill models, key, usage and config
    workspace.rs  threads, one session at a time, the run, approvals, questions, steering, isolated copies
    backend.rs    every provider's connection and models, the selected model, its reasoning control (on/off or effort),
                  engines and one-shot writers for a model, local server scan, learned Claude versions
    thread.rs     Thread, Turn and Item data (tools, plans, questions, steers, notes), history repair, v1 loading
    usage.rs      usage log (usage.jsonl, one line per finished turn, seeded from existing threads) and the /usage report
    mentions.rs   slash commands, @ file index and ranking
    attachments.rs  images as data URLs, text files as fenced blocks
    clipboard.rs  the clipboard's image as a PNG file, for Alt+V
    store.rs      threads in the settings folder's threads/, sent images in images/<thread>
    config.rs     the settings: load, save on every change, reload when the file changes, refuse to save over a broken file
    project.rs, weave.rs (the weave drawn in braille)
assets/  brand/ (weave icons)
```

## How a turn runs

Each thread gets a `quill_core` session the first time it sends, started with the thread's history; the screen keeps one session at a time and shuts it down when another thread is opened. `Workspace::send` builds a `TurnContext` (engine, model, summariser, thinking or effort, output limit, server compaction, context window, workdir, access, toolsets, personal instructions, limits, rules) and sends `Op::Turn` with a fresh cancel flag. A prompt sent while the selected provider is still being probed waits until the probe answers. The session owns the history: it pushes every message itself and reports it with `Event::Message`, which the workspace mirrors into the thread for saving. Events come back through the `Msg` channel and `Workspace::apply` turns them into `Item`s; every turn event carries its turn index, so a stopped run can never write into a newer turn. `history::Progress` then prints each item into the scrollback once it settles: a tool when it ends, text paragraph by paragraph (never inside an open code fence), a run of reads, searches and commands when it is over, and finally the outcome, the changed files and a footer. Whatever has not settled is drawn in the live area.

Messages typed while a run is going become `Op::Steer` and are added before the next step. A tool is prepared first (so edits have their diff), then the policy decides; anything that needs the user emits `ApprovalRequested` and waits for `Op::Decide`, and `ask_user` waits for `Op::Answer`. Stop (Esc, or Ctrl+C) sets the cancel flag; a running command is killed with everything it started, and the engine answers every open tool call, because llama.cpp templates reject dangling tool calls. A new turn sent while a stopped one is still winding down queues behind it. When the window fills past the compaction threshold, earlier turns are summarised and old tool output is trimmed; `/compact` does it on demand and says why when there is nothing to compact (`Event::CompactFailed`). Threads are saved when a turn starts and finishes; a turn still running at shutdown is marked Stopped on load.

With Claude Code as the engine, Claude Code runs the loop and its own tools; Quill turns its stream into the same events (text, tool rows with diffs, plan, questions, usage, cost, context window) and mirrors the messages into the thread's history, so another model can carry on in the same thread. Each permission prompt goes through Quill's policy and approval card. Ask first runs Claude Code in `manual` mode, the other presets in `acceptEdits`. The session id is saved on the thread and resumed with `--resume`. If another model answered in between, a fresh Claude session starts with a transcript of the thread. Messages sent while it works are sent after its current reply, stop is a control-request interrupt, and cost comes from its result.

Access modes: Ask first runs read-only commands on its own and asks before edits, other commands, commits and pull requests; Trust edits also makes edits on its own; Autopilot never asks, except before fetching from the web. Rules (allow, ask or deny a tool, optionally for a command prefix) override the preset; the approval card's "Always allow" choices add rules for the session, and "Allow everything in this thread" raises the session to Autopilot. `quill exec --access` overrides the mode for one run, project settings included.

## Settings

Everything that can be tuned is a typed field in `quill_config::Settings`, saved as `settings.json` in the settings folder (`%APPDATA%\quill-code` on Windows, `~/Library/Application Support/quill-code` on macOS, `$XDG_CONFIG_HOME/quill-code` or `~/.config/quill-code` on Linux); nothing tunable is hard-coded elsewhere. The screen changes the common ones itself (`/model` and its pins, `/think`, `/mode`), and `quill config edit` opens the file. A turn uses the user's settings with the project's `.quill/settings.json` merged on top (objects merge, values and lists replace), so a project can set its own rules, tools or limits. `Settings::limits()` turns them into the engine's `Limits`. A new setting needs a field with a default in `quill-config`; give it a slash command too if people will change it often. Fields the desktop app had and the CLI dropped are ignored when old files load.

## Usage and cost

Every model call reports `Tokens` (prompt, completion, cache read, cache write), summed on the turn. Cost comes from Claude Code's own result, from Anthropic's published prices (`quill_config::default_price`, cache-aware) for Anthropic API models, or from a price the user sets per model in `models.overrides`; local models are free. Each finished or stopped turn is appended to the usage log, so overall totals survive deleting threads. The thread's cost shows on the status line and in each turn's footer; `/usage` and `quill usage` print the totals. Claude Code models are named with the version their alias points to (`quill_config::claude_latest`), replaced by the version Claude Code reports once it runs.

## Status line

The last line of the live area shows the model, its reasoning setting, the access mode, the project (and "isolated" for a worktree) with its branch, uncommitted (●) and unpushed (↑) counts, the pull request number and state, the share of the context window used and the thread's cost. Repo state comes from one `git status --porcelain=v2 --branch` on a background thread, refreshed when the screen starts, when a run ends, and after commits, branch changes and opening a thread; pull request state is fetched after it with the same refresh.

## Terminal notes

- The screen is inline. History lines are printed once and scroll into the terminal's scrollback; the live area below them is rewritten every frame by moving up to its top with relative cursor moves (`term::Layout::frame`), never absolute positions. Frames are built as one ANSI string, wrapped in synchronized updates, and only drawn when something changed.
- Keep every line narrower than the terminal (wrap or truncate to `Layout::room()`); a line that wraps on its own throws off the cursor arithmetic.
- crossterm on Windows reports a resize as the console buffer size plus one, so the size is read again with `terminal::size()`. Windows also reports key releases; they are dropped in `term::translate`.
- Windows consoles deliver a paste as a burst of key events, so `term::translate` turns a burst of typed keys into one paste, where Enter is a new line rather than a send.
- Child processes run with stdin closed and their output piped (and `CREATE_NO_WINDOW` on Windows), so they never read the keyboard or draw over the screen. Stopping a command kills its whole tree: `taskkill /T` on Windows, the command's own process group on Unix.
- Every line is built by pure functions (`history`, `markdown`, `composer`, `picker`), so they are tested without a terminal, and `term.rs` checks its frames against a small virtual terminal.
- Rust 2024 captures every lifetime in `impl Trait` returns; keep helpers returning owned lines.

## Look and feel

- Colours come from the terminal's own palette, so the screen follows any light or dark theme: `text::accent`, `good`, `bad` and `warn` are the dark ANSI cyan, green, red and yellow, plus `dim` and `bold`. Never hard-code RGB.
- Prompts start with `›`. Tool rows look like Claude Code's: a `●` (green when done, red when failed, a braille spinner while running) before `Name(target)` with the whole target wrapped, and the result hangs under `└`. A run of finished reads, searches and shell commands collapses into one dim summary line. List selections are marked `▸` and the current choice `✓`. Diffs show one line number with additions in green and removals in red. Notices start with `·`, problems with `!`.
- A dim rule separates the scrollback from the prompt; the status line sits at the bottom.
- The weave is drawn live from `quill-weave` as braille on the welcome banner and stays monochrome; `quill-weave` is pinned by tests to exact reference frames.
- Keys: Enter sends (and steers while Quill works), Shift+Enter, Alt+Enter or Ctrl+J add a line, Esc stops or closes, Ctrl+C clears, stops, or quits when pressed twice, Ctrl+D quits on an empty prompt, Tab takes a suggestion, Alt+V attaches an image from the clipboard.

## Conventions

- Match the surrounding code. Keep functions short and readable, avoid clever abstractions.
- Keep `cargo build` and clippy warning-free on every platform.