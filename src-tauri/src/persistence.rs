use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Filesystem roots that a workspace transaction is allowed to touch.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(crate) enum MutationRoot {
    Workspace,
    WorkspaceData,
}

/// A user-data mutation. All paths are relative to one of the two roots.
#[derive(Debug)]
pub(crate) enum FileMutation {
    Write {
        root: MutationRoot,
        relative_path: PathBuf,
        bytes: Vec<u8>,
    },
    Delete {
        root: MutationRoot,
        relative_path: PathBuf,
    },
    Move {
        root: MutationRoot,
        from: PathBuf,
        to: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum JournalKind {
    Write,
    Delete,
    Move,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalOperation {
    kind: JournalKind,
    root: MutationRoot,
    source: Option<PathBuf>,
    target: PathBuf,
    staged: Option<PathBuf>,
    backup: Option<PathBuf>,
    expected_hash: Option<String>,
    applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TransactionJournal {
    version: u32,
    id: String,
    workspace: PathBuf,
    workspace_data: PathBuf,
    state: String,
    operations: Vec<JournalOperation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoveOutcome {
    Removed,
    Missing,
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("cannot atomically write a path without a parent"))?;
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create persistence directory {}",
            parent.display()
        )
    })?;

    let name = file_name_for(path);
    let backup = parent.join(format!(".{name}.backup-{}", Uuid::new_v4()));
    let mut temporary = tempfile::Builder::new()
        .prefix(&format!(".{name}.stage-"))
        .tempfile_in(parent)
        .with_context(|| format!("failed to stage {}", path.display()))?;
    temporary
        .write_all(bytes)
        .with_context(|| format!("failed to write staged {}", path.display()))?;
    temporary
        .as_file()
        .sync_all()
        .with_context(|| format!("failed to flush staged {}", path.display()))?;

    let had_target = path.exists();
    if had_target {
        fs::rename(path, &backup).with_context(|| {
            format!(
                "failed to preserve existing {} before replacement",
                path.display()
            )
        })?;
    }

    if let Err(error) = fs::rename(temporary.path(), path) {
        if had_target {
            let _ = fs::rename(&backup, path);
        }
        return Err(error).with_context(|| format!("failed to install {}", path.display()));
    }

    sync_parent(parent);
    if had_target {
        fs::remove_file(&backup)
            .with_context(|| format!("failed to remove backup for {}", path.display()))?;
    }
    Ok(())
}

pub(crate) fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    atomic_write(path, &serde_json::to_vec_pretty(value)?)
}

pub(crate) fn atomic_remove(path: &Path) -> Result<RemoveOutcome> {
    if !path.exists() {
        return Ok(RemoveOutcome::Missing);
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("cannot atomically remove a path without a parent"))?;
    let name = file_name_for(path);
    let backup = parent.join(format!(".{name}.delete-{}", Uuid::new_v4()));
    fs::rename(path, &backup)
        .with_context(|| format!("failed to stage deletion of {}", path.display()))?;
    if let Err(error) = fs::remove_file(&backup) {
        let _ = fs::rename(&backup, path);
        return Err(error).with_context(|| format!("failed to remove {}", path.display()));
    }
    sync_parent(parent);
    Ok(RemoveOutcome::Removed)
}

pub(crate) struct FileTransaction {
    workspace: PathBuf,
    workspace_data: PathBuf,
    journal_path: PathBuf,
    journal: TransactionJournal,
}

