use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn hidden(program: &str) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.stdin(Stdio::null());
    cmd
}

fn run(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = hidden("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("Git could not start: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let error = String::from_utf8_lossy(&output.stderr);
        let lines: Vec<&str> = error.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("hint:")).collect();
        Err(if lines.is_empty() { "Git reported an error.".into() } else { lines.join("\n") })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileStatus {
    pub path: String,
    pub code: String,
    pub added: usize,
    pub removed: usize,
}

impl FileStatus {
    pub fn label(&self) -> &'static str {
        match self.code.trim() {
            "??" | "A" | "AM" => "new",
            "D" | "AD" => "deleted",
            code if code.starts_with('R') => "renamed",
            _ => "changed",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub branch: Option<String>,
    pub head: Option<String>,
    pub upstream: bool,
    pub ahead: usize,
    pub changed: usize,
}

pub fn parse_snapshot(porcelain: &str) -> Snapshot {
    let mut snapshot = Snapshot::default();
    for line in porcelain.lines() {
        if let Some(oid) = line.strip_prefix("# branch.oid ") {
            snapshot.head = (oid != "(initial)").then(|| oid.chars().take(7).collect());
        } else if let Some(name) = line.strip_prefix("# branch.head ") {
            snapshot.branch = (name != "(detached)").then(|| name.to_string());
        } else if line.starts_with("# branch.upstream ") {
            snapshot.upstream = true;
        } else if let Some(counts) = line.strip_prefix("# branch.ab +") {
            snapshot.ahead = counts.split(' ').next().and_then(|n| n.parse().ok()).unwrap_or(0);
        } else if !line.is_empty() && !line.starts_with('#') {
            snapshot.changed += 1;
        }
    }
    snapshot
}

pub fn snapshot(root: &Path) -> Option<Snapshot> {
    run(root, &["status", "--porcelain=v2", "--branch"]).ok().map(|out| parse_snapshot(&out))
}

pub fn branch(root: &Path) -> Option<String> {
    run(root, &["symbolic-ref", "--short", "-q", "HEAD"]).ok().map(|b| b.trim().to_string()).filter(|b| !b.is_empty())
}

pub fn head(root: &Path) -> Option<String> {
    run(root, &["rev-parse", "--short", "HEAD"]).ok().map(|h| h.trim().to_string()).filter(|h| !h.is_empty())
}

pub fn default_branch(root: &Path) -> String {
    if let Ok(out) = run(root, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]) {
        let name = out.trim().trim_start_matches("origin/");
        if !name.is_empty() {
            return name.to_string();
        }
    }
    for candidate in ["main", "master"] {
        if run(root, &["rev-parse", "--verify", "--quiet", candidate]).is_ok() {
            return candidate.to_string();
        }
    }
    "main".into()
}

pub fn parse_status(z: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    let mut parts = z.split('\0').filter(|p| !p.is_empty());
    while let Some(entry) = parts.next() {
        if entry.len() < 4 {
            continue;
        }
        let code = entry[..2].trim().to_string();
        let path = entry[3..].to_string();
        if code.starts_with('R') || code.starts_with('C') {
            parts.next();
        }
        entries.push((code, path));
    }
    entries
}

fn numstat(root: &Path) -> HashMap<String, (usize, usize)> {
    let Ok(out) = run(root, &["diff", "--numstat", "HEAD"]) else { return HashMap::new() };
    out.lines()
        .filter_map(|line| {
            let mut cols = line.split('\t');
            let added = cols.next()?.parse().unwrap_or(0);
            let removed = cols.next()?.parse().unwrap_or(0);
            Some((cols.next()?.to_string(), (added, removed)))
        })
        .collect()
}

pub fn status(root: &Path) -> Result<Vec<FileStatus>, String> {
    let out = run(root, &["status", "--porcelain=v1", "-z", "-uall"])?;
    let counts = numstat(root);
    Ok(parse_status(&out)
        .into_iter()
        .map(|(code, path)| {
            let (added, removed) = counts.get(&path).copied().unwrap_or_else(|| {
                let lines = fs::read_to_string(root.join(&path)).map(|t| t.lines().count()).unwrap_or(0);
                (lines, 0)
            });
            FileStatus { path, code, added, removed }
        })
        .collect())
}

pub fn stage(root: &Path, files: &[String]) -> Result<(), String> {
    let mut args = vec!["add", "-A", "--"];
    args.extend(files.iter().map(String::as_str));
    run(root, &args).map(|_| ())
}

pub fn staged_diff(root: &Path, limit: usize) -> String {
    let stat = run(root, &["diff", "--cached", "--stat"]).unwrap_or_default();
    let mut diff = run(root, &["diff", "--cached"]).unwrap_or_default();
    if diff.len() > limit {
        let mut cut = limit;
        while !diff.is_char_boundary(cut) {
            cut -= 1;
        }
        diff.truncate(cut);
        diff.push_str("\n[diff cut]");
    }
    format!("{stat}\n{diff}")
}

pub fn commit(root: &Path, files: &[String], message: &str) -> Result<String, String> {
    stage(root, files)?;
    let mut args = vec!["commit", "-m", message, "--"];
    args.extend(files.iter().map(String::as_str));
    run(root, &args)?;
    Ok(head(root).unwrap_or_default())
}

pub fn create_branch(root: &Path, name: &str) -> Result<(), String> {
    run(root, &["switch", "-c", name]).map(|_| ())
}

pub fn valid_branch(name: &str) -> bool {
    hidden("git")
        .args(["check-ref-format", "--branch", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Branches {
    pub current: Option<String>,
    pub local: Vec<String>,
    pub remote: Vec<String>,
    pub unmerged: Vec<String>,
}

fn names(root: &Path, args: &[&str]) -> Vec<String> {
    run(root, args).map(|out| out.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()).unwrap_or_default()
}

pub fn remote_only(remote: Vec<String>, local: &[String]) -> Vec<String> {
    remote
        .into_iter()
        .filter_map(|name| name.strip_prefix("origin/").map(str::to_string))
        .filter(|name| name != "HEAD" && !local.contains(name))
        .collect()
}

pub fn branches(root: &Path) -> Branches {
    let current = branch(root);
    let mut local = names(root, &["branch", "--format=%(refname:short)"]);
    if let Some(current) = current.clone().filter(|c| !local.contains(c)) {
        local.insert(0, current);
    }
    let remote = remote_only(names(root, &["branch", "-r", "--format=%(refname:short)"]), &local);
    let unmerged = names(root, &["branch", "--no-merged", "--format=%(refname:short)"]);
    Branches { current, local, remote, unmerged }
}

pub fn switch_branch(root: &Path, name: &str) -> Result<(), String> {
    run(root, &["switch", name]).map(|_| ())
}

pub fn commit_all(root: &Path, message: &str) -> Result<String, String> {
    run(root, &["add", "-A"])?;
    run(root, &["commit", "-m", message])?;
    Ok(head(root).unwrap_or_default())
}

pub fn init(root: &Path, branch: &str, message: &str) -> Result<(), String> {
    run(root, &["init", "-b", branch])?;
    _ = commit_all(root, message);
    Ok(())
}

pub fn push(root: &Path, branch: &str) -> Result<(), String> {
    run(root, &["push", "-u", "origin", branch]).map(|_| ())
}

pub fn add_remote(root: &Path, url: &str) -> Result<(), String> {
    run(root, &["remote", "add", "origin", url]).map(|_| ())
}

pub fn remote_url(root: &Path) -> Option<String> {
    run(root, &["remote", "get-url", "origin"]).ok().map(|u| u.trim().to_string()).filter(|u| !u.is_empty())
}

pub fn repo_name(root: &Path) -> String {
    let common = run(root, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).unwrap_or_default();
    let main = Path::new(common.trim()).parent().unwrap_or(root);
    main.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn host_and_path(remote: &str) -> Option<(String, String)> {
    let remote = remote.trim().trim_end_matches('/').trim_end_matches(".git");
    let (host, path) = match remote.strip_prefix("git@") {
        Some(rest) => rest.split_once(':')?,
        None => {
            let rest = remote.split_once("://")?.1;
            rest.rsplit_once('@').map_or(rest, |(_, r)| r).split_once('/')?
        }
    };
    Some((host.to_string(), path.to_string()))
}

pub fn github_slug(remote: &str) -> Option<String> {
    let (host, path) = host_and_path(remote)?;
    (host == "github.com").then_some(path)
}

pub fn review_url(remote: &str, base: &str, branch: &str) -> Option<String> {
    let (host, path) = host_and_path(remote)?;
    match host.as_str() {
        "github.com" => Some(format!("https://github.com/{path}/compare/{base}...{branch}?expand=1")),
        "gitlab.com" => Some(format!(
            "https://gitlab.com/{path}/-/merge_requests/new?merge_request[source_branch]={branch}&merge_request[target_branch]={base}"
        )),
        _ => None,
    }
}

pub fn add_worktree(project: &Path, path: &Path) -> Result<(), String> {
    let target = path.to_string_lossy().into_owned();
    run(project, &["worktree", "add", "--detach", &target]).map(|_| ()).map_err(|error| {
        if error.contains("invalid reference") { "An isolated copy needs at least one commit in the project first.".into() } else { error }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_entries_are_parsed() {
        let z = " M src/main.rs\0?? new file.txt\0R  renamed.rs\0old.rs\0D  gone.rs\0";
        let entries = parse_status(z);
        assert_eq!(
            entries,
            vec![
                ("M".into(), "src/main.rs".into()),
                ("??".into(), "new file.txt".into()),
                ("R".into(), "renamed.rs".into()),
                ("D".into(), "gone.rs".into()),
            ]
        );
    }

    #[test]
    fn review_pages_come_from_remotes() {
        assert_eq!(
            review_url("https://github.com/me/tool.git", "main", "fix-login").as_deref(),
            Some("https://github.com/me/tool/compare/main...fix-login?expand=1")
        );
        assert_eq!(
            review_url("git@github.com:me/tool.git", "main", "x").as_deref(),
            Some("https://github.com/me/tool/compare/main...x?expand=1")
        );
        assert!(review_url("https://gitlab.com/g/p", "main", "b").unwrap().contains("merge_requests/new"));
        assert!(review_url("https://example.com/a/b", "main", "b").is_none());
    }

    #[test]
    fn snapshots_read_branch_state() {
        let out = "# branch.oid ed107bd9c35ae3222f7fedff635bc91f944fede0\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -0\n1 .M N... 100644 100644 100644 a b hello.py\n? notes.txt\n";
        let snapshot = parse_snapshot(out);
        assert_eq!(snapshot.branch.as_deref(), Some("main"));
        assert_eq!(snapshot.head.as_deref(), Some("ed107bd"));
        assert!(snapshot.upstream);
        assert_eq!((snapshot.ahead, snapshot.changed), (2, 2));
        let fresh = parse_snapshot("# branch.oid (initial)\n# branch.head (detached)\n");
        assert_eq!((fresh.head, fresh.branch, fresh.upstream), (None, None, false));
    }

    #[test]
    fn remote_branches_skip_local_ones_and_head() {
        let remote = vec!["origin/HEAD".into(), "origin/main".into(), "origin/feature".into(), "upstream/x".into()];
        assert_eq!(remote_only(remote, &["main".into()]), vec!["feature".to_string()]);
    }

    #[test]
    fn github_remotes_give_a_slug() {
        assert_eq!(github_slug("https://github.com/me/tool.git").as_deref(), Some("me/tool"));
        assert_eq!(github_slug("git@github.com:me/tool.git").as_deref(), Some("me/tool"));
        assert!(github_slug("https://gitlab.com/g/p").is_none());
    }

    #[test]
    fn labels_describe_changes() {
        let status = |code: &str| FileStatus { path: "a".into(), code: code.into(), added: 0, removed: 0 };
        assert_eq!(status("??").label(), "new");
        assert_eq!(status("M").label(), "changed");
        assert_eq!(status("D").label(), "deleted");
    }
}