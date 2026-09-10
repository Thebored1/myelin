use super::documents_helpers::*;
use super::*;
use crate::persistence::{FileMutation, FileTransaction, MutationRoot};

pub(crate) fn write_note_file(
    workspace: &Path,
    data: &Path,
    path: &Path,
    document: &NoteDocument,
) -> Result<()> {
    if !is_binary_document(path) {
        validate_native_body(path, &document.body)?;
    }
    FileTransaction::new(
        workspace,
        data,
        note_write_plan(workspace, data, path, document)?,
    )?
    .commit()
}

pub(crate) fn note_write_plan(
    workspace: &Path,
    data: &Path,
    path: &Path,
    document: &NoteDocument,
) -> Result<Vec<FileMutation>> {
    let relative = |absolute: PathBuf| -> Result<PathBuf> {
        absolute
            .strip_prefix(data)
            .map(PathBuf::from)
            .map_err(|_| anyhow!("sidecar path escaped workspace data directory"))
    };
    let mut mutations = Vec::new();
    let annotations = data
        .join("annotations")
        .join(format!("{}.annotations.json", document.id));
    mutations.push(if document.annotations.is_empty() {
        FileMutation::Delete {
            root: MutationRoot::WorkspaceData,
            relative_path: relative(annotations)?,
        }
    } else {
        FileMutation::Write {
            root: MutationRoot::WorkspaceData,
            relative_path: relative(annotations)?,
            bytes: serde_json::to_vec_pretty(&document.annotations)?,
        }
    });
    let chat = data
        .join("chats")
        .join(format!("{}.chat.json", document.id));
    mutations.push(if document.chat_history.is_empty() {
        FileMutation::Delete {
            root: MutationRoot::WorkspaceData,
            relative_path: relative(chat)?,
        }
    } else {
        FileMutation::Write {
            root: MutationRoot::WorkspaceData,
            relative_path: relative(chat)?,
            bytes: serde_json::to_vec_pretty(&document.chat_history)?,
        }
    });
    if is_metadata_document(path) {
        let metadata = DocumentMetadataSidecar {
            schema_version: 2,
            metadata: frontmatter_from_document(document),
            relative_path: relative_to_workspace(workspace, path),
        };
        let metadata_path = native_metadata_app_path(workspace, data, path);
        mutations.push(FileMutation::Write {
            root: MutationRoot::WorkspaceData,
            relative_path: relative(metadata_path)?,
            bytes: serde_json::to_vec_pretty(&metadata)?,
        });
    }
    if is_binary_document(path) {
        return Ok(mutations);
    }
    let bytes = if is_native_text_file(path) {
        document.body.as_bytes().to_vec()
    } else {
        let yaml = serde_yaml::to_string(&frontmatter_from_document(document))?
            .trim()
            .to_string();
        format!("---\n{yaml}\n---\n\n{}", document.body.trim_end()).into_bytes()
    };
    mutations.push(FileMutation::Write {
        root: MutationRoot::Workspace,
        relative_path: PathBuf::from(relative_to_workspace(workspace, path)),
        bytes,
    });
    Ok(mutations)
}

pub(crate) fn note_delete_plan(
    workspace: &Path,
    data: &Path,
    path: &Path,
    id: &str,
) -> Result<Vec<FileMutation>> {
    let relative = |absolute: PathBuf| -> Result<PathBuf> {
        absolute
            .strip_prefix(data)
            .map(PathBuf::from)
            .map_err(|_| anyhow!("sidecar path escaped workspace data directory"))
    };
    let mut mutations = vec![FileMutation::Delete {
        root: MutationRoot::Workspace,
        relative_path: PathBuf::from(relative_to_workspace(workspace, path)),
    }];
    for sidecar in [
        data.join("annotations")
            .join(format!("{id}.annotations.json")),
        data.join("chats").join(format!("{id}.chat.json")),
    ] {
        mutations.push(FileMutation::Delete {
            root: MutationRoot::WorkspaceData,
            relative_path: relative(sidecar)?,
        });
    }
    if is_metadata_document(path) {
        mutations.push(FileMutation::Delete {
            root: MutationRoot::WorkspaceData,
            relative_path: relative(native_metadata_app_path(workspace, data, path))?,
        });
        mutations.push(FileMutation::Delete {
            root: MutationRoot::Workspace,
            relative_path: PathBuf::from(relative_to_workspace(
                workspace,
                &workspace
                    .join(".myelin")
                    .join(NATIVE_METADATA_DIR)
                    .join(native_metadata_file_name(workspace, path)),
            )),
        });
    }
    Ok(mutations)
}
