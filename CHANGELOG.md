# Changelog

Each release is a `## <version>` heading, optionally followed by ` - <name>`. When the workspace version in `Cargo.toml` reaches `main` without a release, the Release workflow builds Windows, Linux and macOS, takes the heading as the release title and the section below it as the notes, and opens a draft release tagged `v<version>`.

## 0.1.0 - First preview

- Projects and threads, with several agents running at once and a sidebar that shows each thread's branch, commit, uncommitted work, unpushed commits, pull request and model.
- A tool-calling agent that lists, reads, searches, writes and edits files, runs commands, commits and opens pull requests, under Ask first, Trust edits and Autopilot access modes.
- Isolated copies (git worktrees) so parallel threads never collide.
- A commit dialog that can push, open a pull request page or publish a new GitHub repository, and new projects that can start on GitHub with a first branch of your choosing.
- Slash commands, @ file mentions, image and file attachments with previews, pinned models with Ctrl+1 to 9, a context ring, a PowerShell drawer and saved history.
- Models, context windows, vision and thinking controls detected from the server.