impl FileTransaction {
    pub(crate) fn new(
        workspace: impl Into<PathBuf>,
        workspace_data: impl Into<PathBuf>,
        mutations: Vec<FileMutation>,
    ) -> Result<Self> {
        let workspace = workspace.into();
        let workspace_data = workspace_data.into();
        let id = Uuid::new_v4().to_string();
        let transaction_dir = workspace_data.join("transactions");
        fs::create_dir_all(&transaction_dir)?;
        let journal_path = transaction_dir.join(format!("{id}.json"));
        let mut operations = Vec::with_capacity(mutations.len());

        for (index, mutation) in mutations.into_iter().enumerate() {
            let operation = match mutation {
                FileMutation::Write {
                    root,
                    relative_path,
                    bytes,
                } => {
                    let target = resolve_path(root, &workspace, &workspace_data, &relative_path)?;
                    let parent = target
                        .parent()
                        .ok_or_else(|| anyhow!("write target has no parent"))?;
                    fs::create_dir_all(parent)?;
                    let staged =
                        parent.join(format!(".{}.txn-{id}-{index}", file_name_for(&target)));
                    fs::write(&staged, &bytes)?;
                    File::open(&staged)?.sync_all()?;
                    JournalOperation {
                        kind: JournalKind::Write,
                        root,
                        source: None,
                        target: relative_path,
                        staged: Some(staged),
                        backup: Some(target.with_file_name(format!(
                            ".{}.txn-backup-{}",
                            file_name_for(&target),
                            id
                        ))),
                        expected_hash: Some(hash_bytes(&bytes)),
                        applied: false,
                    }
                }
                FileMutation::Delete {
                    root,
                    relative_path,
                } => {
                    let target = resolve_path(root, &workspace, &workspace_data, &relative_path)?;
                    JournalOperation {
                        kind: JournalKind::Delete,
                        root,
                        source: None,
                        target: relative_path,
                        staged: None,
                        backup: Some(target.with_file_name(format!(
                            ".{}.txn-delete-{}",
                            file_name_for(&target),
                            id
                        ))),
                        expected_hash: None,
                        applied: false,
                    }
                }
                FileMutation::Move { root, from, to } => {
                    resolve_path(root, &workspace, &workspace_data, &from)?;
                    let target = resolve_path(root, &workspace, &workspace_data, &to)?;
                    JournalOperation {
                        kind: JournalKind::Move,
                        root,
                        source: Some(from),
                        target: to,
                        staged: None,
                        backup: Some(target.with_file_name(format!(
                            ".{}.txn-move-{}",
                            file_name_for(&target),
                            id
                        ))),
                        expected_hash: None,
                        applied: false,
                    }
                }
            };
            operations.push(operation);
        }

        Ok(Self {
            workspace: workspace.clone(),
            workspace_data: workspace_data.clone(),
            journal_path,
            journal: TransactionJournal {
                version: 1,
                id,
                workspace: workspace.clone(),
                workspace_data: workspace_data.clone(),
                state: "prepared".into(),
                operations,
            },
        })
    }

    pub(crate) fn commit(mut self) -> Result<()> {
        write_journal(&self.journal_path, &self.journal)?;
        self.journal.state = "applying".into();
        write_journal(&self.journal_path, &self.journal)?;

        for index in 0..self.journal.operations.len() {
            self.apply_operation(index)?;
            self.journal.operations[index].applied = true;
            write_journal(&self.journal_path, &self.journal)?;
        }

        self.journal.state = "committed".into();
        cleanup_journal(&self.journal)?;
        let _ = fs::remove_file(&self.journal_path);
        sync_parent(self.journal_path.parent().unwrap_or(Path::new(".")));
        Ok(())
    }

