# Quill Code

Quill Code is a coding agent for your terminal that runs only on your own local models. You type `quill` to start it. Run a model in llama.cpp's `llama-server`, LM Studio, Ollama or vLLM, pick the model that powers Quill, and hand it work: it reads and edits your projects, runs commands in your system's own shell, shows every diff, and takes the git side all the way to a pull request.

## Install

macOS and Linux:

```
curl -fsSL https://github.com/Smoffyy/quill-code/releases/latest/download/install.sh | sh
```

Windows (PowerShell):

```
irm https://github.com/Smoffyy/quill-code/releases/latest/download/install.ps1 | iex
```

That's all: the script downloads the right build for your system, checks it against the release's checksums, puts `quill` in `~/.local/bin` (or `%LOCALAPPDATA%\Programs\quill` on Windows) and adds that folder to your PATH. No admin rights, no Rust, nothing else to install. Then open a terminal in any project and run `quill`.

The first time, Quill Code asks which app runs your models: llama.cpp, LM Studio, Ollama, vLLM, Jan, GPT4All or another OpenAI-compatible server. It looks for the ones already running, asks for an address and an API key if the app needs one, and lists the models it finds. Connect a server later with `/connect`. The first time you run it in a folder, it asks whether you trust the files there; folders inside a trusted one count as trusted.

**Update** with `quill update`. It checks GitHub only when you run it, never on its own.

**Switch versions** with `/version`, which lists every published release, installs the one you pick and reopens Quill Code on the thread you were in. From the command line, `quill versions` lists them and `quill update 1.0.0` installs that one. To install a particular release from scratch, pass its version to the one-liner:

```
curl -fsSL https://github.com/Smoffyy/quill-code/releases/latest/download/install.sh | QUILL_VERSION=1.0.0 sh
```

```powershell
$env:QUILL_VERSION = '1.0.0'; irm https://github.com/Smoffyy/quill-code/releases/latest/download/install.ps1 | iex
```

Settings and threads keep what newer versions saved in them, so going back and forward again loses nothing.

**Uninstall** by deleting `~/.local/bin/quill` (or the `%LOCALAPPDATA%\Programs\quill` folder on Windows). Your settings and threads stay in the settings folder listed by `quill config`.

The installer reads a few variables: `QUILL_VERSION=1.0.0` installs a particular release, `QUILL_INSTALL_DIR` picks another folder, and `QUILL_NO_MODIFY_PATH=1` leaves your PATH alone. Builds are made for Windows (x64; ARM machines run it through emulation), Linux (x64 and ARM, one static binary for every distribution) and macOS (Apple silicon and Intel).

## Principles

- **A full-screen terminal program.** Rust and crossterm. The conversation scrolls inside Quill (mouse wheel, Page Up and Page Down) while the prompt stays fixed at the bottom, and nothing on screen moves on its own. When you quit, the session is printed into your terminal so it stays in the scrollback. To select text while Quill runs, hold Shift as you drag.
- **No telemetry, ever.** Quill Code talks only to the model servers you configure, to your git remotes, and to GitHub to show the pull request of a branch whose remote is there, and when you publish a repository, open a pull request, run `quill update` or open `/version`.
- **MIT licensed,** with permissively licensed dependencies only.
- **Built for local models.** Tool calls, context windows and thinking controls are detected from the server rather than configured by hand.

## Using it

Run `quill` in a project folder:

```
quill                      start in this folder
quill "fix the failing test"   start with a prompt
quill -c                   continue the latest thread in this folder
quill -w                   work in an isolated copy (a git worktree) so parallel runs never collide
quill -C path/to/project   work in another folder
```

While Quill works, the live area shows what it is doing; press Esc to stop it, or type to steer it. Edits, commands, commits and pull requests ask first unless the access mode allows them.

When the conversation nears the end of the model's context window, Quill trims old command output and, if that is not enough, summarises the conversation and carries on with the task where it stopped. Your request is kept word for word, along with its latest steps. The status line says "compacts soon" when it is close; `/compact` does it on demand, and `agent.auto_compact` in the settings turns it off.

Edits keep each file's final newline, or its lack of one, unless you ask Quill to change it. New files follow `insert_final_newline` from the project's `.editorconfig` and otherwise end with a newline. Set `tools.final_newline` to `add` to give every file Quill writes a final newline, or to `remove` to end files right after their last character, except files that need one: patches, diffs, crontabs, and files the `.editorconfig` gives `insert_final_newline = true`.

| Key | Does |
| --- | --- |
| Enter | Send, or steer the reply while Quill works |
| Shift+Enter, Alt+Enter, Ctrl+J | New line |
| Shift+Tab | Switch between Ask first, Trust edits and Autopilot |
| Ctrl+O | Show thoughts in full, or as a short summary |
| Page Up, Page Down, mouse wheel | Scroll the conversation (Ctrl+Home and Ctrl+End jump to the top and bottom) |
| Esc | Stop the reply, close a menu |
| Ctrl+C | Clear the prompt or stop; press twice to quit |
| Ctrl+D | Quit when the prompt is empty |
| ↑ ↓ | Earlier prompts |
| Tab | Take the highlighted suggestion |
| `@path` | Bring a file into the prompt; images go to models that can see them |

Paste or drag a file path into the prompt to attach the file.

