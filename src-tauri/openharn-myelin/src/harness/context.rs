use serde_json::Value;

/// A single tool result can be huge; cap it so one result can't blow the context.
pub const TOOL_RESULT_CAP: usize = 4_000;

pub fn cap_result_with(mut s: String, cap: usize) -> String {
    if s.chars().count() > cap {
        s = s.chars().take(cap).collect();
        s.push_str(
            "\n…[result truncated — narrow your search (a more specific query) if you need more]",
        );
    }
    s
}

/// Trim history by dropping only complete old turns. The system message and
/// the current user turn are fixed context and are never removed.
pub fn fit_context(history: &mut Vec<Value>, max_chars: usize) -> bool {
    if history.len() < 3 {
        return false;
    }
    let fixed_len = if history[0]["role"] == "system" {
        history[0].to_string().len()
    } else {
        0
    };
    let total = |h: &[Value]| -> usize {
        h.iter()
            .map(|m| m.to_string().len())
            .sum::<usize>()
            .saturating_sub(fixed_len)
    };
    let mut changed = false;
    while total(history) > max_chars && history.len() > 3 {
        let Some(current_user) = history.iter().rposition(|m| m["role"] == "user") else {
            break;
        };
        let mut end = 2;
        while end < current_user && history[end]["role"] != "user" {
            end += 1;
        }
        if end >= history.len() || end > current_user {
            break;
        }
        history.drain(1..end);
        changed = true;
    }
    changed
}

/// Remove leaked reasoning tags when the model is configured not to think.
pub fn strip_think(s: &str) -> String {
    let tail = match s.rfind("</think>") {
        Some(i) => &s[i + "</think>".len()..],
        None => s,
    };
    tail.replace("<think>", "")
        .replace("</think>", "")
        .trim()
        .to_string()
}
