<br/>

<p align="center">
  <img src="brand/app-icon.svg" alt="" width="112">
</p>

<h1 align="center">Quill Code</h1>

<p align="center">
  A coding agent for your terminal that runs on your own models. Point it at llama.cpp, LM Studio, Ollama or vLLM, hand it work, and watch every step.
</p>

<p align="center">
  <a href="https://github.com/Smoffyy/quill-code/releases/latest"><img src="https://img.shields.io/badge/dynamic/json?url=https%3A%2F%2Fapi.github.com%2Frepos%2FSmoffyy%2Fquill-code%2Freleases%2Flatest&query=%24.name&label=release&labelColor=1f1f1e&color=d97757" alt="Latest release"></a>
  <a href="https://github.com/Smoffyy/quill-code/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/Smoffyy/quill-code/ci.yml?branch=dev&label=CI&labelColor=1f1f1e&logo=githubactions&logoColor=f4f3ee" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-7193f5?labelColor=1f1f1e" alt="MIT license"></a>
  <a href="https://github.com/Smoffyy/quill-code/commits/dev"><img src="https://img.shields.io/github/last-commit/Smoffyy/quill-code/dev?labelColor=1f1f1e&color=d97757&logo=git&logoColor=f4f3ee" alt="Last commit"></a>
</p>

<br/>

<p align="center">
  <img src="docs/images/demo.gif" alt="Quill Code running npm test, editing src/slug.js, running the tests again and finishing with every test passing" width="900">
</p>

<p align="center"><sub>A real session at real speed: qwen3.5-35b-a3b on llama.cpp, fixing a failing test.</sub></p>

<br/>

## Up and running

macOS and Linux:

```bash
curl -fsSL https://github.com/Smoffyy/quill-code/releases/latest/download/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://github.com/Smoffyy/quill-code/releases/latest/download/install.ps1 | iex
```

Then open a terminal in any project and run **`quill`**. That is the whole setup.

The script downloads the right build for your system, checks it against the release's checksums, puts `quill` in `~/.local/bin` (or `%LOCALAPPDATA%\Programs\quill` on Windows) and adds that folder to your PATH. No admin rights, no Rust, nothing else to install. The first time, Quill asks which app runs your models, finds the ones already running and lists their models.

<br/>

## A quick tour

<table>
<tr>
<td width="58%"><img src="docs/images/approval.png" alt="An approval card showing the diff of a rename in src/slug.js, with Allow, Always allow, Allow everything, Deny and tell Quill what to do instead"/></td>
<td width="42%" valign="middle">

### It asks before it acts

Every edit and command shows up as a card with its full diff before anything happens. Allow it once, always, or for the whole thread, or tell Quill what to do instead. **Shift+Tab** moves between Ask first, Trust edits and Autopilot, and commands that throw work away, like `git reset --hard` or a forced push, ask in every mode.

</td>
</tr>
<tr>
<td width="42%" valign="middle">

### It finds your models

Tool calls, context windows, vision and thinking controls are read from the server, not typed into a config file. `/model` lists everything your servers offer with what is loaded, `/effort` turns thinking on or off or picks a level, and the status line keeps an eye on how full the context is.

</td>
<td width="58%"><img src="docs/images/models.png" alt="The model picker listing three Qwen models on llama.cpp with their status, context size and vision support"/></td>
</tr>
<tr>
<td width="58%"><img src="docs/images/exec.png" alt="quill exec running the tests headless, fixing src/slug.js and reporting that all tests now pass"/></td>
<td width="42%" valign="middle">

### It works without you

`quill exec` runs one task headless for scripts and CI: the model's text on standard output, progress on standard error, `--json` for the event stream, and an exit code that says how it went. Or hand it to another agent and let your local model take the mechanical work.

</td>
</tr>
</table>

<br/>

## Works with

**Model servers:** llama.cpp · LM Studio · Ollama · vLLM · Jan · GPT4All · any OpenAI-compatible server

**Systems:** Windows (x64) · Linux (x64 and ARM, one static binary for every distribution) · macOS (Apple silicon and Intel)