| Command | Does |
| --- | --- |
| `/new` | Start a fresh thread in this project |
| `/clear` | Clear the screen and start a fresh thread |
| `/resume` | Go back to an earlier thread (Ctrl+D deletes one) |
| `/model` | Pick the model that powers Quill (Ctrl+P pins it to the top) |
| `/effort` | Turn thinking on or off, or pick the effort level for models like gpt-oss |
| `/connect` | Connect another model server |
| `/mode` | Ask first, Trust edits or Autopilot (Shift+Tab switches too) |
| `/init` | Have Quill write an AGENTS.md for the project |
| `/instructions` | Edit your personal AGENTS.md, which every session reads (`/memory` works too) |
| `/compact` | Summarise the conversation to free up context |
| `/retry` | Send the last prompt again |
| `/diff` | Show everything the last reply changed |
| `/usage` | Tokens used by this thread and overall |
| `/version` | Switch to another released version |
| `/commit` | Commit, then push, open a pull request page or publish to GitHub |
| `/branch` | Switch to a branch or create one |
| `/rename <title>` | Rename the thread |
| `/help`, `/quit` | Help, and leave |

### Scripts and CI

```
quill exec "add a test for the parser"
echo "summarise the README" | quill exec -
quill exec --json "list the TODOs"          one JSON event per line
quill exec --access auto "run the tests and fix what fails"
quill exec --model local/qwen3.5-9b "explain src/main.rs"
```

The model's text goes to standard output and progress to standard error. Nobody is there to approve, so anything the access mode would ask about is declined; `--access auto` allows it. The exit code is 0 when the task finished, 1 when it failed and 130 when it was stopped.

### Models and settings

```
quill models           model servers, their status and models
quill models --scan    look for model servers on this machine and add them
quill usage            tokens used today, in 7 and 30 days, and all time
quill versions         every published release; quill update <version> switches to one
quill config           where the settings live; config edit, config instructions, config reset
```

Settings are kept in `settings.json` in `%APPDATA%\quill-code` on Windows, `~/Library/Application Support/quill-code` on macOS and `~/.config/quill-code` on Linux. A project can override any of them in `.quill/settings.json` at its root.

GitHub sign-in goes through Git Credential Manager, so Quill Code never stores a token.

### Model profiles

Models control their reasoning in different ways. Quill Code knows the common families: gpt-oss takes a `reasoning_effort` of low, medium or high; Qwen3, GLM 4.5 to 4.7 and Hunyuan turn thinking on or off with `enable_thinking`; DeepSeek V3.1 and V3.2 use `thinking`. For anything else it reads what the server reports, such as llama.cpp's chat template or Ollama's capabilities. `/effort` then offers on and off, or the effort levels.

To teach it a new model, or change how a known one is handled, add a profile to `models.profiles` in `settings.json`. The first profile whose `match` appears in the model's name wins, before the built-in ones:

```json
"profiles": [
  { "match": "my-reasoner", "reasoning": "effort", "levels": ["low", "medium", "high"] },
  { "match": "my-thinker", "except": ["instruct"], "reasoning": "toggle", "kwarg": "enable_thinking" },
  { "match": "my-coder", "reasoning": "none", "kwargs": { "my_template_variable": true } },
  { "match": "plain-model", "reasoning": "none" }
]
```

`reasoning` is `effort`, `toggle` or `none`. `kwargs` are extra chat template variables sent with every request. A project can add its own profiles in `.quill/settings.json`.

### Good to have

- **Git** on the PATH, for commits, branches and isolated copies.
- **Git Credential Manager** for GitHub sign-in. It comes with [Git for Windows](https://git-scm.com/download/win); on macOS and Linux install [Git Credential Manager](https://github.com/git-ecosystem/git-credential-manager) or any credential helper that stores a GitHub token.
- On Windows, Windows Terminal is recommended; the classic console works too.

## Building from source

For working on Quill Code itself. You need Rust 1.88 or newer from [rustup](https://rustup.rs) and a C compiler for the TLS library:

- **Windows:** the [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the **Desktop development with C++** workload.
- **macOS:** the Xcode Command Line Tools, `xcode-select --install`.
- **Linux:** `sudo apt install build-essential` (Debian, Ubuntu) or `sudo dnf install gcc` (Fedora).

Then run it in place with `cargo run -p quill-code --release`, or install your build with `cargo install --path crates/quill-code`.

## Releasing

1. Work on `dev` and try your build with `cargo run -p quill-code --release` (or run `target/release/quill` from any project). CI checks every push on Windows, Linux and macOS, including the install scripts.
2. Bump `version` in `Cargo.toml` and move the `## [Unreleased]` notes in `CHANGELOG.md` into a `## [<version>] - <YYYY-MM-DD>` section, following [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), with a link for it at the bottom. The release is titled `Quill Code v<version>`. Keep the archive names (`quill-<version>-<target>`) and `SHA256SUMS` as they are: `quill update <version>` installs old releases with the newest installer.
3. Merge `dev` into `main`. The Release workflow builds every target and creates a **draft** release with the archives, checksums and both install scripts. If any build fails, no draft is made.
4. Download the draft's files from GitHub and try them. Something wrong? Keep working on `dev` and merge again with the same version: the draft is rebuilt and replaced.
5. When it's right, press **Publish** on the draft. Only then can users get it: the install command and `quill update` only see published releases. Publishing runs the Verify release workflow, which installs the release on Windows, Linux and macOS and checks `quill --version` and `quill update`.

A version that is already published is never rebuilt; bump the version for the next release.

To try the packing and installer by hand: `sh scripts/package.sh <target> <binary> dist`, `cat dist/*.sha256 > dist/SHA256SUMS`, then run `install.sh` or `install.ps1` with `QUILL_DOWNLOAD_URL` set to the `dist` folder.

### Checks

```
cargo test --workspace
cargo clippy --workspace --all-targets
cargo deny check
```

`cargo deny` is installed with `cargo install cargo-deny`.

## Licence

MIT, see [LICENSE](LICENSE).