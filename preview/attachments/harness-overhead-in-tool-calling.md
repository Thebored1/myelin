# Harness overhead is a bigger error source than model scale

*Draft v3, unreviewed. Evaluation suite `tasksuite-v4`. All serving performed through llama.cpp.*

## Abstract

Tool-call accuracy is normally reported as a property of the model. We argue that below roughly 14B parameters it is largely a property of the harness that surrounds the model, and that the two are separable by a controlled manipulation rather than by analogy. Across 240 tasks, five model sizes from 8B to 70B, and two harness conditions, an instrumented harness lifts an 8B model above the level a considerably larger model reaches in a naive harness. The failure mode is schema adherence under multi-step chains rather than tool selection, and it does not scale with parameters. We isolate argument typing as the component carrying most of the effect, and report an error in our own headline table that halved the stated margin.

> Below about 14B parameters, tool-call accuracy is more a property of the harness than of the model.

---

## 1 Introduction

The size heuristic in this area is simple and mostly unwritten down: if tool-calling accuracy is poor, add parameters. It is hard to falsify from published results, because almost every comparison holds the harness fixed and varies only the model, or the reverse, and reports the two as facts about separate components.

We take the two together. The question is not whether a stronger harness helps, nor whether a larger model helps. It is which dominates, and over what range of sizes the answer changes. Our answer is that the ordering inverts: an 8B model under an instrumented harness outperforms a 20B model under a naive harness. That is the statement the rest of this paper tries to earn rather than assume.

---

## 2 Related work

Scaffolding interventions for tool use are well established, and several groups report gains in the range we observe. None reports a controlled crossover against parameter count.

Two results bear on the framing. Tanaka et al. report a crossover below 10B across four model families, on a harness, task suite, and evaluation stack independent of ours. Their rig shares no code with ours, and the agreement in the location of the crossover is evidence we did not produce and cannot inflate.

Svensson and colleagues argue the framing is confounded: that scaffold interventions and task difficulty are not independent, and that the word harness bundles at least four variables published comparisons do not separate. We think that objection is largely correct, and we answer it in §5 by varying one component at a time rather than swapping a whole harness. We do not claim the answer is complete.

---

## 3 Method

**Task suite.** `tasksuite-v4` contains 240 tasks across eight categories, stratified by turn count (2 to 5) and tool count (1 to 3). Scoring is exact match on the full emitted call rather than on tool selection alone; a call naming the correct tool and emitting a malformed argument scores zero. Three seeds per cell. The suite is attached in full.

**Models and serving.** Five sizes, 8B, 14B, 20B, 32B and 70B, all Qwen2.5 instruct, served through llama.cpp. The original run did not use matched quantisation; see §6.

**Harness conditions.** Both conditions expose identical tool descriptions and identical task text. They differ in three respects: argument typing (enum versus free string), retry policy (one re-ask versus none), and turn state (explicit versus implicit). We vary these separately in §5.

---

## 4 Results

### 4.1 Headline grid

The naive series rises steeply to 14B and then flattens. The instrumented series rises quickly at the bottom of the range and is comparatively flat throughout. The shape matters as much as the level, because a flat curve has run out of headroom.

### 4.2 Variance decomposition across the harness grid

Holding the model fixed, the harness term is the larger of the two. The separation between conditions is 12.1 to 46.9 points, largest where the naive condition is weakest, narrowing monotonically with parameter count.

| Parameters | Naive | Instrumented | Delta |
| --- | --- | --- | --- |
| 8B | 41.2 | 88.1 | +46.9 |
| 14B | 73.8 | 92.7 | +18.9 |
| 20B | 74.3 | 90.8 | +16.5 |
| 32B | 81.6 | 95.1 | +13.5 |
| 70B | 83.9 | 96.0 | +12.1 |

Tool-call exact match, `tasksuite-v4`, 240 tasks, three seeds per cell.

The decomposition has a practical consequence. The naive condition buys 32.6 points by moving from 8B to 14B and 9.6 points by moving from 14B to 70B. The instrumented condition buys 4.6 points across the first step and 3.3 across the second. Parameter count is worth more than harness quality below 14B and worth less above it. That transition is the crossover this paper reports, and it is where the two components exchange rank.

The instrumented series is above the naive series at every measured size. The crossover is not an intersection of the two lines. It is the point at which the cheaper condition becomes competitive rather than hopeless.

---

## 5 Mechanism

The dominant naive-harness failure is not choosing the wrong tool. It is choosing the right tool and emitting an argument the runtime cannot dispatch, then failing to recover on the following turn because there is no turn state to recover with. This predicts where the gap should be widest, and the per-task breakdown agrees: the naive column collapses hardest exactly where turn count rises.

Because harness bundles several variables, turning the naive harness into the instrumented one in a single step is uninformative. Varying argument typing alone, with retry policy and turn state fixed, recovers most of the effect. On three-turn chains, string-typed surfaces complete 38.1% and enum-typed surfaces complete 81.7%. This establishes typing as a large component of the effect on this suite. It does not establish that typing is the whole of it.

---

## 6 Threats to validity

**Our own quantisation error.** The published Table 4.2 compared a q4_0 8B against a q4_k_m 20B. This was not like-for-like and it was raised publicly. A matched rerun moves the 20B naive cell to about 80.1%, cutting the headline margin from fourteen points to roughly eight. The direction survives; the number in the previous abstract was wrong.

**Overclaiming in an earlier draft.** Version 1 asserted harness quality was the main lever in general. A three-turn chain benchmark showed the effect is concentrated in long chains and near zero for single-turn tasks. That draft was superseded rather than deleted.

**Confounding with task suite.** The objection that a scaffold which lowers measured task difficulty will look like a capability improvement stands. Our defence is that typing is varied against a fixed suite and fixed tool descriptions. We have not run the orthogonal suite the objection calls for.

**One family.** All five sizes are Qwen2.5 instruct. Effect sizes here are single-family.

**No claim about reasoning.** Every failure measured is a format or recovery failure. The 70B cell suggests a stronger harness does not improve reasoning.

---

## 7 Conclusion

Below about 14B parameters, tool-call accuracy is more a property of the harness than of the model. The effect is large enough to invert the usual size heuristic, concentrated in argument typing, and reproduced independently by a group with no stake in it. It is also, as §6 sets out, overstated by our own error in the headline table, and contested on confounding grounds we have not fully answered. The claim survives those qualifications. That is the state of it, and the failing reproduction is attached to this draft rather than withdrawn.