**Git:** your own `git` for commits, branches and isolated copies, and Git Credential Manager for GitHub, so Quill never stores a token.

<br/>

## Questions people ask

<details>
<summary><b>Does it come with a model?</b></summary>
<br/>

No. Quill is the agent, the model runs in an app you choose. The easiest start is [LM Studio](https://lmstudio.ai) or [Ollama](https://ollama.com); [llama.cpp](https://github.com/ggml-org/llama.cpp)'s `llama-server` gives the most control. Pick a model that supports tool calls. Connect another server any time with `/connect`.

</details>

<details>
<summary><b>Is anything sent anywhere?</b></summary>
<br/>

Only what you set up. There is no telemetry, no analytics and no update check running on its own. Quill talks to the model servers you configure, to your git remotes, and to GitHub when you publish a repository, open a pull request, look at a branch's pull request, run `quill update` or open `/version`.

</details>

<details>
<summary><b>How much may it do on its own?</b></summary>
<br/>

That is the access mode. **Ask first** runs read-only commands by itself and asks before edits, other commands, commits and pull requests. **Trust edits** also makes edits on its own. **Autopilot** never asks. In every mode, commands that throw work away ask first unless a rule allows them. The first time you run Quill in a folder, it asks whether you trust the files there.

</details>

<details>
<summary><b>What happens when the context window fills up?</b></summary>
<br/>

Quill trims old command output first and, if that is not enough, summarises the conversation and carries on where it stopped, keeping your request and its latest steps word for word. The status line says "compacts soon" when it is close, `/compact` does it on demand, and `agent.auto_compact` turns it off.

</details>

<details>
<summary><b>How do I update or go back to an older version?</b></summary>
<br/>

`quill update` installs the newest release. `/version` lists every published release, installs the one you pick and reopens Quill on the thread you were in; `quill versions` and `quill update 1.0.0` do the same from the command line. Settings and threads keep what newer versions saved in them, so going back and forward loses nothing.

To install a particular release from scratch, set `QUILL_VERSION` for the one-liner. `QUILL_INSTALL_DIR` picks another folder and `QUILL_NO_MODIFY_PATH=1` leaves your PATH alone.

```bash
curl -fsSL https://github.com/Smoffyy/quill-code/releases/latest/download/install.sh | QUILL_VERSION=1.0.0 sh
```

```powershell
$env:QUILL_VERSION = '1.0.0'; irm https://github.com/Smoffyy/quill-code/releases/latest/download/install.ps1 | iex
```

</details>

<details>
<summary><b>How do I uninstall it?</b></summary>
<br/>

Delete `~/.local/bin/quill`, or the `%LOCALAPPDATA%\Programs\quill` folder on Windows. Your settings and threads stay in the settings folder that `quill config` shows.

</details>

<br/>

## Using it

```
quill                          start in this folder
quill "fix the failing test"   start with a prompt
quill -c                       continue the latest thread in this folder
quill -w                       work in an isolated copy (a git worktree) so parallel runs never collide
quill -C path/to/project       work in another folder
```

While Quill works, the live area shows what it is doing. Type to steer it, press **Esc** to stop it. The conversation scrolls inside Quill with the mouse wheel and Page Up and Page Down, and when you quit, the session is printed into your terminal's scrollback. Hold **Shift** while dragging to select text.

<details>
<summary><b>Keys</b></summary>
<br/>

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
| Alt+V | Attach an image from the clipboard |
| `@path` | Bring a file into the prompt; images go to models that can see them |

Paste or drag a file path into the prompt to attach the file.

</details>

<details>
<summary><b>Slash commands</b></summary>
<br/>

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

</details>

<details>
<summary><b>Scripts and CI</b></summary>
<br/>

```
quill exec "add a test for the parser"
echo "summarise the README" | quill exec -
quill exec --json "list the TODOs"          one JSON event per line
quill exec --access auto "run the tests and fix what fails"
quill exec --model local/qwen3.5-9b "explain src/main.rs"
```

Nobody is there to approve, so anything the access mode would ask about is declined; `--access auto` allows it. The exit code is 0 when the task finished, 1 when it failed and 130 when it was stopped.

</details>

<details>
<summary><b>Models, settings and profiles</b></summary>
<br/>

```
quill models           model servers, their status and models
quill models --scan    look for model servers on this machine and add them
quill usage            tokens used today, in 7 and 30 days, and all time
quill versions         every published release; quill update <version> switches to one
quill config           where the settings live; config edit, config instructions, config reset
```

Settings are kept in `settings.json` in `%APPDATA%\quill-code` on Windows, `~/Library/Application Support/quill-code` on macOS and `~/.config/quill-code` on Linux. A project can override any of them in `.quill/settings.json` at its root.

Edits keep each file's final newline, or its lack of one. New files follow the project's `.editorconfig`. Set `tools.final_newline` to `add` or `remove` to change that for every file Quill writes, except patches, diffs, crontabs and files the `.editorconfig` asks to keep one.

Models control their reasoning in different ways. gpt-oss takes a `reasoning_effort` of low, medium or high; Qwen3, GLM 4.5 to 4.7 and Hunyuan turn thinking on or off with `enable_thinking`; DeepSeek V3.1 and V3.2 use `thinking`. For anything else Quill reads what the server reports. To teach it a new model, add a profile to `models.profiles`; the first one whose `match` appears in the model's name wins:

```json
"profiles": [
  { "match": "my-reasoner", "reasoning": "effort", "levels": ["low", "medium", "high"] },
  { "match": "my-thinker", "except": ["instruct"], "reasoning": "toggle", "kwarg": "enable_thinking" },
  { "match": "my-coder", "reasoning": "none", "kwargs": { "my_template_variable": true } }
]
```

`reasoning` is `effort`, `toggle` or `none`, and `kwargs` are extra chat template variables sent with every request.

</details>

<br/>

## Under the hood

For anyone who wants to read or change the code. Rust 2024 and crossterm, no GPU and no browser engine.

| Crate | |
| --- | --- |
| **quill-code** | The `quill` binary: the screen, slash commands, pickers, approvals, `exec` |
| **quill-core** | The agent engine: the turn loop, tools, access policy, compaction, the system prompt |
| **quill-providers** | Model access: OpenAI-compatible chat, LM Studio and Ollama, model profiles, streaming tool calls |
| **quill-config** | Typed settings in layers, paths per system |
| **quill-git** | Your `git`: status, commits, branches, worktrees, and the GitHub API |
| **quill-weave** | The maths of the weave mark on the welcome banner |

You need Rust 1.88 or newer from [rustup](https://rustup.rs) and a C compiler: the [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with **Desktop development with C++** on Windows, `xcode-select --install` on macOS, `build-essential` or `gcc` on Linux.

| Command | |
| --- | --- |
| `cargo run -p quill-code --release` | Build and start `quill` in place |
| `cargo install --path crates/quill-code` | Install your build |
| `cargo test --workspace` | Every unit test |
| `cargo clippy --workspace --all-targets` | Lints, kept clean |
| `cargo deny check` | Licence and banned-crate policy (`cargo install cargo-deny`) |
| `vhs docs/demo/demo.tape` | Record the demo above again (needs [VHS](https://github.com/charmbracelet/vhs) and a running model) |

Releases come from `main`: bump `version` in `Cargo.toml`, add its section to `CHANGELOG.md`, and merge `dev` into `main`. CI builds every target into a draft release, which is published by hand once it has been tried. Published releases are immutable: their files and tag can never be changed, and `gh release verify-asset v<version> <file>` checks any download against the release's signed attestation. The architecture and conventions are in [AGENTS.md](AGENTS.md).

<br/>

## Get involved

- **Report a bug** in [Issues](https://github.com/Smoffyy/quill-code/issues), with the output of `quill --version` and the model you used
- **Send a PR** against `dev`, never `main`, with clippy and the tests passing

Quill Code is meant for people who would rather run their own models than rent someone else's. It will stay free and MIT licensed.

<br/>

<p align="center">
  <a href="CHANGELOG.md">Changelog</a> · <a href="AGENTS.md">Architecture</a> · <a href="LICENSE">MIT License</a>
</p>