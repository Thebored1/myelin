use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use git2::{DiffOptions, Repository, Signature};
use std::path::{Path, PathBuf};

/// Initialize a git repository in the workspace if it doesn't exist
pub fn init_repo(workspace: &Path) -> Result<()> {
    if !workspace.join(".git").exists() {
        Repository::init(workspace).context("Failed to initialize git repository")?;
    }
    Ok(())
}

/// Commit all changes in the workspace
pub fn commit_changes(workspace: &Path, message: &str) -> Result<()> {
    let repo = Repository::open(workspace).context("Failed to open git repository")?;

    let mut index = repo.index().context("Failed to get git index")?;
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .context("Failed to add files to git index")?;
    index.write().context("Failed to write git index")?;

    let tree_id = index.write_tree().context("Failed to write tree")?;
    let tree = repo.find_tree(tree_id).context("Failed to find tree")?;

    let parent_commit = match repo.head() {
        Ok(head) => Some(head.peel_to_commit().context("Failed to peel to commit")?),
        Err(_) => None,
    };

    if let Some(parent) = &parent_commit {
        let parent_tree = parent.tree().context("Failed to load parent tree")?;
        if parent_tree.id() == tree.id() {
            return Ok(());
        }
    }

    let signature = Signature::now("Myelin Auto-Save", "auto@myelin.app")
        .context("Failed to create signature")?;

    let mut parents = Vec::new();
    if let Some(ref parent) = parent_commit {
        parents.push(parent);
    }

    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parents,
    )
    .context("Failed to create commit")?;

    Ok(())
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GitCommit {
    pub hash: String,
    pub message: String,
    pub timestamp: String,
    pub author: String,
}

/// Get the commit history for a specific file
pub fn get_file_history(workspace: &Path, file_path: &str) -> Result<Vec<GitCommit>> {
    let repo = Repository::open(workspace).context("Failed to open git repository")?;
    let relative_path = repo_relative_path(&repo, workspace, file_path)?;
    let mut revwalk = repo.revwalk().context("Failed to create revwalk")?;

    // If push_head fails (e.g. repo is empty and has no commits yet), return empty history
    if revwalk.push_head().is_err() {
        return Ok(Vec::new());
    }

    let mut history = Vec::new();

    for oid in revwalk {
        if let Ok(oid) = oid {
            if let Ok(commit) = repo.find_commit(oid) {
                if !commit_touches_path(&repo, &commit, &relative_path)? {
                    continue;
                }

                let time = commit.time();
                let datetime =
                    DateTime::<Utc>::from_timestamp(time.seconds(), 0).unwrap_or_default();

                history.push(GitCommit {
                    hash: commit.id().to_string(),
                    message: commit.message().unwrap_or("").trim().to_string(),
                    timestamp: datetime.to_rfc3339(),
                    author: commit.author().name().unwrap_or("Unknown").to_string(),
                });
            }
        }
    }

    Ok(history)
}

/// Retrieve the exact contents of a file at a specific commit hash
pub fn get_file_at_commit(workspace: &Path, commit_hash: &str, file_path: &str) -> Result<String> {
    let repo = Repository::open(workspace).context("Failed to open git repository")?;
    let oid = git2::Oid::from_str(commit_hash).context("Invalid commit hash")?;
    let commit = repo.find_commit(oid).context("Commit not found")?;
    let tree = commit.tree().context("Tree not found")?;
    let relative_path = repo_relative_path(&repo, workspace, file_path)?;

    let entry = tree
        .get_path(&relative_path)
        .context("File not found in commit")?;
    let object = entry
        .to_object(&repo)
        .context("Failed to convert entry to object")?;
    let blob = object
        .into_blob()
        .map_err(|_| anyhow::anyhow!("Object is not a blob"))?;

    let content =
        String::from_utf8(blob.content().to_vec()).context("File content is not valid UTF-8")?;
    Ok(content)
}

fn repo_relative_path(repo: &Repository, workspace: &Path, file_path: &str) -> Result<PathBuf> {
    let path = PathBuf::from(file_path);
    let rel_path = if path.is_absolute() {
        let repo_root = repo
            .workdir()
            .or_else(|| repo.path().parent())
            .unwrap_or(workspace);
        path.strip_prefix(repo_root)
            .map(|value| value.to_path_buf())
            .map_err(|_| anyhow!("Path is outside the git repository"))?
    } else {
        path
    };

    // libgit2 expects forward slashes, even on Windows
    let s = rel_path.to_string_lossy().replace("\\", "/");
    Ok(PathBuf::from(s))
}

fn commit_touches_path(repo: &Repository, commit: &git2::Commit<'_>, path: &Path) -> Result<bool> {
    let tree = commit.tree().context("Tree not found")?;

    if commit.parent_count() == 0 {
        return Ok(tree.get_path(path).is_ok());
    }

    let parent = commit.parent(0).context("Parent commit not found")?;
    let parent_tree = parent.tree().context("Parent tree not found")?;
    let mut diff_options = DiffOptions::new();
    diff_options.pathspec(path);
    let diff = repo
        .diff_tree_to_tree(Some(&parent_tree), Some(&tree), Some(&mut diff_options))
        .context("Failed to diff commit trees")?;

    Ok(diff.deltas().len() > 0)
}
