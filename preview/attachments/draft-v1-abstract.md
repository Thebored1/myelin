# Draft v1 — abstract and claims (superseded 9 Aug)

Retained for the record. This is the first circulated abstract and claim list. It has been superseded twice and is not the text of the current manuscript. It is reproduced here because two of its claims are quoted in secondary coverage of the paper and should remain traceable to the draft that made them.

## Abstract (as circulated, Aug 2026)

> Harness quality is the main lever in tool-call accuracy. Across model families spanning 7B to 70B parameters, models equipped with an instrumented harness outperform the same models under naive prompting by wide and consistent margins, and the effect is largest for smaller models. We find consistent gains from schema validation. Argument type checking, result normalisation, and structured repair prompts each contribute, and the benefit accumulates across multi-turn chains. Harness quality therefore constitutes a first-order design variable for deployed tool-calling systems, and practitioners should treat it as more consequential than scaling.

## Claims as circulated

The following three claims appeared in the v1 contribution list.

1. Harness quality is the main lever in tool-call accuracy.
2. We find consistent gains from schema validation.
3. The effect holds across the full 7B to 70B range examined.

Each was defensible against the v1 experiment set. Each fails against the corrected and extended runs, and two of them overstate the direction of the evidence rather than merely its size.

## What changed

| Claim | v1 | v3 |
| --- | --- | --- |
| Scope of the effect | Harness quality is the main lever in tool-call accuracy. | Harness quality dominates parameter count below approximately 14B parameters; above that the ordering is not established either way. |
| Source of the gain | We find consistent gains from schema validation. | The gain is concentrated in chains of three or more turns; single-turn chains show no reliable difference between typed and untyped surfaces. |

The correction on the second claim is the substantive one. v1 attributed the effect to schema validation generally. The instrumented runs isolate argument typing specifically: enum-typed surfaces complete three-turn chains at 81.7% against 38.1% for string-typed surfaces. Validation is not the lever. Typing is, and it is a lever on chain depth rather than on accuracy in general.

The correction on the first claim follows from the erratum to Table 4.2 and from the crossover analysis. The matched rerun moves the 20B naive cell from 74.3% to approximately 80.1%, reducing the reported margin from roughly 14 points to roughly 8. A margin of 8 points does not support a claim about the main lever across a 7B to 70B sweep.

---

v1 circulated 2 Aug 2026. Superseded 9 Aug 2026. Retained unaltered.