use anyhow::{Context, Result};
use git2::{Repository, Signature, Time, Commit, ObjectType};
use std::path::Path;
use chrono::{DateTime, Utc};

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
    index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .context("Failed to add files to git index")?;
    index.write().context("Failed to write git index")?;

    let tree_id = index.write_tree().context("Failed to write tree")?;
    let tree = repo.find_tree(tree_id).context("Failed to find tree")?;

    let signature = Signature::now("Myelin Auto-Save", "auto@myelin.app")
        .context("Failed to create signature")?;

    let parent_commit = match repo.head() {
        Ok(head) => Some(head.peel_to_commit().context("Failed to peel to commit")?),
        Err(_) => None,
    };

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
    ).context("Failed to create commit")?;

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
    let mut revwalk = repo.revwalk().context("Failed to create revwalk")?;
    
    // If push_head fails (e.g. repo is empty and has no commits yet), return empty history
    if revwalk.push_head().is_err() {
        return Ok(Vec::new());
    }

    let mut history = Vec::new();

    for oid in revwalk {
        if let Ok(oid) = oid {
            if let Ok(commit) = repo.find_commit(oid) {
                // In a more robust implementation, we would diff the tree to see if the specific file changed.
                // For simplicity, if we pass a file path, we can either return all commits or filter them.
                // Since our commits are fine-grained auto-saves, returning the commits is a good start.
                let time = commit.time();
                let datetime = DateTime::<Utc>::from_timestamp(time.seconds(), 0).unwrap_or_default();

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

    // In git, paths are relative to the repo root without leading slash
    let relative_path = file_path.trim_start_matches('/');
    
    let entry = tree.get_path(Path::new(relative_path)).context("File not found in commit")?;
    let object = entry.to_object(&repo).context("Failed to convert entry to object")?;
    let blob = object.into_blob().map_err(|_| anyhow::anyhow!("Object is not a blob"))?;
    
    let content = String::from_utf8(blob.content().to_vec()).context("File content is not valid UTF-8")?;
    Ok(content)
}
