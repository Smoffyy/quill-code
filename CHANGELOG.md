# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.1] - TBD

### Changed

- Releases are named by their version alone, and their notes end with GitHub's list of what changed and a link to the full comparison with the previous release.
- `quill versions` lists each release's version and date without repeating its name.
- Dependencies are updated to their newest versions, including the bundled root certificates.
- `read_file` lists what a folder contains when it is given a folder, and calls to `list_dir`, `list_files` or `ls` go to it.
- Reading a path that does not exist opens the file when exactly one file in the project has that name; otherwise the error names the files with that name, or shows what the nearest folder contains.
- When `find_files`, or `grep` with a file filter, finds nothing, the reply lists the kinds of files the folder has, so a model that guessed the wrong language corrects itself.
- When an edit's text is not in the file, the error names the lines that are missing and shows the file's current lines at that spot, ready to copy.
- A failed PowerShell command that used `&&`, a Unix command such as `grep`, `head` or `touch`, or a Unix flag such as `rm -rf` now ends with the PowerShell way to do it.
- After renaming, moving or deleting something, Quill searches for the old name to make sure nothing still uses it.

### Fixed

- Glob patterns with a brace group, such as `**/*.{js,ts}`, no longer fail in `find_files` and `grep`.
- Writes and edits that would replace real code with a placeholder such as `// ... existing code ...` are refused instead of deleting that code.
- An edit that would leave a file unchanged now says so instead of reporting a change, and rewriting a file with its current content no longer asks for approval.
- Reading a file with very long lines, such as minified code, no longer floods the model's context.
- Tool arguments that arrive encoded twice, wrapped in an extra object, followed by stray characters, or with lists sent as text are read correctly.
- Text a model sends with escaped line breaks (`\n`) instead of real ones is written and matched as intended.

### Security

- Releases are immutable from 1.0.1 on: once published, their files and tag can never change, and each one carries a signed attestation. Check a download with `gh release verify-asset v1.0.1 <file>`.

## [1.0.0] - 2026-10-19

### Added

- `quill`, a command-line program for Windows, Linux and macOS, with a one-line install (`curl ... | sh`, or `irm ... | iex` on Windows) that checks the download against the release's checksums and needs no admin rights and no Rust.
- Self-contained builds: one static Linux binary for every distribution on x64 and ARM, Windows without the Visual C++ runtime, and macOS for Apple silicon and Intel. TLS is pure Rust (rustls) and uses the system's certificates.
- Support for llama.cpp, LM Studio, Ollama, vLLM, Jan, GPT4All and any other OpenAI-compatible server you run.
- A first run that asks which app runs your models, finds the servers already running and lists their models, or says what to set up. /connect adds another server at any time.
- A trust prompt the first time Quill Code opens a folder.
- Model profiles: how each model family controls its reasoning lives in one table, and settings.json can add or override profiles without a rebuild. gpt-oss gets low, medium and high reasoning effort through /effort.
- Auto-compaction near the end of the context window: old command output is trimmed first, then the conversation is summarised and the task carries on where it stopped, with the request and its latest steps kept word for word. The summary follows fixed headings, and Quill adds the list of files read or changed and the latest plan, telling the model to read a file again before editing it; the five files worked on most recently are read back from disk into the new context. If the server still reports a full context, Quill compacts once and tries again. A turn compacts at most three times, then says the task needs a bigger window.
- /instructions (or /memory) opens your personal AGENTS.md, which every session reads, in your editor.
- Quill can ask up to four questions at once: pick an answer with the arrow keys, press Enter again to confirm, or type your own on the "Something else" row. ← and → switch between questions, and the answers go back together once all are in.
- `tools.final_newline`: files keep their final newline, or their lack of one, unless an edit changes it, and new files follow `.editorconfig`. Set it to `add` to end every file with a newline, or to `remove` to end files right after their last character, except patches, diffs, crontabs and files `.editorconfig` asks to keep one. Diffs say "No newline at end of file" when an edit changes only that.
- `/version`, `quill versions` and `quill update <version>` switch to any published release, and the one-line install takes `QUILL_VERSION` to install a particular one. `/version` reinstalls and reopens Quill Code on the thread you were in.
- `quill exec` runs one task headless for scripts and CI, with `--json` for the event stream; `quill models`, `quill usage` and `quill config` replace the settings pages.
- Slash commands: /new, /clear, /resume, /model with pins, /effort, /mode, /init, /compact, /retry, /diff, /usage, /commit, /branch, /rename, /version and /help, plus @ file mentions and attachments by pasting a path.
- A full-screen view: the conversation scrolls inside Quill with the mouse wheel, Page Up and Page Down while the prompt stays fixed at the bottom, and the session is printed into the terminal's scrollback on exit.
- Thoughts show as a short summary made of whole sentences that refreshes every few seconds; ctrl+o shows them in full.

