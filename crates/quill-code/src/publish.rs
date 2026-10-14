use std::path::Path;

use quill_agent::Publisher;

use crate::{git, github};

pub struct GitPublisher;

impl Publisher for GitPublisher {
    fn commit(&self, root: &Path, message: &str, branch: Option<&str>) -> Result<String, String> {
        if let Some(branch) = branch.filter(|b| git::branch(root).as_deref() != Some(*b)) {
            if !git::valid_branch(branch) {
                return Err(format!("\"{branch}\" is not a branch name Git accepts."));
            }
            git::create_branch(root, branch).or_else(|_| git::switch_branch(root, branch))?;
        }
        let sha = git::commit_all(root, message)?;
        let on = git::branch(root).unwrap_or_else(|| "a detached HEAD".into());
        Ok(format!("Committed {sha} on {on}."))
    }

    fn open_pull_request(&self, root: &Path, title: &str, body: &str) -> Result<String, String> {
        let branch = git::branch(root).ok_or("Not on a branch. Commit to a new branch with commit_changes first.")?;
        let base = git::default_branch(root);
        if branch == base {
            return Err(format!("You are on {base}, the main branch. Commit the work to a new branch with commit_changes first."));
        }
        let remote =
            git::remote_url(root).ok_or("This repository has no remote called origin, so there is nowhere to open a pull request.")?;
        let slug = git::github_slug(&remote).ok_or("Pull requests can only be opened for repositories on GitHub.")?;
        git::push(root, &branch)?;
        let url = github::create_pull_request(&slug, &branch, &base, title, body)?;
        Ok(format!("Pushed {branch} and opened a pull request into {base}: {url}"))
    }
}