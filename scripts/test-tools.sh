#!/bin/bash
# Test each tool by sending prompts to llama-server and checking if the model
# calls the correct tool. Uses a tempfile for the JSON body to avoid quoting hell.
#
# Usage: bash scripts/test-tools.sh
# Requires: llama-server running on http://127.0.0.1:39281

API="http://127.0.0.1:39281/v1/chat/completions"

pass=0
fail=0

function test_prompt() {
    local name="$1"
    local prompt="$2"
    local expected_tool="$3"

    echo "─── Test: $name ───"
    echo "  Prompt: $prompt"
    echo "  Expect: $expected_tool"
    echo ""

    # Use jq to build the JSON safely (no shell quoting issues)
    body=$(jq -n \
      --arg prompt "$prompt" \
      --argjson tool_list "$TOOL_LIST" \
      '{
        model: "local-model",
        messages: [
          {role: "system", content: "You are a helpful assistant. The user has a note-taking app with an open note titled \"Test Note\" containing text about Rust programming and ML."},
          {role: "user", content: $prompt}
        ],
        tools: $tool_list,
        tool_choice: "auto",
        stream: false,
        max_tokens: 256
      }')

    resp=$(curl -s "$API" -H "Content-Type: application/json" -d "$body")

    finish=$(echo "$resp" | jq -r '.choices[0].finish_reason // "error"')
    tc_name=$(echo "$resp" | jq -r '.choices[0].message.tool_calls[0].function.name // "none"')
    text=$(echo "$resp" | jq -r '.choices[0].message.content // ""' | head -c 80)
    args=$(echo "$resp" | jq -r '.choices[0].message.tool_calls[0].function.arguments // ""' | head -c 120)

    if [ "$tc_name" = "$expected_tool" ]; then
        echo "  ✅ TOOL CALL: $tc_name"
        echo "  Args: $args"
        pass=$((pass+1))
    elif [ "$tc_name" = "none" ] && [ "$expected_tool" = "none" ]; then
        echo "  ✅ TEXT REPLY: ${text}..."
        pass=$((pass+1))
    elif [ "$tc_name" = "none" ]; then
        echo "  ❌ Expected tool call '$expected_tool' but got text: ${text}..."
        fail=$((fail+1))
    else
        echo "  ❌ Expected '$expected_tool' but got '$tc_name' (text: ${text}...)"
        fail=$((fail+1))
    fi
    echo ""
}

# Build the tool list once using jq (clean JSON, no escaping issues)
TOOL_LIST=$(jq -n '[
  {type:"function", function:{name:"write_note", description:"Edit the note currently OPEN in the editor. mode: replace (default) sets the whole body; appends adds to the end; edit finds and replaces.", parameters:{type:"object", properties:{content:{type:"string"}, mode:{type:"string", enum:["replace","append","edit"]}, find:{type:"string"}}, required:["content"]}}},
  {type:"function", function:{name:"search_notes", description:"Search the ENTIRE workspace for OTHER notes containing keywords.", parameters:{type:"object", properties:{query:{type:"string"}}, required:["query"]}}},
  {type:"function", function:{name:"read_note", description:"Read the full Markdown of ANOTHER note by its id.", parameters:{type:"object", properties:{note_id:{type:"string"}}, required:["note_id"]}}},
  {type:"function", function:{name:"fetch_web_page", description:"Fetch the text content of a public web page.", parameters:{type:"object", properties:{url:{type:"string"}}, required:["url"]}}},
  {type:"function", function:{name:"web_search", description:"Search the web for current info when you have NO URL.", parameters:{type:"object", properties:{query:{type:"string"}, count:{type:"integer"}}, required:["query"]}}},
  {type:"function", function:{name:"format_note", description:"Apply a structural Markdown transform exactly in code (not by LLM). Use for formatting requests.", parameters:{type:"object", properties:{operation:{type:"string", enum:["remove_headings","remove_bold","remove_italics","remove_bullets","remove_numbering","remove_links","remove_images","remove_code","remove_quotes","remove_strikethrough","remove_dividers","remove_blank_lines","strip_plain","convert_headings_to_bold","convert_bold_to_headings","promote_headings","demote_headings","bullets_to_numbered","numbered_to_bullets","lower_case","upper_case","title_case"]}}, required:["operation"]}}},
  {type:"function", function:{name:"find_in_note", description:"Check whether an exact word or phrase appears in the open note.", parameters:{type:"object", properties:{query:{type:"string"}}, required:["query"]}}},
  {type:"function", function:{name:"search_documents", description:"Search the users ingested source documents (PDFs, books) for relevant passages.", parameters:{type:"object", properties:{query:{type:"string"}, count:{type:"integer"}}, required:["query"]}}}
]')

# ====== Test cases ======
# Expected results are for a 1.2B tool-calling model (LFM2-1.2B-Tool-Q4_K_M).
# Larger models should pass more of the "ambiguous" cases.

# Chat-only (no tool expected)
test_prompt "small talk" "hello, how are you?" "none"

# write_note (must reference "my note" or use an edit verb the model recognizes)
test_prompt "write_note (append)" "add a section about rust to my note" "write_note"
test_prompt "write_note (shorten)" "make this note shorter" "write_note"
test_prompt "write_note (poem in note)" "write a short poem about the moon in my note" "write_note"

# search_notes
test_prompt "search_notes" "find notes about machine learning" "search_notes"

# read_note
test_prompt "read_note" "read note 123 and summarize it" "read_note"

# fetch_web_page (requires a URL in the prompt)
test_prompt "fetch_web_page" "fetch https://example.com for me" "fetch_web_page"

# web_search (no URL, just a search query)
test_prompt "web_search" "search the web for AI news" "web_search"

# format_note (specific formatting verbs work better than general ones)
test_prompt "format_note (bold)" "convert all bold text to headings" "format_note"
test_prompt "format_note (headings)" "remove all headings from my note" "format_note"

# find_in_note
test_prompt "find_in_note" "does my note contain the word Rust" "find_in_note"

# search_documents
test_prompt "search_documents" "search my documents for transformer architecture" "search_documents"

# ====== Summary ======
echo "═══════════════════════════════"
echo "  Results: $pass passed, $fail failed"
echo "═══════════════════════════════"
exit $fail