### Changed

- Quill Code is a command-line program instead of a desktop window, so nothing needs a GPU, Vulkan, Xcode's Metal tools or bundled fonts any more.
- Quill Code runs on local models only.
- Fewer, clearer tools, shaped like Claude Code's, and Quill is told to use the system's own commands rather than scripts.
- read_file marks each line with → and says when a file ends with a newline, so small models copy indentation exactly and can see the newline to remove it.
- Shell commands that edit files with a script (python -c, sed -i, Set-Content, echo into a file and the like) are refused, and the model is told to use edit_file or write_file so every change shows as a diff.
- Tool rows in the style of Claude Code: details hang off a ⎿, a running command shows its full text with its time and line count, and edits show a summary and a diff with shaded added and removed lines. The activity line says Processing, not Thinking, while the model reads tool results.
- Every turn prints as it settles: tool rows with timings, grouped exploring, diffs, command output, plans, questions, reasoning and the answer as rendered markdown, with tables and code blocks.
- The prompt sits at the bottom of the window between two rules, with a status line below it that shows the model, access mode, branch, uncommitted and unpushed work, pull request and context. The access mode leads it in its own colour (green Ask first, yellow Trust edits, blue Autopilot), and Shift+Tab switches it.
- Your messages and answers stand out on a shaded background, Quill's replies start with ●, reasoning shows live as it streams, and tool rows are marked ✓ or ✗.
- Quill Code opens on a clear screen with an animated weave, the activity line carries a small weave that moves one way while Quill thinks and another while it writes, and the cursor stays steady while the screen redraws.
- Settings live in the platform's own folder on each system and are written atomically. Settings and threads keep the fields an older version does not know, so moving between versions loses nothing.

### Removed

- The desktop window.
- Claude Code, the Anthropic and OpenAI APIs, and cost tracking; /usage counts tokens.
- The list_dir and apply_patch tools: find_files and shell cover listing, and edit_file and write_file cover changes.

### Fixed

- Small models that get indentation wrong in an edit no longer break the file: the replacement keeps the file's own indentation.
- Small models no longer commit or open pull requests on their own: those tools only run when your message asks for a commit, a push or a pull request.
- Stopping a command ends everything it started, not only the shell.

## [0.1.0] - 2026-10-14

### Added

- Projects and threads, with several agents running at once and a sidebar that shows each thread's branch, commit, uncommitted work, unpushed commits, pull request and model.
- A tool-calling agent that lists, reads, searches, writes and edits files, runs commands, commits and opens pull requests, under Ask first, Trust edits and Autopilot access modes.
- Isolated copies (git worktrees) so parallel threads never collide.
- A commit dialog that can push, open a pull request page or publish a new GitHub repository, and new projects that can start on GitHub with a first branch of your choosing.
- Slash commands, @ file mentions, image and file attachments with previews, pinned models with Ctrl+1 to 9, a context ring, a PowerShell drawer and saved history.
- Models, context windows, vision and thinking controls detected from the server.