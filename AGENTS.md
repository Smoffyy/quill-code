## quill-code

A native desktop control surface for coding agents on local models. The assistant is always Quill; the model picker only chooses which model powers it. The workflow follows the shape the general app agents app (projects, threads, a composer with model, reasoning and access chips, approvals, step-by-step tool rows, a diff panel and a files panel), Rust, GPUI through `gpui-kit`, Windows first. MIT licensed.

## Commands

| Command | Does |
| --- | --- |
| `cargo run -p quill-code` | Build and start the app |
| `cargo test --workspace` | Every unit test |
| `cargo clippy --workspace --all-targets` | Lints, must be clean |
| `cargo fmt --all` | Formatting (`rustfmt.toml`); strip the trailing newline it adds |
| `cargo deny check` | Licence and banned-crate policy (`deny.toml`) |
| `cargo run -p quill-core --example probe -- <url> <model> "<task>"` | Run one agent turn headless in `%TEMP%\quill-probe` |

CI (`.github/workflows/ci.yml`, Windows) runs build, clippy, test and `cargo deny check`. All must stay green.

Releases (`.github/workflows/release.yml`) run when `main` gets a workspace version that has no release yet. The version needs a `## <version>` section in `CHANGELOG.md`, whose heading becomes the release title and whose body the notes. Windows, Linux and both macOS targets are built and attached to a draft release tagged `v<version>`, which is published by hand.

## Rules that do not bend

- **No telemetry.** No analytics, crash reporting or update pings. The app talks only to the model providers the user configured (local servers, OpenAI-compatible services, Anthropic), to the Claude Code program the user installed, to git remotes through the user's own `git`, and to the GitHub API for what the user asked for: creating a repository, opening a pull request, and reading pull request status for branches whose origin is on GitHub. `deny.toml` bans the usual telemetry crates; do not add network calls anywhere else.
- **Permissive licences only.** The project is MIT. Dependencies must pass `cargo deny check`; the only exceptions are build-time `option-ext` (MPL-2.0) and the `webpki-root-certs` data crate (CDLA-Permissive-2.0) that ureq's `native-tls` feature pulls in. Unmaintained-crate advisories are checked for direct dependencies only (`unmaintained = "workspace"`), because gpui's own tree carries a few with no upgrade path. Never depend on or copy from Zed's GPL crates (anything in the Zed repository other than `gpui`). Nothing GPL is bundled: Git features use the user's installed `git` and say "Needs Git" without it.
- **GitHub sign-in is Git's.** The app never stores a token. `github.rs` asks `git credential fill` for one, so Git Credential Manager signs the user in the first time they publish. Background checks pass `GCM_INTERACTIVE=never` so they can never pop a sign-in window.
- **Models are detected, not configured.** `/v1/models` gives the list, status, vision and the window size (llama.cpp `meta.n_ctx` or `--ctx-size`, vLLM `max_model_len`, LM Studio `loaded_context_length`, then `context_length`). `/props?model=<id>` gives llama.cpp's template capabilities; reading it for an unloaded model loads it, so it is only read for the selected model. Anthropic's `/v1/models` gives window, output limit, vision, effort levels and server-side compaction. Claude Code models are the aliases listed for it (`fable`, `opus`, `sonnet`, `haiku`); the exact version each one resolves to is read from Claude Code's init event and cached in `%APPDATA%\quill-code\claude-models.json`, so the picker says "Claude Haiku 5.5". Settings only fill in what a provider does not report.
- **Keys live in the system keychain.** API keys are stored with the `keyring` crate in Windows Credential Manager under the service `quill-code` and the provider's id, never in `settings.json` or a thread. `OPENAI_API_KEY` and `ANTHROPIC_API_KEY` are used only for providers pointing at the official addresses. Claude Code always runs on the user's own sign-in: Quill never reads its credentials and removes `ANTHROPIC_API_KEY` from its environment so a stray key does not bill the API instead of the user's plan.

## Layout

