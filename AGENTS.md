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
| `cargo run -p quill-agent --example probe -- <url> <model> "<task>"` | Run one agent turn headless in `%TEMP%\quill-probe` |

CI (`.github/workflows/ci.yml`, Windows) runs build, clippy, test and `cargo deny check`. All must stay green.

Releases (`.github/workflows/release.yml`) run when `main` gets a workspace version that has no release yet. The version needs a `## <version>` section in `CHANGELOG.md`, whose heading becomes the release title and whose body the notes. Windows, Linux and both macOS targets are built and attached to a draft release tagged `v<version>`, which is published by hand.

## Rules that do not bend

- **No telemetry.** No analytics, crash reporting or update pings. The app talks only to the model server the user configured, to git remotes through the user's own `git`, and to the GitHub API for what the user asked for: creating a repository, opening a pull request, and reading pull request status for branches whose origin is on GitHub. `deny.toml` bans the usual telemetry crates; do not add network calls anywhere else.
- **Permissive licences only.** The project is MIT. Dependencies must pass `cargo deny check`; the only exceptions are build-time `option-ext` (MPL-2.0) and the `webpki-root-certs` data crate (CDLA-Permissive-2.0) that ureq's `native-tls` feature pulls in. Never depend on or copy from Zed's GPL crates (anything in the Zed repository other than `gpui`). Nothing GPL is bundled: Git features use the user's installed `git` and say "Needs Git" without it.
- **GitHub sign-in is Git's.** The app never stores a token. `github.rs` asks `git credential fill` for one, so Git Credential Manager signs the user in the first time they publish. Background checks pass `GCM_INTERACTIVE=never` so they can never pop a sign-in window.
- **Models are detected, not configured.** `/v1/models` gives the list, status, vision and the window size (llama.cpp `meta.n_ctx` or `--ctx-size`, vLLM `max_model_len`, LM Studio `loaded_context_length`, then `context_length`). `/props?model=<id>` gives llama.cpp's template capabilities; reading it for an unloaded model loads it, so it is only read for the selected model.

## Layout

```
crates/
  quill-llama/   OpenAI-compatible client tuned for llama.cpp: models, props, streaming chat with tool-call deltas and usage
  quill-agent/   the agent engine, no UI
    lib.rs        agent loop (stream, collect tool calls, approve, execute, repeat), Access, Approval, Publisher
    prompt.rs     Quill's system prompt: identity, workspace, tools, native shell guidance, workflow, git, reply
    tools.rs      list/read/search/write/edit/run plus commit_changes and open_pull_request, path guard, aliases, shell runner
    diff.rs       line diffs grouped into hunks
    instructions.rs  AGENTS.md, CLAUDE.md (with @imports), GEMINI.md, copilot instructions
    examples/probe.rs  headless agent run against a real server
  quill-weave/   the weave mark's maths
  quill-code/    the desktop app
    main.rs       startup: fonts, assets, config, palette, window
    shell.rs      window: title bar, sidebar timeline, layout, menus, toasts, repo and pull request caches
    workspace.rs  model: threads, runs (several at once), approvals, isolated copies, persistence hooks
    thread.rs     Thread, Turn and Item data, summaries, history repair (serde, tested)
    thread_view.rs  transcript: bubbles with image previews, step rows, final answer, changes card
    composer.rs   prompt box: chips, slash commands, @ mentions, attachments, context ring, approval card, workspace strip
    context_ring.rs  context gauge and its label
    commit_dialog.rs  commit, new branch, push, pull request page, publish to GitHub
    publish.rs    the app's Publisher: what commit_changes and open_pull_request do
    git.rs        status snapshot, commit, branch, push, worktrees, remote parsing
    github.rs     repository and pull request API through Git's stored credential
    terminal.rs   PowerShell drawer
    panel.rs      right panel: Review and Files tabs
    add_project.rs  Add project sheet: start fresh (first branch, optional GitHub repo), open a folder, clone
    settings.rs   Settings: General, Models, Instructions, Projects, Version
    backend.rs    model server connection, selected model, detected thinking
    mentions.rs   slash commands, @ file index and ranking
    attachments.rs  images as data URLs, text files as fenced blocks
    project.rs    badges, create, clone, open in VS Code or Explorer
    store.rs      threads in %APPDATA%\quill-code\threads, sent images in ...\images\<thread>
    config.rs     settings in %APPDATA%\quill-code\config.json; projects in ~/.quill-code/projects, isolated copies in ~/.quill-code/worktrees
    icons.rs      the Icon enum; every SVG is embedded
    version.rs, weave.rs, palette.rs, assets.rs, ui.rs
assets/  brand/ (weave icons), icons/ (interface icons), fonts/ (Open Sans, OFL)
```

## How a turn runs

`Workspace::send` builds a `quill_agent::Task`: Quill's system prompt, the full history, the access mode and, when the workdir is a Git repository, a `Publisher` that gives the agent `commit_changes` and `open_pull_request`. It runs `quill_agent::run` on the background executor. Events come back through an unbounded channel and `Workspace::apply` turns them into `Item`s on the turn; `Usage` sets the thread's `context_used`, which the context ring shows against the model's window. Every run is keyed by thread and turn index, so a stopped run can never write into a newer turn. Anything the access mode does not allow waits on an `mpsc` reply that the approval card answers. Stop sets the run's `AtomicBool`; `repair_history` answers any tool call left without a result, because llama.cpp templates reject dangling tool calls. Threads are saved when a turn starts and finishes; a turn still running at shutdown is marked Stopped on load.

Access modes: Ask first asks before edits, commits, commands and pull requests; Trust edits asks only before commands and pull requests; Autopilot never asks. "Allow everything in this thread" raises the mode inside the running agent only.

## Sidebar

Threads are grouped into In motion (running or waiting on the user), Past day, Past week and Older. Each row shows the project badge, title and age; the project and branch; then the current commit, uncommitted file count, unpushed commit count, pull request number and state, and a live weave mark seeded from the thread's model. Repo state comes from one `git status --porcelain=v2 --branch` per workdir, cached until a run ends or the window is activated; pull request state is fetched in the background with the same invalidation.

## GPUI notes

- Any notified view redraws the whole window unless a view is embedded with `.cached(StyleRefinement::default().size_full())`. The live weave marks animate every frame, so heavy views (`ThreadView`, `Panel`, `Settings`) are cached and observe the `Workspace` model instead of being rebuilt by the shell.
- The title bar's row is a Windows drag area. Interactive elements inside it need `.occlude()` or clicks become window drags.
- Keystroke interceptors run before key bindings and are used for popup navigation, Escape, Ctrl+1 to 9 and terminal history. Calling `stop_propagation` there keeps the key from the text field.
- Rust 2024 captures every lifetime in `impl Trait` returns; helper methods that build elements return `impl IntoElement + use<>`.

## Look and feel

- One theme: the dark palette and its light counterpart, following the Windows setting live. Every colour lives in `palette.rs`; components read `palette(cx)`.
- Open Sans everywhere (`palette::FONT`), Consolas for code (`ui::MONO`).
- The weave is drawn live, never as an image, and stays monochrome; `quill-weave` is pinned by tests to exact reference frames. Model marks are the same weave seeded from the model's name.
- Interface icons are our own, drawn in one style (24 viewBox, 1.5 stroke), all in `assets/icons` and listed in `icons.rs`. Draw a new icon in that style rather than borrowing another product's.

## Conventions

- Match the surrounding code. Keep functions short and readable, avoid clever abstractions.
- Keep `cargo build` and clippy warning-free.