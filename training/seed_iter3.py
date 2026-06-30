"""Append iteration-3 gold anchors to seed.jsonl: academic/math/research + hard
faithful edits + list conversions. NORMAL strings (not raw): real \\n newlines,
and LaTeX backslashes written as \\\\ so they survive into one backslash in JSON."""
import json, pathlib

W = lambda content, **kw: {"tool": "write_note", "args": {"content": content, **kw}}
F = lambda op: {"tool": "format_note", "args": {"operation": op}}
T = lambda text: {"text": text}

R = []  # (note, title, instruction, assistant)

# ---- math / LaTeX (note newline = \n, LaTeX backslash = \\) ----
R += [
 ("# Calc\nThe derivative of x squared is 2x.", "Calc", "write the derivative in proper math notation",
  W("# Calc\nThe derivative of $x^2$ is $2x$.")),
 ("", "Quadratic", "write the quadratic formula",
  W("# Quadratic Formula\n$$x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}$$")),
 ("# Physics\n$$E = mc^2$$", "Physics", "add a line explaining what each symbol means, keep the equation",
  W("# Physics\n$$E = mc^2$$\nHere $E$ is energy, $m$ is mass, and $c$ is the speed of light.")),
 ("# Calculus\nThe integral of x dx is x^2/2 + C.\nThis is a basic result.", "Calculus",
  "convert the first line to LaTeX, leave the second line",
  W("# Calculus\n$\\int x \\, dx = \\frac{x^2}{2} + C$\nThis is a basic result.")),
 ("# Stats\nThe sample mean.\nThe sample variance.", "Stats", "add the formula for the mean right after the first line",
  W("# Stats\nThe sample mean.\n$\\bar{x} = \\frac{1}{n}\\sum_{i=1}^{n} x_i$\nThe sample variance.")),
 ("# Theorem\n$$a^2 + b^2 = c^2$$\nThis is the Pythagorian theorem.", "Theorem",
  "fix the spelling in the prose, keep the equation",
  W("# Theorem\n$$a^2 + b^2 = c^2$$\nThis is the Pythagorean theorem.")),
 ("# Series\nThe sum from i=1 to n of i.", "Series", "write that as a summation formula",
  W("# Series\n$\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}$")),
 ("# Notation\nalpha, beta, and gamma are angles.", "Notation", "write the greek letters as math symbols, keep the sentence",
  W("# Notation\n$\\alpha$, $\\beta$, and $\\gamma$ are angles.")),
]

# ---- research / academic ----
R += [
 ("# Lit Review\nSmith (2020) found a positive effect.\nJones (2021) reported no effect.", "Lit Review",
  "add a sentence about a 2022 meta-analysis that reconciled the two",
  W("# Lit Review\nSmith (2020) found a positive effect.\nJones (2021) reported no effect.\nA 2022 meta-analysis by Lee reconciled the findings, attributing the difference to sample size.")),
 ("# Abstract\nThis paper investigates the effect of sleep on memory. We ran experiments with fifty participants over six weeks and found significant gains in recall. The educational implications are discussed at length, alongside limitations and directions for future work.",
  "Abstract", "shorten the abstract to two sentences",
  W("# Abstract\nWe tested how sleep affects memory in fifty participants over six weeks, finding significant gains in recall. We discuss the educational implications and limitations.")),
 ("# Methods\nWe surveyed 200 participants online.", "Methods", "add that participants were randomly assigned to two groups",
  W("# Methods\nWe surveyed 200 participants online.\nParticipants were randomly assigned to one of two groups.")),
 ("# Intro\nClimate models predict continued warming.", "Intro", "add a citation (IPCC, 2021) to the end of that sentence",
  W("# Intro\nClimate models predict continued warming (IPCC, 2021).")),
 ("# References\n[1] Smith, J. (2020). Memory and sleep.\n[2] Jones, K. (2021). Null effects.", "References",
  "add a third reference: Lee, M. (2022), Meta-analysis of recall",
  W("# References\n[1] Smith, J. (2020). Memory and sleep.\n[2] Jones, K. (2021). Null effects.\n[3] Lee, M. (2022). Meta-analysis of recall.")),
 ("# Paper\n## Introduction\nThe problem is stated here.\n## Conclusion\nWe summarize the findings.", "Paper",
  "add a Methods section between the introduction and the conclusion",
  W("# Paper\n## Introduction\nThe problem is stated here.\n## Methods\nDescribe the approach here.\n## Conclusion\nWe summarize the findings.")),
 ("# Discussion\nOur results support the hypothesis. The effect held across all conditions.", "Discussion",
  "what's a limitation i should mention?",
  T("One worth noting: the sample may not generalize beyond your population, so external validity is limited. Want me to add a limitations sentence to the note?")),
 ("# Hypothesis\nWe expect X to correlate with Y.", "Hypothesis", "rewrite this as a null and alternative hypothesis",
  W("# Hypothesis\nH0: there is no correlation between X and Y.\nH1: there is a correlation between X and Y.")),
]

