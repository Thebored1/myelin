use super::{dispatch_tool, normalize_lfm_tool_arguments};
use crate::policy::{is_mutating_tool, is_terminal_mutation, should_stream_note_preview};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, watch, Mutex};

#[test]
fn lfm_history_uses_mapping_arguments_for_native_template_rendering() {
    let mut messages = vec![json!({
        "role": "assistant",
        "content": null,
        "tool_calls": [{
            "type": "function",
            "function": {
                "name": "write_note",
                "arguments": "{\"content\":\"hello\",\"mode\":\"replace\"}"
            }
        }]
    })];
    normalize_lfm_tool_arguments(&mut messages, "LFM2-2.6B-Tool-Q4.gguf");
    assert_eq!(
        messages[0]["tool_calls"][0]["function"]["arguments"]["content"],
        "hello"
    );
}

#[test]
fn non_lfm_history_keeps_openai_argument_strings() {
    let mut messages = vec![json!({
        "tool_calls": [{
            "function": {"name": "write_note", "arguments": "{\"content\":\"hello\"}"}
        }]
    })];
    normalize_lfm_tool_arguments(&mut messages, "qwen.gguf");
    assert!(messages[0]["tool_calls"][0]["function"]["arguments"].is_string());
}

#[test]
fn successful_note_write_is_terminal() {
    assert!(is_terminal_mutation(
        "write_note",
        "Note successfully updated with ID: 90dc6"
    ));
}

#[test]
fn failed_or_readonly_tool_result_is_not_terminal() {
    assert!(!is_terminal_mutation(
        "write_note",
        "No note is currently open to write to."
    ));
    assert!(!is_terminal_mutation(
        "read_note",
        "Note successfully updated"
    ));
}

#[test]
fn mutating_tools_are_identified_for_operation_abort() {
    assert!(is_mutating_tool("write_note"));
    assert!(is_mutating_tool("replace_in_note"));
    assert!(!is_mutating_tool("search_notes"));
}

#[test]
fn other_successful_mutations_are_terminal() {
    assert!(is_terminal_mutation(
        "format_note",
        " Note successfully updated."
    ));
    assert!(is_terminal_mutation(
        "edit_notebook",
        "Notebook cell updated."
    ));
    assert!(is_terminal_mutation(
        "append_note",
        "Note successfully updated with ID: 42"
    ));
    assert!(is_terminal_mutation(
        "prepend_note",
        "Note successfully updated"
    ));
    assert!(is_terminal_mutation(
        "replace_in_note",
        "Note successfully updated with ID: 1"
    ));
    assert!(is_terminal_mutation(
        "insert_after_line",
        "Note successfully updated with ID: 1"
    ));
    assert!(is_terminal_mutation(
        "delete_in_note",
        "Note successfully updated with ID: 1"
    ));
}

#[test]
fn selection_scoped_writes_always_enable_safe_preview_streaming() {
    assert!(should_stream_note_preview("insert text here", true));
    assert!(should_stream_note_preview("add below the selection", true));
}

#[tokio::test]
async fn pending_tool_wait_is_cancelled_and_removed() {
    let pending = Arc::new(Mutex::new(HashMap::new()));
    let (events, mut received) = mpsc::channel(4);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let pending_for_task = pending.clone();
    let task = tokio::spawn(async move {
        dispatch_tool(
            &events,
            &pending_for_task,
            "request",
            "call",
            "write_note",
            "{}",
            30,
            &cancel_rx,
        )
        .await
    });
    let _ = received.recv().await;
    cancel_tx.send(true).unwrap();
    let result = task.await.unwrap();
    assert!(result.contains("cancelled"));
    assert!(pending.lock().await.is_empty());
}

#[tokio::test]
async fn pending_tool_wait_times_out_and_is_removed() {
    let pending = Arc::new(Mutex::new(HashMap::new()));
    let (events, mut received) = mpsc::channel(4);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let pending_for_task = pending.clone();
    let task = tokio::spawn(async move {
        dispatch_tool(
            &events,
            &pending_for_task,
            "request",
            "call",
            "read_note",
            "{}",
            0,
            &cancel_rx,
        )
        .await
    });
    let _ = received.recv().await;
    let result = task.await.unwrap();
    assert!(result.contains("timed out"));
    assert!(pending.lock().await.is_empty());
}
