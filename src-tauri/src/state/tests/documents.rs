use super::*;

#[test]
fn slugify_avoids_reserved_names() {
    assert_eq!(slugify("CON"), "con-note");
    assert_eq!(slugify("Hello World"), "hello-world");
}

#[test]
fn pdf_import_names_increment_without_case_collisions() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Paper.pdf"), b"one").unwrap();
    std::fs::write(dir.path().join("paper 1.pdf"), b"two").unwrap();
    let path = unique_pdf_path(dir.path(), std::ffi::OsStr::new("paper.pdf"));
    assert_eq!(path.file_name().unwrap(), "paper 2.pdf");
}

#[test]
fn frontmatter_split_handles_markdown() {
    let raw = "---\ntitle: Test\n---\n\n# Hello";
    let (frontmatter, body) = split_frontmatter(raw);
    assert!(frontmatter.is_some());
    assert_eq!(body, "# Hello");
}
#[test]
fn new_ipynb_body_is_a_canonical_valid_notebook() {
    let body = initial_note_body(std::path::Path::new("new.ipynb"));
    assert_eq!(body, EMPTY_IPYNB);
    validate_native_body(std::path::Path::new("new.ipynb"), &body).unwrap();
    let notebook: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(notebook["cells"], serde_json::json!([]));
    assert_eq!(notebook["metadata"], serde_json::json!({}));
    assert_eq!(notebook["nbformat"], 4);
    assert_eq!(notebook["nbformat_minor"], 5);
    assert!(initial_note_body(std::path::Path::new("new.tex")).is_empty());
}

#[test]
fn duplicate_extension_preserves_native_formats_only() {
    assert_eq!(
        duplicate_note_extension(std::path::Path::new("paper.tex")),
        "tex"
    );
    assert_eq!(
        duplicate_note_extension(std::path::Path::new("analysis.IPYNB")),
        "IPYNB"
    );
    assert_eq!(
        duplicate_note_extension(std::path::Path::new("note.md")),
        "md"
    );
    assert_eq!(
        duplicate_note_extension(std::path::Path::new("paper.pdf")),
        "md"
    );
}

#[test]
fn tex_disk_round_trip_keeps_native_source_and_app_data_metadata() {
    let workspace = tempfile::tempdir().unwrap();
    let app_data = tempfile::tempdir().unwrap();
    let path = workspace.path().join("paper.tex");
    let body = "\\documentclass{article}\n\\begin{document}\nHello\n\\end{document}\n";
    let document = disk_test_document("paper.tex", body);

    write_note_file(workspace.path(), app_data.path(), &path, &document).unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
    assert!(native_metadata_app_path(workspace.path(), app_data.path(), &path).is_file());
    let loaded = parse_note_file(workspace.path(), app_data.path(), &path).unwrap();
    assert_eq!(loaded.id, document.id);
    assert_eq!(loaded.title, document.title);
    assert_eq!(loaded.tags, document.tags);
    assert_eq!(loaded.body, body);
}

#[test]
fn legacy_wrapped_ipynb_migrates_to_raw_json_and_round_trips_metadata() {
    let workspace = tempfile::tempdir().unwrap();
    let app_data = tempfile::tempdir().unwrap();
    let path = workspace.path().join("legacy.ipynb");
    let body = r#"{"cells":[{"cell_type":"markdown","metadata":{},"source":["hello"]}],"metadata":{},"nbformat":4,"nbformat_minor":5}"#;
    let document = disk_test_document("legacy.ipynb", body);
    let metadata = super::frontmatter_from_document(&document);
    let yaml = serde_yaml::to_string(&metadata).unwrap();
    std::fs::write(&path, format!("---\n{}\n---\n\n{body}", yaml.trim_end())).unwrap();

    let migrated = parse_note_file(workspace.path(), app_data.path(), &path).unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    serde_json::from_str::<serde_json::Value>(&raw).unwrap();
    assert_eq!(raw, body);
    assert_eq!(migrated.id, document.id);
    assert_eq!(migrated.title, document.title);
    assert_eq!(migrated.tags, document.tags);
    assert!(native_metadata_app_path(workspace.path(), app_data.path(), &path).is_file());

    let loaded_again = parse_note_file(workspace.path(), app_data.path(), &path).unwrap();
    assert_eq!(loaded_again.id, document.id);
    assert_eq!(loaded_again.body, body);
}

