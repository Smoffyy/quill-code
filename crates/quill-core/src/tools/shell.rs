use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::args::{self, COMMAND};
use super::paths::resolve;
use super::{Ctx, Output, Prepared, Spec, Tool, Toolset};
use crate::options::Shell;
use crate::policy::{Effect, discards_work, edits_files};

const SCRIPT_EDIT: &str = "Change files with edit_file or write_file, not with shell commands or scripts, so the user sees a diff of every change. Use shell to run programs, builds and tests.";
use crate::protocol::Category;

pub fn shell_command(shell: Shell, command: &str) -> Command {
    let mut cmd = match shell {
        Shell::PowerShell | Shell::Pwsh => {
            let mut cmd = Command::new(if shell == Shell::Pwsh { "pwsh" } else { "powershell" });
            cmd.args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &format!("[Console]::OutputEncoding=[Text.Encoding]::UTF8; $ProgressPreference='SilentlyContinue'; {command}"),
            ]);
            cmd
        }
        Shell::Cmd => {
            let mut cmd = Command::new("cmd");
            cmd.args(["/D", "/S", "/C", command]);
            cmd
        }
        Shell::Bash => {
            let mut cmd = Command::new(git_bash().unwrap_or_else(|| PathBuf::from("bash")));
            cmd.args(["-c", command]);
            cmd
        }
        Shell::Sh => {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", command]);
            cmd
        }
    };
    hide_window(&mut cmd);
    cmd
}

fn git_bash() -> Option<PathBuf> {
    let program_files = std::env::var_os("ProgramFiles")?;
    let path = PathBuf::from(program_files).join("Git").join("bin").join("bash.exe");
    path.is_file().then_some(path)
}

#[cfg(windows)]
pub(crate) fn hide_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
pub(crate) fn hide_window(_: &mut Command) {}

#[cfg(windows)]
fn own_group(_: &mut Command) {}

#[cfg(not(windows))]
fn own_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

#[cfg(windows)]
fn stop(child: &mut Child) {
    let mut taskkill = Command::new("taskkill");
    taskkill.args(["/T", "/F", "/PID"]).arg(child.id().to_string()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    hide_window(&mut taskkill);
    let _ = taskkill.status();
    let _ = child.kill();
}

#[cfg(not(windows))]
fn stop(child: &mut Child) {
    let group = format!("-{}", child.id());
    let _ = Command::new("kill").args(["-KILL", "--", &group]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    let _ = child.kill();
}

fn cap(text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let mut head = max * 2 / 3;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - max / 3;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!("{}\n... [output cut] ...\n{}", &text[..head], &text[tail..])
}

fn pump(stream: Option<impl Read + Send + 'static>, tx: std::sync::mpsc::Sender<String>) {
    let Some(stream) = stream else { return };
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        while reader.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
            if tx.send(String::from_utf8_lossy(&buf).into_owned()).is_err() {
                break;
            }
            buf.clear();
        }
    });
}

pub fn run(
    shell: Shell,
    dir: &Path,
    command: &str,
    timeout: Duration,
    cancel: &AtomicBool,
    max_output: usize,
    on_chunk: &dyn Fn(&str),
) -> (bool, String) {
    let mut cmd = shell_command(shell, command);
    own_group(&mut cmd);
    let spawned = cmd.current_dir(dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => return (false, format!("The command could not start: {e}")),
    };
    let (tx, rx) = channel();
    pump(child.stdout.take(), tx.clone());
    pump(child.stderr.take(), tx);
    let started = Instant::now();
    let mut text = String::new();
    let mut ended: Option<Instant> = None;
    let mut outcome = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(40)) {
            Ok(chunk) => {
                on_chunk(&chunk);
                if text.len() < max_output * 4 {
                    text.push_str(&chunk);
                }
            }
            Err(RecvTimeoutError::Disconnected) if outcome.is_some() => break,
            Err(_) => {}
        }
        if outcome.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => outcome = Some((Some(status), None)),
                Ok(None) if cancel.load(Ordering::Relaxed) => {
                    stop(&mut child);
                    outcome = Some((None, Some("Stopped by the user.".to_string())));
                }
                Ok(None) if started.elapsed() > timeout => {
                    stop(&mut child);
                    outcome = Some((None, Some(format!("Stopped after {} seconds without finishing.", timeout.as_secs()))));
                }
                Ok(None) => {}
                Err(e) => outcome = Some((None, Some(e.to_string()))),
            }
            if outcome.is_some() {
                ended = Some(Instant::now());
            }
        } else if ended.is_some_and(|at| at.elapsed() > Duration::from_millis(800)) {
            break;
        }
    }
    let _ = child.wait();
    let (status, note) = outcome.unwrap_or((None, None));
    let ok = status.is_some_and(|s| s.success());
    let mut report = match (status.and_then(|s| s.code()), note) {
        (_, Some(note)) => note,
        (Some(code), None) => format!("Exit code {code}"),
        (None, None) => "Exited".into(),
    };
    let body = text.trim_end();
    if !body.is_empty() {
        report.push('\n');
        report.push_str(body);
    }
    (ok, cap(report, max_output))
}

