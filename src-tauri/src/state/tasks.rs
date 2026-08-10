use super::core::*;
use ::anyhow::{anyhow, Context, Result};
use super::*;
use crate::persistence::{FileMutation, FileTransaction, MutationRoot};

impl AppState {
    pub fn list_tasks(&self) -> Result<Vec<Task>> {
        let workspace = self.require_workspace()?;
        let mut tasks = std::collections::HashMap::<String, (Task, PathBuf)>::new();
        let mut issues = Vec::new();
        for entry in walkdir::WalkDir::new(&workspace)
            .into_iter()
            .filter_entry(|e| !is_hidden_or_ignored(e))
        {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    issues.push(StorageIssue {
                        code: "task-traversal".into(), severity: "error".into(),
                        path: error.path().map(|path| path.display().to_string()),
                        message: "A task directory could not be scanned.".into(), recoverable: true,
                    });
                    continue;
                }
            };
            let path = entry.path();
            if !is_task_file(path) {
                continue;
            }
            let raw = match fs::read_to_string(path) {
                Ok(raw) => raw,
                Err(_) => {
                    issues.push(StorageIssue {
                        code: "task-read".into(), severity: "error".into(),
                        path: Some(path.display().to_string()),
                        message: "A task file could not be read.".into(), recoverable: true,
                    });
                    continue;
                }
            };
            let mut task = match serde_json::from_str::<Task>(&raw) {
                Ok(task) => task,
                Err(_) => {
                    issues.push(StorageIssue {
                        code: "task-parse".into(), severity: "error".into(),
                        path: Some(path.display().to_string()),
                        message: "A task file contains invalid JSON and was left untouched.".into(), recoverable: true,
                    });
                    continue;
                }
            };
            // The file's location is the source of truth for the notebook (a task
            // file copied into another notebook folder belongs to that notebook).
            task.notebook = notebook_from_task_path(&workspace, path);
            if let Some((existing, existing_path)) = tasks.get(&task.id) {
                issues.push(StorageIssue {
                    code: "duplicate-task".into(), severity: "error".into(),
                    path: Some(path.display().to_string()),
                    message: format!("This task ID also exists at {}.", existing_path.display()), recoverable: true,
                });
                if task.updated_at > existing.updated_at {
                    tasks.insert(task.id.clone(), (task, path.to_path_buf()));
                }
            } else {
                tasks.insert(task.id.clone(), (task, path.to_path_buf()));
            }
        }
        self.replace_storage_issues_matching(issues, |issue| {
            issue.code.starts_with("task-") || issue.code == "duplicate-task"
        });
        let mut tasks = tasks.into_values().map(|(task, _)| task).collect::<Vec<_>>();
        tasks.sort_by(|a, b| match (a.position, b.position) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.created_at.cmp(&b.created_at),
        });
        Ok(tasks)
    }

    /// Create (empty id) or update a task, writing its JSON file. Moving it between
    /// notebooks (changing `notebook`) relocates the file. Returns the saved task.
    pub fn save_task(&self, mut task: Task) -> Result<Task> {
        let workspace = self.require_workspace()?;
        let _persistence_guard = self.inner.persistence_lock.lock();
        task.notebook = task
            .notebook
            .map(|s| s.trim().replace('\\', "/"))
            .filter(|s| !s.is_empty());
        if let Some(nb) = &task.notebook {
            validate_relative_dir(nb)?;
        }
        if task.id.trim().is_empty() {
            task.id = Uuid::new_v4().to_string();
        }
        validate_task_id(&task.id)?;
        let existing_paths = task_files_for(&workspace, &task.id);
        let mut stored_tasks = Vec::new();
        for path in &existing_paths {
            let raw = fs::read_to_string(path).with_context(|| {
                format!("failed to read existing task file {}", path.display())
            })?;
            let stored = serde_json::from_str::<Task>(&raw).map_err(|_| {
                anyhow!(
                    "task file {} is malformed; repair it before saving this task",
                    path.display()
                )
            })?;
            stored_tasks.push(stored);
        }
        let latest_stored = stored_tasks.into_iter().map(|stored| stored.updated_at).max();
        if !task.updated_at.trim().is_empty()
            && latest_stored.as_deref().is_some_and(|latest| task.updated_at < latest.to_string())
        {
            return Err(anyhow!("task update is older than the latest persisted task"));
        }
        let now = Utc::now().to_rfc3339();
        if task.created_at.trim().is_empty() {
            task.created_at = now.clone();
        }
        task.updated_at = now;
        for sub in task.subtasks.iter_mut() {
            if sub.id.trim().is_empty() {
                sub.id = Uuid::new_v4().to_string();
            }
        }
        let dir = task_dir_for(&workspace, task.notebook.as_deref());
        let target = dir.join(format!("{}.json", task.id));
        let bytes = serde_json::to_vec_pretty(&task)?;
        if existing_paths.len() > 1 {
            return Err(anyhow!("task ID is ambiguous because multiple task files exist"));
        }
        let relative_target = relative_to_workspace(&workspace, &target);
        let mutations = std::iter::once(FileMutation::Write {
                root: MutationRoot::Workspace,
                relative_path: PathBuf::from(relative_target),
                bytes,
            })
            .chain(existing_paths
                .into_iter()
                .filter(|path| path != &target)
                .map(|path| FileMutation::Delete {
                    root: MutationRoot::Workspace,
                    relative_path: PathBuf::from(relative_to_workspace(&workspace, &path)),
                }))
            .collect();
        FileTransaction::new(&workspace, self.workspace_data_dir(&workspace), mutations)?.commit()?;
        let _ = self.handle.emit("tasks://changed", ());
        Ok(task)
    }

    pub fn delete_task(&self, id: String) -> Result<()> {
        let workspace = self.require_workspace()?;
        let _persistence_guard = self.inner.persistence_lock.lock();
        validate_task_id(&id)?;
        let mutations = task_files_for(&workspace, &id)
            .into_iter()
            .map(|path| FileMutation::Delete {
                root: MutationRoot::Workspace,
                relative_path: PathBuf::from(relative_to_workspace(&workspace, &path)),
            })
            .collect();
        FileTransaction::new(&workspace, self.workspace_data_dir(&workspace), mutations)?.commit()?;
        let _ = self.handle.emit("tasks://changed", ());
        Ok(())
    }
}