# ---- hard faithful edits (the measured gap) ----
R += [
 ("# List\n- alpha\n- beta\n- gamma\n- delta", "List", "remove the third item",
  W("# List\n- alpha\n- beta\n- delta")),
 ("# Steps\nFirst, gather the data.\nSecond, clean it.\nThird, analyze it.", "Steps",
  "insert a validation step between cleaning and analysis",
  W("# Steps\nFirst, gather the data.\nSecond, clean it.\nThird, validate it.\nFourth, analyze it.")),
 ("# Para\nThe study was run in 2019. It had many participants. The results were clear.", "Para",
  "change 2019 to 2020, leave the rest",
  W("# Para\nThe study was run in 2020. It had many participants. The results were clear.")),
 ("# Lines\nline one\nline two\nline three\nline four", "Lines", "swap line two and line three",
  W("# Lines\nline one\nline three\nline two\nline four")),
 ("# Outline\nintro paragraph here\nbody paragraph here\nclosing paragraph here", "Outline",
  "delete only the body paragraph",
  W("# Outline\nintro paragraph here\nclosing paragraph here")),
 ("# Email\nDear team,\nThe meeting is at 3pm.\nBest,\nAlex", "Email", "change the time to 4pm, keep everything else",
  W("# Email\nDear team,\nThe meeting is at 4pm.\nBest,\nAlex")),
 ("# Recipe\n- 2 cups flour\n- 1 cup sugar\n- 3 eggs", "Recipe", "double the eggs",
  W("# Recipe\n- 2 cups flour\n- 1 cup sugar\n- 6 eggs")),
 ("# Def\nA prime number is divisible only by itself.", "Def", "fix the definition to also mention 1",
  W("# Def\nA prime number is divisible only by 1 and itself.")),
]

# ---- list conversions (missing routing) ----
R += [
 ("- one\n- two\n- three", "L", "number this list", F("bullets_to_numbered")),
 ("1. a\n2. b\n3. c", "L", "make this a bulleted list instead", F("numbered_to_bullets")),
 ("- [ ] task a\n- [x] task b", "Tasks", "remove the checkboxes", F("tasks_to_bullets")),
]

# ---- tables / code (academic markdown) ----
R += [
 ("# Data\n| x | y |\n| - | - |\n| 1 | 2 |", "Data", "add a row with 3 and 4",
  W("# Data\n| x | y |\n| - | - |\n| 1 | 2 |\n| 3 | 4 |")),
 ("# Code\n```python\nprint('hi')\n```", "Code", "add a comment above the print line",
  W("# Code\n```python\n# greet the user\nprint('hi')\n```")),
]

path = pathlib.Path(__file__).parent / "seed.jsonl"
with path.open("a", encoding="utf-8") as f:
    for note, title, instr, a in R:
        f.write(json.dumps({"note": note, "title": title, "instruction": instr, "assistant": a}, ensure_ascii=False) + "\n")
print(f"appended {len(R)} anchors; seed now has {sum(1 for _ in path.open(encoding='utf-8'))} lines")
