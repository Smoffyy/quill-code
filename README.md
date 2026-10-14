# quill-code

A native desktop home for Quill, a coding agent that runs on your own models. Point it at a llama.cpp server (or another OpenAI-compatible one), pick the model that powers Quill, and hand it work: it reads and edits your projects, runs commands in your system's own shell, shows every diff, and takes the git side all the way to a pull request.

## Principles

- **Native.** Rust and GPUI, drawn on the GPU. No browser engine renders the interface.
- **No telemetry, ever.** The app talks only to the model servers you configure, to your git remotes, and to GitHub when you publish a repository or open a pull request.
- **MIT licensed,** with permissively licensed dependencies only.
- **Built for local models.** Tool calls, context windows and thinking controls are detected from the server rather than configured by hand.

## Status

Early development, Windows first. Working today:

- Projects and threads, with several agents running at once and a sidebar that shows each thread's branch, commit, uncommitted work, unpushed commits, pull request and model.
- A tool-calling agent that lists, reads, searches, writes and edits files, runs commands, commits and opens pull requests, under Ask first, Trust edits and Autopilot access modes.
- Isolated copies (git worktrees) so parallel threads never collide, a commit dialog that can push, open a pull request page or publish a new GitHub repository, and new projects that can start on GitHub with a first branch of your choosing.
- Slash commands, @ file mentions, image and file attachments with previews, pinned models with Ctrl+1 to 9, a context ring, a PowerShell drawer and saved history.

GitHub sign-in goes through Git Credential Manager, so quill-code never stores a token.

## Building

Every platform needs:

- **Rust 1.88 or newer**, installed with [rustup](https://rustup.rs). The default stable toolchain is enough.
- **Git** on the PATH, for cloning, commits, branches and isolated copies.
- **A model server** to talk to, such as llama.cpp's `llama-server`, or any server with an OpenAI-compatible `/v1` API.

Then, from the repository folder:

```
cargo run -p quill-code --release
```

### Windows

Windows is the platform quill-code is developed and tested on.

1. Install the [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the **Desktop development with C++** workload, which brings the MSVC compiler and the Windows SDK.
2. Install Rust with `rustup-init.exe`, keeping the default `x86_64-pc-windows-msvc` toolchain.
3. Install [Git for Windows](https://git-scm.com/download/win). It includes Git Credential Manager, which handles GitHub sign-in.

### macOS

1. Install **Xcode** from the App Store, not only the Command Line Tools: the interface's Metal shaders are compiled at build time with Xcode's `metal` tool. Open Xcode once to accept its licence, then run `sudo xcode-select -s /Applications/Xcode.app`. If the build says the `metal` tool is missing, run `xcodebuild -downloadComponent MetalToolchain`.
2. Install Rust with rustup.
3. Git comes with Xcode. For GitHub sign-in, install [Git Credential Manager](https://github.com/git-ecosystem/git-credential-manager) or any credential helper that stores a GitHub token.

### Linux

Debian and Ubuntu:

```
sudo apt install build-essential pkg-config git libssl-dev libfontconfig-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan1 mesa-vulkan-drivers
```

Fedora:

```
sudo dnf install gcc gcc-c++ pkgconf-pkg-config git openssl-devel fontconfig-devel libxkbcommon-devel libxkbcommon-x11-devel wayland-devel vulkan-loader mesa-vulkan-drivers
```

The interface draws with Vulkan, so the machine needs a working Vulkan driver; the Mesa drivers above cover most GPUs. For GitHub sign-in, install [Git Credential Manager](https://github.com/git-ecosystem/git-credential-manager) or any credential helper that stores a GitHub token.

macOS and Linux builds go through the same code but have not been tested yet.

### Checks

```
cargo test --workspace
cargo clippy --workspace --all-targets
cargo deny check
```

`cargo deny` is installed with `cargo install cargo-deny`.

## Licence

MIT, see [LICENSE](LICENSE). Bundled Open Sans is under the SIL Open Font License, see [assets/fonts/OFL.txt](assets/fonts/OFL.txt). The llama.cpp mark is from the llama.cpp project (MIT, Copyright (c) 2023-2026 The ggml authors).