```
crates/
  quill-providers/  model access, no UI: the Provider trait, Message, Effort, streaming events with tool-call deltas, usage, stop reasons
    openai.rs     OpenAI-compatible chat completions: llama.cpp and other local servers (props, template thinking), and cloud
                  services with a Bearer key and reasoning_effort
    anthropic.rs  the Messages API over raw HTTP: Models API, adaptive thinking with effort, tool use with eager input
                  streaming, prompt caching, refusal fallbacks, server-side compaction, native content replay
    message.rs    chat messages with images and tool calls, stored in OpenAI shape plus the provider's native content
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
  quill-config/  typed settings in layers (defaults, %APPDATA%\quill-code\settings.json, a project's .quill/settings.json),
                 migration from the old config.json, limits and rules for the engine, app paths;
                 models.rs (providers, model references, role models, per-model overrides), secrets.rs (API keys in the keychain)
  quill-weave/   the weave mark's maths
  quill-code/    the desktop app
    main.rs       startup: fonts, assets, config, palette, window
    shell.rs      window: title bar, sidebar timeline, layout, menus, toasts, repo and pull request caches
    workspace.rs  model: threads, one session per thread, runs (several at once), approvals, questions, steering, isolated copies
    thread.rs     Thread, Turn and Item data (tools, plans, questions, steers, notes), summaries, history repair, v1 loading
    thread_view.rs  transcript: bubbles, live activity header, grouped exploring, timed tool rows with live output, folding
                  reasoning, plan card, final answer with the weave beside the latest, changes card, try again
    composer.rs   prompt box: pill chips, slash commands, @ mentions, attachments, context ring, approval card (diff preview,
                  Enter allows, Esc denies), question card, workspace strip
    motion.rs     animation helpers and the motion setting
    context_ring.rs  context gauge and its label
    commit_dialog.rs  commit, new branch, push, pull request page, publish to GitHub
    terminal.rs   PowerShell drawer
    panel.rs      right panel: Review, Files and Usage tabs
    usage.rs      usage log (%APPDATA%\quill-code\usage.jsonl, one line per finished turn, seeded from existing threads)
                  and the Usage tab: this chat's cost and tokens per turn, totals for today, 7 and 30 days, all time, by model
    add_project.rs  Add project sheet: start fresh (first branch, optional GitHub repo), open a folder, clone
    settings.rs   Settings, built on gpui-component's settings pages: General, Models, Agent, Tools, Permissions, Instructions,
                  Projects, Advanced, Version; every field reads and writes the settings global, with search and reset
    backend.rs    every provider's connection and models, the selected model, its reasoning control (on/off or effort),
                  engines and one-shot writers for a model, local server scan, learned Claude versions
    model_settings.rs  Settings, Models: provider cards (address, key, on/off), add and find providers, effort, side-job models,
                  overrides for the model in use
    mentions.rs   slash commands, @ file index and ranking
    attachments.rs  images as data URLs, text files as fenced blocks
    project.rs    badges, create, clone, open in VS Code or Explorer
    store.rs      threads in %APPDATA%\quill-code\threads, sent images in ...\images\<thread>
    config.rs     the settings global: load, save on every change, reload when the file changes, refuse to save over a broken file
    icons.rs      the Icon enum; every SVG is embedded
    version.rs, weave.rs, palette.rs, assets.rs, ui.rs
assets/  brand/ (weave icons), icons/ (interface icons), fonts/ (Open Sans, OFL)
```

## How a turn runs

Each thread gets a `quill_core` session the first time it sends, started with the thread's history. `Workspace::send` builds a `TurnContext` (engine, model, summariser, thinking or effort, output limit, server compaction, context window, workdir, access, toolsets, personal instructions, limits, rules) and sends `Op::Turn` with a fresh cancel flag. The session owns the history: it pushes every message itself and reports it with `Event::Message`, which the workspace mirrors into the thread for saving. Events come back through an unbounded channel and `Workspace::apply` turns them into `Item`s; every turn event carries its turn index, so a stopped run can never write into a newer turn. Messages typed while a run is going become `Op::Steer` and are added before the next step. A tool is prepared first (so edits have their diff), then the policy decides; anything that needs the user emits `ApprovalRequested` and waits for `Op::Decide`, and `ask_user` waits for `Op::Answer`. Stop sets the cancel flag; the engine answers every open tool call, because llama.cpp templates reject dangling tool calls. A new turn sent while a stopped one is still winding down queues behind it. When the window fills past the compaction threshold, earlier turns are summarised and old tool output is trimmed. Threads are saved when a turn starts and finishes; a turn still running at shutdown is marked Stopped on load.

With Claude Code as the engine, Claude Code runs the loop and its own tools; Quill turns its stream into the same events (text, tool rows with diffs, plan, questions, usage, cost, context window) and mirrors the messages into the thread's history, so another model can carry on in the same thread. Each permission prompt goes through Quill's policy and approval card. Ask first runs Claude Code in `manual` mode, the other presets in `acceptEdits`. The session id is saved on the thread and resumed with `--resume`. If another model answered in between, a fresh Claude session starts with a transcript of the thread. Messages sent while it works are sent after its current reply, stop is a control-request interrupt, and cost comes from its result.