    fn apply_operation(&mut self, index: usize) -> Result<()> {
        let operation = &mut self.journal.operations[index];
        let root_path = resolve_root(operation.root, &self.workspace, &self.workspace_data);
        let target = root_path.join(&operation.target);
        match operation.kind {
            JournalKind::Write => {
                let staged = operation
                    .staged
                    .as_ref()
                    .ok_or_else(|| anyhow!("write transaction is missing its staged file"))?;
                if let Some(expected_hash) = &operation.expected_hash {
                    if target.exists() && hash_file(&target).ok().as_deref() == Some(expected_hash)
                    {
                        return Ok(());
                    }
                }
                if target.exists() {
                    let backup = operation
                        .backup
                        .as_ref()
                        .ok_or_else(|| anyhow!("write transaction is missing its backup path"))?;
                    if !backup.exists() {
                        fs::rename(&target, backup)
                            .with_context(|| format!("failed to preserve {}", target.display()))?;
                    }
                }
                if let Err(error) = fs::rename(staged, &target) {
                    if let Some(backup) = operation.backup.as_ref() {
                        if backup.exists() {
                            let _ = fs::rename(backup, &target);
                        }
                    }
                    return Err(error)
                        .with_context(|| format!("failed to install {}", target.display()));
                }
            }
            JournalKind::Delete => {
                if !target.exists() {
                    return Ok(());
                }
                let backup = operation
                    .backup
                    .as_ref()
                    .ok_or_else(|| anyhow!("delete transaction is missing its backup path"))?;
                if !backup.exists() {
                    fs::rename(&target, backup).with_context(|| {
                        format!("failed to stage deletion of {}", target.display())
                    })?;
                }
            }
            JournalKind::Move => {
                let source = root_path.join(
                    operation
                        .source
                        .as_ref()
                        .ok_or_else(|| anyhow!("move transaction is missing its source"))?,
                );
                if !source.exists() && target.exists() {
                    return Ok(());
                }
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                if target.exists() {
                    let backup = operation
                        .backup
                        .as_ref()
                        .ok_or_else(|| anyhow!("move transaction is missing its backup path"))?;
                    if !backup.exists() {
                        fs::rename(&target, backup)?;
                    }
                }
                fs::rename(&source, &target).with_context(|| {
                    format!(
                        "failed to move {} to {}",
                        source.display(),
                        target.display()
                    )
                })?;
            }
        }
        Ok(())
    }
}

pub(crate) fn recover_transactions(workspace_data: &Path) -> Result<Vec<anyhow::Error>> {
    let transaction_dir = workspace_data.join("transactions");
    let mut errors = Vec::new();
    let Ok(entries) = fs::read_dir(&transaction_dir) else {
        return Ok(errors);
    };
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        match fs::read(&entry.path()).and_then(|bytes| {
            serde_json::from_slice::<TransactionJournal>(&bytes).map_err(std::io::Error::other)
        }) {
            Ok(mut journal) => {
                if let Err(error) = recover_journal(&mut journal, &entry.path()).and_then(|_| {
                    cleanup_journal(&journal)
                        .and_then(|_| fs::remove_file(entry.path()).map_err(Into::into))
                }) {
                    errors.push(error.context("failed to recover persistence transaction"));
                }
            }
            Err(error) => {
                let quarantine = transaction_dir.join("quarantine");
                let _ = fs::create_dir_all(&quarantine);
                let destination = quarantine.join(format!(
                    "{}-{}",
                    file_name_for(&entry.path()),
                    Uuid::new_v4()
                ));
                if let Err(move_error) = fs::rename(entry.path(), destination) {
                    errors.push(anyhow!("malformed persistence journal could not be quarantined: {error}; {move_error}"));
                } else {
                    errors.push(anyhow!(
                        "malformed persistence journal was quarantined: {error}"
                    ));
                }
            }
        }
    }
    Ok(errors)
}

fn recover_journal(journal: &mut TransactionJournal, journal_path: &Path) -> Result<()> {
    if journal.state == "committed" {
        return Ok(());
    }
    journal.state = "applying".into();
    write_journal(journal_path, journal)?;
    for index in 0..journal.operations.len() {
        if journal.operations[index].applied {
            continue;
        }
        let mut transaction = FileTransaction {
            workspace: journal.workspace.clone(),
            workspace_data: journal.workspace_data.clone(),
            journal_path: PathBuf::new(),
            journal: journal.clone(),
        };
        transaction.apply_operation(index)?;
        journal.operations[index] = transaction.journal.operations[index].clone();
        journal.operations[index].applied = true;
        write_journal(journal_path, journal)?;
    }
    journal.state = "committed".into();
    write_journal(journal_path, journal)?;
    Ok(())
}

fn cleanup_journal(journal: &TransactionJournal) -> Result<()> {
    for operation in &journal.operations {
        if let Some(backup) = &operation.backup {
            if backup.exists() {
                fs::remove_file(backup)?;
            }
        }
        if let Some(staged) = &operation.staged {
            if staged.exists() {
                fs::remove_file(staged)?;
            }
        }
    }
    Ok(())
}

