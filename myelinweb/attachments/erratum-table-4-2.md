# Erratum — Table 4.2, 20B naive cell

## Statement of error

Table 4.2 reported a tool-call accuracy of 88.1% for an 8B model under the instrumented harness and 74.3% for a 20B model under the naive harness. Read as written, the table implied a margin of roughly 14 percentage points in favour of the smaller model.

The two cells are not like-for-like. The 8B model was evaluated at `q4_0`. The 20B model was evaluated at `q4_k_m`. Quantisation was fixed per model rather than held constant across the pair, so the comparison varies two factors at once: harness condition and weight quantisation.

The error originates in our analysis pipeline. The runs were executed as recorded and the table was assembled from those runs without detecting the quantisation difference.

## Correction

The 20B naive cell has been reissued at approximately 80.1%, from a rerun of the 20B model at `q4_0` with the naive harness left unchanged.

| Cell | As published | Matched rerun |
| --- | --- | --- |
| 8B, instrumented, `q4_0` | 88.1% | 88.1% |
| 20B, naive, `q4_k_m` | 74.3% | not comparable |
| 20B, naive, `q4_0` | not reported | 80.1% |

The corrected margin is approximately 8 percentage points rather than approximately 14. All other cells in Table 4.2 are unaffected.

## Effect on the claim

> The direction of the result is unchanged. The magnitude was wrong, and the published gap overstates the effect by roughly a factor of 1.7.

The instrumented 8B model still scores above the naive 20B model under matched quantisation. The corrected margin is also small enough that the crossover point moves, so any statement in the surrounding text about where harness quality stops dominating parameter count must be re-derived rather than carried forward.

The mechanism result is not affected. Chain completion on enum-typed tool surfaces against string-typed surfaces, 81.7% against 38.1%, was measured within a single quantisation setting on a single model and does not depend on the 20B cell.

## Status

This erratum is open. The corrected 20B cell has been agreed with the original reporter and is awaiting inclusion in the next revision of the manuscript.

The published table has not been amended in place. The Crossover claim, the abstract, and the discussion section are under revision in parallel. Until that revision lands, the 74.3% figure should not be quoted as a matched comparison against the 88.1% figure.

---

Erratum raised 12 Oct 2026. Rerun completed 13 Oct 2026. Manuscript revision pending.