use serde_json::Value;

/// Split a multi-operation request into an ordered tool plan. The harness
/// supplies structure while the model supplies arguments for one operation at
/// a time, which is more reliable for small quantized models.
pub fn harness_decompose(request: &str, tools: &Value) -> Vec<(String, String)> {
    let req = request.trim();
    if req.is_empty() {
        return Vec::new();
    }

    let clause_split = regex::Regex::new(
        r"(?i)\b(?:,|;| and | plus | then | also | as well as | along with |&|/)\b",
    )
    .unwrap();
    let mut clauses: Vec<String> = clause_split
        .split(req)
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    if clauses.is_empty() {
        clauses.push(req.to_string());
    }

    let word_re = regex::Regex::new(r"[a-z0-9_]+").unwrap();
    let mut tool_kw: Vec<(String, Vec<String>)> = Vec::new();
    if let Some(arr) = tools.as_array() {
        for t in arr {
            let f = if t["function"].is_object() { &t["function"] } else { t };
            let name = f["name"].as_str().unwrap_or("").to_string();
            if name.is_empty() {
                continue;
            }
            let mut kw = Vec::new();
            let description = f["description"].as_str().unwrap_or("").to_lowercase();
            kw.extend(word_re.find_iter(&description).map(|m| m.as_str().to_string()));
            kw.extend(name.split(['.', '_']).map(str::to_string));
            if let Some(props) = f["parameters"]["properties"].as_object() {
                for (property, spec) in props {
                    kw.extend(property.split(['.', '_']).map(str::to_string));
                    if let Some(enums) = spec["enum"].as_array() {
                        for value in enums.iter().filter_map(Value::as_str) {
                            kw.extend(
                                word_re
                                    .find_iter(&value.to_lowercase())
                                    .map(|m| m.as_str().to_string()),
                            );
                        }
                    }
                }
            }
            tool_kw.push((name, kw));
        }
    }

    fn score(clause_words: &[String], tool_words: &[String]) -> f64 {
        if clause_words.is_empty() || tool_words.is_empty() {
            return 0.0;
        }
        let hit = clause_words
            .iter()
            .filter(|word| word.len() >= 3 && tool_words.iter().any(|tool| tool == *word))
            .count();
        hit as f64 / clause_words.len() as f64
    }

    let mut plan = Vec::new();
    for clause in clauses {
        let words: Vec<String> = word_re
            .find_iter(&clause.to_lowercase())
            .map(|m| m.as_str().to_string())
            .collect();
        let best = tool_kw
            .iter()
            .map(|(name, keywords)| (score(&words, keywords), name))
            .filter(|(score, _)| *score > 0.0)
            .max_by(|left, right| left.0.total_cmp(&right.0));
        if let Some((_, name)) = best {
            plan.push((name.clone(), clause));
        }
    }
    plan
}