fn write_journal(path: &Path, journal: &TransactionJournal) -> Result<()> {
    atomic_write(path, &serde_json::to_vec_pretty(journal)?)
}

fn resolve_root<'a>(root: MutationRoot, workspace: &'a Path, workspace_data: &'a Path) -> &'a Path {
    match root {
        MutationRoot::Workspace => workspace,
        MutationRoot::WorkspaceData => workspace_data,
    }
}

fn resolve_path(
    root: MutationRoot,
    workspace: &Path,
    workspace_data: &Path,
    relative: &Path,
) -> Result<PathBuf> {
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(anyhow!("persistence path must remain relative to its root"));
    }
    Ok(resolve_root(root, workspace, workspace_data).join(relative))
}

fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn hash_file(path: &Path) -> Result<String> {
    Ok(hash_bytes(&fs::read(path).with_context(|| {
        format!("failed to verify {}", path.display())
    })?))
}

fn file_name_for(path: &Path) -> &str {
    path.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("file")
}

fn sync_parent(path: &Path) {
    #[cfg(unix)]
    if let Ok(file) = File::open(path) {
        let _ = file.sync_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_preserves_previous_bytes_on_successful_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        atomic_write(&path, b"old").unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn transaction_writes_moves_and_deletes_files() {
        let workspace = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let source = workspace.path().join("source.md");
        fs::write(&source, b"old").unwrap();
        FileTransaction::new(
            workspace.path(),
            data.path(),
            vec![
                FileMutation::Write {
                    root: MutationRoot::Workspace,
                    relative_path: PathBuf::from("new.json"),
                    bytes: b"new".to_vec(),
                },
                FileMutation::Move {
                    root: MutationRoot::Workspace,
                    from: PathBuf::from("source.md"),
                    to: PathBuf::from("moved.md"),
                },
                FileMutation::Delete {
                    root: MutationRoot::Workspace,
                    relative_path: PathBuf::from("new.json"),
                },
            ],
        )
        .unwrap()
        .commit()
        .unwrap();
        assert!(!source.exists());
        assert!(workspace.path().join("moved.md").exists());
        assert!(!workspace.path().join("new.json").exists());
        assert_eq!(
            fs::read_dir(data.path().join("transactions"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn transaction_rejects_parent_escape() {
        let workspace = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let result = FileTransaction::new(
            workspace.path(),
            data.path(),
            vec![FileMutation::Delete {
                root: MutationRoot::Workspace,
                relative_path: PathBuf::from("../outside"),
            }],
        );
        assert!(result.is_err());
    }

    #[test]
    fn recovery_rolls_forward_an_interrupted_write_and_is_idempotent() {
        let workspace = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let target = workspace.path().join("note.md");
        fs::write(&target, b"old").unwrap();
        let transaction_id = "interrupted-write";
        let staged = workspace.path().join(".note.md.txn-stage");
        fs::write(&staged, b"new").unwrap();
        let backup = workspace
            .path()
            .join(format!(".note.md.txn-backup-{transaction_id}"));
        let journal_path = data
            .path()
            .join("transactions")
            .join(format!("{transaction_id}.json"));
        fs::create_dir_all(journal_path.parent().unwrap()).unwrap();
        write_journal(
            &journal_path,
            &TransactionJournal {
                version: 1,
                id: transaction_id.into(),
                workspace: workspace.path().into(),
                workspace_data: data.path().into(),
                state: "applying".into(),
                operations: vec![JournalOperation {
                    kind: JournalKind::Write,
                    root: MutationRoot::Workspace,
                    source: None,
                    target: PathBuf::from("note.md"),
                    staged: Some(staged.clone()),
                    backup: Some(backup.clone()),
                    expected_hash: Some(hash_bytes(b"new")),
                    applied: false,
                }],
            },
        )
        .unwrap();

        assert!(recover_transactions(data.path()).unwrap().is_empty());
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(!staged.exists());
        assert!(!backup.exists());
        assert!(!journal_path.exists());
        assert!(recover_transactions(data.path()).unwrap().is_empty());
        assert_eq!(fs::read(&target).unwrap(), b"new");
    }
}