Access modes: Ask first asks before edits, commands, commits and pull requests; Trust edits makes edits and runs read-only commands on its own; Autopilot never asks, except before fetching from the web. Rules (allow, ask or deny a tool, optionally for a command prefix) override the preset; the approval card's "Always allow" choices add rules for the session, and "Allow everything in this thread" raises the session to Autopilot.

## Settings

Everything that can be tuned is a typed field in `quill_config::Settings`, saved as `settings.json` and shown in Settings; nothing tunable is hard-coded elsewhere. A turn uses the user's settings with the project's `.quill/settings.json` merged on top (objects merge, values and lists replace), so a project can set its own rules, tools or limits. `Settings::limits()` turns them into the engine's `Limits`. A new setting needs a field with a default in `quill-config` and an item in `settings.rs`.

## Usage and cost

Every model call reports `Tokens` (prompt, completion, cache read, cache write), summed on the turn. Cost comes from Claude Code's own result, from Anthropic's published prices (`quill_config::default_price`, cache-aware) for Anthropic API models, or from a price the user sets per model in Settings, Models; local models are free. Each finished or stopped turn is appended to the usage log, so overall totals survive deleting threads. The chat's total shows on its sidebar row and on the toolbar's Usage button. Claude Code models are named with the version their alias points to (`quill_config::claude_latest`), replaced by the version Claude Code reports once it runs.

## Sidebar

Threads are grouped into In motion (running or waiting on the user), Past day, Past week and Older. Each row shows the project badge, title and age; the project and branch; then the current commit, uncommitted file count, unpushed commit count, pull request number and state, and a live weave mark seeded from the thread's model. Repo state comes from one `git status --porcelain=v2 --branch` per workdir, cached until a run ends or the window is activated; pull request state is fetched in the background with the same invalidation.

## GPUI notes

- Any notified view redraws the whole window unless a view is embedded with `.cached(StyleRefinement::default().size_full())`. The live weave marks animate every frame, so heavy views (`ThreadView`, `Panel`, `Settings`) are cached and observe the `Workspace` model instead of being rebuilt by the shell.
- The title bar's row is a Windows drag area. Interactive elements inside it need `.occlude()` or clicks become window drags.
- Keystroke interceptors run before key bindings and are used for popup navigation, Escape, Ctrl+1 to 9 and terminal history. Calling `stop_propagation` there keeps the key from the text field.
- Rust 2024 captures every lifetime in `impl Trait` returns; helper methods that build elements return `impl IntoElement + use<>`.

## Look and feel

- One theme in two modes: the ink palette, dark and light, following Windows live unless Appearance pins one. Every colour lives in `palette.rs` (surfaces `sidebar`, `bg`, `raised`, `menu_bg`; one `accent` with `accent_soft` and `accent_text`); components read `palette(cx)`.
- Primary actions use the accent (`ui::button` with `Tone::Primary`); secondary, ghost and danger tones cover the rest. Menus, cards and the composer are rounded (10 to 20 px) with a 1 px border; the composer gets an accent ring when focused.
- Motion lives in `motion.rs`: `rise` for things that appear, `pop` for menus, `spin` for working rows, `shimmer` for live labels, springs for the sidebar width and the panel tabs. Appearance's Motion setting scales every duration and Off sets GPUI's reduce-motion.
- The title bar holds the sidebar toggle, breadcrumbs and a toolbar (Open in, Commit, Review, Files, Terminal); there is no floating card. Ctrl K opens the command launcher, Ctrl N a new thread, Ctrl B the sidebar, Ctrl , settings, Ctrl ` the terminal.
- Open Sans everywhere (`palette::FONT`), Consolas for code (`ui::MONO`).
- The weave is drawn live, never as an image, and stays monochrome; `quill-weave` is pinned by tests to exact reference frames. Model marks are the same weave seeded from the model's name.
- Interface icons are our own, drawn in one style (24 viewBox, 1.5 stroke), all in `assets/icons` and listed in `icons.rs`. Draw a new icon in that style rather than borrowing another product's. The two exceptions are `claude.svg` and `openai.svg`, the providers' own marks (from Simple Icons), shown only in the model picker and Settings to say which backend a model comes from; in a chat the model is shown by the weave. Local servers have no mark, so their models keep the weave in the picker.

## Conventions

- Match the surrounding code. Keep functions short and readable, avoid clever abstractions.
- Keep `cargo build` and clippy warning-free.