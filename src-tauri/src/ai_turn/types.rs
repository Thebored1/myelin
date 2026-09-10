use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnKind {
    DirectAnswer,
    ToolSelection,
}

#[derive(Debug, Clone)]
pub struct AiTurn {
    pub messages: Vec<Value>,
    pub tools: Vec<Value>,
    pub intent_is_tool: bool,
    pub kind: TurnKind,
}

pub struct AiTurnInput<'a> {
    pub mode: &'a str,
    pub doc_type: &'a str,
    pub note_title: &'a str,
    pub system_context: &'a str,
    pub conversation: &'a [Value],
    pub question: &'a str,
    pub mode_policy: &'a str,
    pub turn_instructions: &'a str,
    pub has_open_note: bool,
    pub edit_thread: bool,
    pub oversized: bool,
    pub supports_tools: bool,
    pub verbose_tool_schemas: bool,
    pub section_context: bool,
}
