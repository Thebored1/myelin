# Current Required Changes

This is the sole working plan for requested repository changes. It contains only outstanding work. Completed, rejected, or obsolete items must be removed rather than retained as history.

## Ground Rules

- Do not edit repository files.
- The only file the planning agent may create or modify is `current-required-change.md`.
- Analyze, review, and propose changes; do not implement them.
- Keep this plan synchronized with the latest request.
- Add newly required work and remove work that is completed, rejected, superseded, or no longer needed.

## LaTeX: Required Change Plan

### Task LATEX-001: Deliver near-real-time LaTeX preview

- Replace the fixed two-second auto-compile delay with a short adaptive debounce, initially targeting 250–400 ms after typing pauses.
- Compile an in-memory editor snapshot rather than waiting for the normal note-save round trip.
- Assign every source snapshot a monotonically increasing revision and apply a returned PDF only when it matches the newest relevant revision and open note.
- Keep at most one Tectonic job active. Coalesce all changes received during that job into one follow-up compile of the newest snapshot.
- Investigate safe cancellation of an obsolete Tectonic session; use revision-based result rejection and coalescing if the embedded engine cannot be interrupted safely.
- Keep the last valid PDF visible while a new compile is pending or while incomplete source temporarily fails.
- Surface unobtrusive states for pending, compiling, current, and error without making the editor feel blocked.
- Avoid setting the application-wide busy state for background preview compilation; manual compilation may still expose an explicit busy state.
- Start background compilation only after the Tectonic support bundle is warmed. First-run bundle acquisition must remain an explicit progress state.
- Instrument edit-to-preview latency separately for debounce, queue wait, TeX compilation, IPC transfer, and PDF rendering.
- Set initial warmed-cache performance goals: median preview update under 500 ms for small documents and under 1 second for typical notes, measured on supported reference hardware.
- Verify rapid typing, edits during compilation, disabling auto-compile, switching notes, closing the preview, and compile errors never display a stale PDF.

### Task LATEX-002: Eliminate package option clashes

- Reproduce the reported failure with a note that requests options for `xcolor`, such as `\usepackage[dvipsnames]{xcolor}`, and capture the exact transformed source sent to Tectonic.
- Treat a source containing preamble commands but no `\documentclass` as a distinct case; do not blindly place its `\usepackage` commands after Myelin's generated `\begin{document}`.
- Split a partial preamble from document body content when safely possible, merge it before `\begin{document}`, and preserve requested package options.
- Replace substring checks in `ensure_packages` with parsing that recognizes active `\usepackage`, `\RequirePackage`, and `\PassOptionsToPackage` declarations.
- Ignore package names appearing only in comments or ordinary document text.
- Support comma-separated package declarations and declarations with options.
- Never load the same package twice with incompatible options. If Myelin supplies a package, combine compatible options before its first load or defer to the user's declaration.
- Prefer leaving a complete document's package choices and ordering untouched. If automatic injection into full documents remains required, restrict it to an explicitly defined safe set and do not inject `xcolor` or `hyperref` without accounting for class and package ordering.
- Return a concise actionable diagnostic when an option clash originates entirely inside the user's own complete preamble; distinguish that from a clash introduced by Myelin's transformation.
- Verify documents using `xcolor` with `dvipsnames`, `table`, multiple options, no options, indirect class loading, and commented-out declarations.
- Acceptance criterion: the reported `Option clash for package xcolor` case compiles without requiring the user to remove valid package options.

### Task LATEX-003: Correct diagnostic line mapping

- Track the exact insertion position and number of generated lines when modifying a full document.
- Subtract injected lines only from diagnostics located after the insertion point.
- Preserve correct mappings for errors before or on `\documentclass`.
- Keep the existing bare-document preamble mapping, but verify its first and last body-line boundaries.
- Add coverage for `l.NN` errors, `on input line NN` errors, duplicates, and diagnostics without line numbers.

### Task LATEX-004: Define and enforce multi-file LaTeX behavior

- Decide whether `.tex` notes should support `\input`, `\include`, local `.sty` files, bibliographies, and relative images.
- If supported, configure a constrained input root based on the note/workspace location and prevent access outside the allowed workspace.
- If unsupported, detect common project-file references and show a clear limitation instead of a generic Tectonic failure.
- Document the selected behavior in the editor or project documentation.

### Task LATEX-005: Clarify compile snapshot semantics

- Decide whether compilation targets the persisted note or the exact editor snapshot at the moment Compile is requested.
- Prefer passing or otherwise pinning an immutable source revision so later saves cannot ambiguously change the requested job.
- Ensure the PDF result is applied only if it corresponds to the relevant note and source revision.
- Avoid showing a completed preview from a note that was closed or replaced during compilation.

### Task LATEX-006: Expand focused LaTeX tests

- Unit-test bare-source wrapping and full-document preservation.
- Test frontmatter removal and empty-document handling.
- Test package detection with comments, options, comma-separated packages, and misleading substrings.
- Add a regression test for partial preambles and `\usepackage[dvipsnames]{xcolor}` in a document without `\documentclass`.
- Test line remapping before and after injected content.
- Test structured error serialization and frontend parsing.
- Test auto-compile changes during an active compile.
- Add integration coverage for first-run cache events, warmed-cache behavior, and failed bundle downloads.
- Add tests for the chosen multi-file policy.

### Task LATEX-007: Update LaTeX documentation

- Explain the real-time preview behavior and what happens while source is temporarily invalid.
- Document first-run support-bundle download and offline behavior after warm-up.
- Document the selected package-injection and multi-file policies.
- State relevant limitations and expected preview latency without promising literally instantaneous compilation.

## Recommended Order

1. `LATEX-002`: Fix the active package-option-clash failure and establish the package-injection policy.
2. `LATEX-004`: Decide and enforce the multi-file policy.
3. `LATEX-005`: Establish source-snapshot and result-validity semantics.
4. `LATEX-001`: Implement near-real-time compilation.
5. `LATEX-003`: Correct diagnostic mapping.
6. `LATEX-006`: Add focused unit and integration tests.
7. `LATEX-007`: Update user-facing documentation for the final behavior.
