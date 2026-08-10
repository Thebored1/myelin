use super::*;

    pub(crate) fn disk_test_document(relative_path: &str, body: &str) -> NoteDocument {
        NoteDocument {
            id: "native-note-id".to_string(),
            title: "Native title".to_string(),
            tags: vec!["format-safe".to_string()],
            body: body.to_string(),
            relative_path: relative_path.to_string(),
            created_at: "2026-08-08T10:00:00Z".to_string(),
            updated_at: "2026-08-08T11:00:00Z".to_string(),
            ..Default::default()
        }
    }