#[test]
fn legacy_wrapped_tex_migrates_without_changing_tex_body() {
    let workspace = tempfile::tempdir().unwrap();
    let app_data = tempfile::tempdir().unwrap();
    let path = workspace.path().join("legacy.tex");
    let body = "\\section{Legacy}\nBody with trailing newline.\n";
    let document = disk_test_document("legacy.tex", body);
    let metadata = super::frontmatter_from_document(&document);
    let yaml = serde_yaml::to_string(&metadata).unwrap();
    std::fs::write(&path, format!("---\n{}\n---\n\n{body}", yaml.trim_end())).unwrap();

    let migrated = parse_note_file(workspace.path(), app_data.path(), &path).unwrap();

    assert_eq!(migrated.id, document.id);
    assert_eq!(migrated.body, body);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
    assert!(native_metadata_app_path(workspace.path(), app_data.path(), &path).is_file());
}

#[test]
fn markdown_disk_round_trip_retains_yaml_frontmatter_behavior() {
    let workspace = tempfile::tempdir().unwrap();
    let app_data = tempfile::tempdir().unwrap();
    let path = workspace.path().join("note.md");
    let document = disk_test_document("note.md", "# Heading\n\nMarkdown body\n");

    write_note_file(workspace.path(), app_data.path(), &path, &document).unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.starts_with("---\n"));
    assert!(raw.contains("title: Native title"));
    let loaded = parse_note_file(workspace.path(), app_data.path(), &path).unwrap();
    assert_eq!(loaded.id, document.id);
    assert_eq!(loaded.title, document.title);
    // Markdown writes historically trim trailing whitespace; keep that
    // behavior while native formats preserve their bodies byte-for-byte.
    assert_eq!(loaded.body, document.body.trim_end());
}

#[test]
fn ipynb_writer_rejects_invalid_json_without_touching_disk() {
    let workspace = tempfile::tempdir().unwrap();
    let app_data = tempfile::tempdir().unwrap();
    let path = workspace.path().join("notebook.ipynb");
    std::fs::write(&path, EMPTY_IPYNB).unwrap();
    for invalid in [
        "{ invalid json",
        "[]",
        r#"{"nbformat":4,"nbformat_minor":5}"#,
        r#"{"cells":[],"nbformat":"four","nbformat_minor":5}"#,
    ] {
        let document = disk_test_document("notebook.ipynb", invalid);
        assert!(write_note_file(workspace.path(), app_data.path(), &path, &document).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), EMPTY_IPYNB);
        assert!(!native_metadata_app_path(workspace.path(), app_data.path(), &path).exists());
    }
}

#[test]
fn pdf_and_epub_writes_never_replace_binary_payloads() {
    let workspace = tempfile::tempdir().unwrap();
    let app_data = tempfile::tempdir().unwrap();
    for extension in ["pdf", "epub"] {
        let path = workspace.path().join(format!("source.{extension}"));
        let payload = [0_u8, 159, 255, 13, 10, 42];
        std::fs::write(&path, payload).unwrap();
        let document = disk_test_document(
            path.file_name().unwrap().to_str().unwrap(),
            "replacement text",
        );

        write_note_file(workspace.path(), app_data.path(), &path, &document).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), payload);
    }
}

#[test]
fn native_metadata_sidecar_tracks_path_lifecycle() {
    let workspace = tempfile::tempdir().unwrap();
    let app_data = tempfile::tempdir().unwrap();
    let source = workspace.path().join("source.tex");
    let target = workspace.path().join("folder").join("target.tex");
    let document = disk_test_document("source.tex", "Body");

    write_native_metadata_sidecar(workspace.path(), app_data.path(), &source, &document).unwrap();
    let source_sidecar = native_metadata_app_path(workspace.path(), app_data.path(), &source);
    assert!(source_sidecar.is_file());

    write_native_metadata_sidecar(workspace.path(), app_data.path(), &target, &document).unwrap();
    remove_native_metadata_sidecar(workspace.path(), app_data.path(), &source);
    assert!(!source_sidecar.exists());
    assert!(native_metadata_app_path(workspace.path(), app_data.path(), &target).is_file());

    remove_native_metadata_sidecar(workspace.path(), app_data.path(), &target);
    assert!(!native_metadata_app_path(workspace.path(), app_data.path(), &target).exists());
}