pub struct ShellTool;

const SHELL: Spec = Spec {
    name: "shell",
    title: "Run commands",
    summary: "Runs commands in the project folder: builds, tests, installs.",
    aliases: &[
        "run_command",
        "run",
        "bash",
        "powershell",
        "exec",
        "execute_command",
        "terminal",
        "cmd",
        "run_shell_command",
        "shell_command",
    ],
    toolset: Toolset::Core,
    category: Category::Command,
    active: "Running",
    done: "Ran",
    parallel: false,
    description: "Run a shell command in the project folder and return its output and exit code. Use it to build, test, install and run code. The command must finish on its own.",
    guide: "shell runs a command in the project folder and returns its output and exit code.",
};

impl Tool for ShellTool {
    fn spec(&self) -> &'static Spec {
        &SHELL
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The command to run." },
                "timeout_seconds": { "type": "integer", "description": "How long to wait before stopping it. Default 120." },
                "workdir": { "type": "string", "description": "Optional folder to run in, relative to the project root." }
            },
            "required": ["command"]
        })
    }

    fn target(&self, args: &Value) -> String {
        args::text(args, &COMMAND).unwrap_or_default().trim().to_string()
    }

    fn prepare(&self, args: &Value, ctx: &Ctx<'_>) -> Result<Prepared, String> {
        let command = args::trimmed(args, &COMMAND).ok_or("shell needs a \"command\".")?.to_string();
        if edits_files(&command) {
            return Err(SCRIPT_EDIT.into());
        }
        let dir = match args::trimmed(args, &["workdir", "cwd", "dir", "directory"]) {
            Some(rel) => resolve(ctx.root, rel)?,
            None => ctx.root.to_path_buf(),
        };
        let limits = ctx.limits;
        let seconds =
            args::number(args, &["timeout_seconds", "timeout"]).unwrap_or(limits.command_timeout).clamp(1, limits.max_command_timeout);
        let title = if discards_work(&command) { "Wants to run a command that throws work away" } else { "Wants to run a command" };
        let mut prepared = Prepared::new(
            Effect::Execute,
            command.clone(),
            title,
            Box::new(move |ctx: &Ctx<'_>| {
                let (ok, text) =
                    run(ctx.limits.shell, &dir, &command, Duration::from_secs(seconds), ctx.cancel, ctx.limits.max_output, &|chunk| {
                        ctx.host.output(chunk)
                    });
                Output { ok, ..Output::ok(text) }
            }),
        );
        prepared.approval.similar = similar(&prepared.subject);
        Ok(prepared)
    }
}

pub(crate) fn similar(command: &str) -> Option<String> {
    let words: Vec<&str> = command.split_whitespace().take_while(|word| !word.starts_with('-')).take(2).collect();
    (!words.is_empty()).then(|| words.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopping_ends_the_programs_a_command_started() {
        let dir = std::env::temp_dir().join(format!("quill-stop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let command = if cfg!(windows) {
            "cmd /C \"ping -n 3 127.0.0.1 >nul & echo late > marker.txt\""
        } else {
            "sh -c 'sleep 2; echo late > marker.txt'"
        };
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let stopper = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(700));
            stopper.store(true, Ordering::Relaxed);
        });
        let (ok, output) = run(Shell::default(), &dir, command, Duration::from_secs(60), &cancel, 30_000, &|_| {});
        assert!(!ok && output.starts_with("Stopped by the user."), "{output}");
        std::thread::sleep(Duration::from_secs(4));
        assert!(!dir.join("marker.txt").exists(), "the started program kept running");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commands_report_exit_codes_and_stream() {
        let dir = std::env::temp_dir();
        let cancel = AtomicBool::new(false);
        let seen = std::sync::Mutex::new(String::new());
        let (ok, output) = run(Shell::default(), &dir, "echo quill", Duration::from_secs(60), &cancel, 30_000, &|chunk| {
            seen.lock().unwrap().push_str(chunk)
        });
        assert!(ok, "{output}");
        assert!(output.starts_with("Exit code 0") && output.contains("quill"));
        assert!(seen.lock().unwrap().contains("quill"));
        let (ok, output) = run(Shell::default(), &dir, "exit 3", Duration::from_secs(60), &cancel, 30_000, &|_| {});
        assert!(!ok);
        assert!(output.starts_with("Exit code 3"));
    }

    #[test]
    fn long_output_keeps_head_and_tail() {
        let text = format!("{}{}", "a".repeat(100), "z".repeat(100));
        let capped = cap(text, 60);
        assert!(capped.starts_with("aaaa") && capped.ends_with("zzzz") && capped.contains("[output cut]"));
    }

    #[test]
    fn similar_commands_use_the_leading_words() {
        assert_eq!(similar("npm test -- --watch").as_deref(), Some("npm test"));
        assert_eq!(similar("cargo build --release").as_deref(), Some("cargo build"));
        assert_eq!(similar("ls").as_deref(), Some("ls"));
    }
}