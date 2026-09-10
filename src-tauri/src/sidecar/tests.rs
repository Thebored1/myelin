use super::recovery::{conversation_delta, recoverable_assistant_text};
use serde_json::json;

#[test]
fn legacy_full_conversation_is_reduced_to_delta() {
    let submitted = vec![
        json!({"role": "system", "content": "policy"}),
        json!({"role": "user", "content": "hello"}),
    ];
    let mut returned = submitted.clone();
    returned.push(json!({"role": "assistant", "content": "hi"}));
    let delta = conversation_delta(&submitted, &returned).unwrap();
    assert_eq!(delta, vec![json!({"role": "assistant", "content": "hi"})]);
}

#[test]
fn incompatible_full_conversation_is_rejected() {
    let submitted = vec![json!({"role": "user", "content": "hello"})];
    let returned = vec![json!({"role": "system", "content": "wrong"})];
    assert!(conversation_delta(&submitted, &returned).is_err());
}

#[test]
fn empty_current_turn_never_recovers_an_older_answer() {
    let current = vec![json!({"role": "assistant", "content": ""})];
    assert!(recoverable_assistant_text(&current).is_none());
}

#[test]
fn recovery_uses_the_latest_current_assistant_message() {
    let current = vec![
        json!({"role": "user", "content": "new question"}),
        json!({"role": "assistant", "content": "new answer"}),
    ];
    assert_eq!(
        recoverable_assistant_text(&current).as_deref(),
        Some("new answer")
    );
}
