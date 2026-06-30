"""Append iteration-3 held-out eval cases (academic/math/research/hard-edit/
conversion) to eval.jsonl. NORMAL strings: real \\n newlines, LaTeX as \\\\."""
import json, pathlib

C = []  # (note, title, instruction, check)

# math: must PRESERVE equations while editing prose
C += [
 ("# Eq\n$$E = mc^2$$\nEnergy equation.", "Eq", "change 'Energy equation' to 'The energy-mass relation'",
  {"tool": "write_note", "content_has": ["$$E = mc^2$$", "The energy-mass relation"], "content_lacks": ["Energy equation"]}),
 ("# Calc\n$\\int x \\, dx = \\frac{x^2}{2} + C$\nThis is basic.", "Calc",
  "change 'This is basic' to 'A standard integral'",
  {"tool": "write_note", "content_has": ["\\frac{x^2}{2}", "A standard integral"], "content_lacks": ["This is basic"]}),
 ("# Stats\nThe mean is the average value.", "Stats", "add the formula for the mean on a new line, keep the sentence",
  {"tool": "write_note", "content_has": ["The mean is the average value.", "$"]}),
]

# research / academic structure
C += [
 ("# Refs\n[1] Alpha (2019).\n[2] Beta (2020).", "Refs", "add a third reference: Gamma (2021)",
  {"tool": "write_note", "content_has": ["[1] Alpha (2019).", "[2] Beta (2020).", "Gamma"]}),
 ("# Intro\nGlobal temperatures are rising.", "Intro", "add a citation (NASA, 2020) to the sentence",
  {"tool": "write_note", "content_has": ["Global temperatures are rising", "NASA"]}),
 ("# Paper\n## Introduction\nbackground\n## Conclusion\nsummary", "Paper",
  "add a Results section before the conclusion",
  {"tool": "write_note", "content_has": ["## Introduction", "## Results", "## Conclusion"]}),
 ("# Abstract\nWe study how caffeine affects focus across a large sample with many measures and report several effects in detail across conditions and timepoints and subgroups.",
  "Abstract", "shorten this to one sentence",
  {"tool": "write_note", "content_has": ["caffeine"]}),
]

# hard surgical edits
C += [
 ("# L\n- a\n- b\n- c\n- d", "L", "remove the second item",
  {"tool": "write_note", "content_has": ["- a", "- c", "- d"], "content_lacks": ["- b"]}),
 ("# T\nkeep one\nchange me\nkeep two", "T", "change 'change me' to 'changed' and keep the rest",
  {"tool": "write_note", "content_has": ["keep one", "changed", "keep two"], "content_lacks": ["change me"]}),
 ("# Steps\nstep one\nstep two", "Steps", "insert 'step one-and-a-half' between them",
  {"tool": "write_note", "content_has": ["step one", "step one-and-a-half", "step two"]}),
 ("# Data\n| a | b |\n| - | - |\n| 1 | 2 |", "Data", "add a row with 3 and 4",
  {"tool": "write_note", "content_has": ["| 1 | 2 |", "3"]}),
]

# conversions
C += [
 ("- x\n- y\n- z", "L", "number this list", {"tool": "format_note", "args_contains": {"operation": "bullets_to_numbered"}}),
 ("1. x\n2. y", "L", "convert to a bulleted list", {"tool": "format_note", "args_contains": {"operation": "numbered_to_bullets"}}),
]

path = pathlib.Path(__file__).parent / "eval.jsonl"
with path.open("a", encoding="utf-8") as f:
    for note, title, instr, chk in C:
        f.write(json.dumps({"note": note, "title": title, "instruction": instr, "check": chk}, ensure_ascii=False) + "\n")
print(f"appended {len(C)} eval cases; eval now has {sum(1 for _ in path.open(encoding='utf-8'))} lines")
