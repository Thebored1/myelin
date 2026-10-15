# On the limits of harness effects in tool-use evaluation

*E. Svensson, M. Lindholm, A. Farouk. Preprint, 2026. Not peer reviewed.*

---

## Abstract

Published claims that harness quality dominates parameter count for tool-call accuracy rest on comparisons in which the scaffold intervention and the task suite vary together. We argue that such comparisons cannot attribute the effect, because a stronger scaffold typically lowers the measured difficulty of the same task, and because the term harness bundles at least four independently varying components. We run a variance decomposition across four task suites and two model families and show that the component credited with 82% of variance in published results is credited with 31% on a suite chosen independently. We do not dispute any individual crossover. We dispute the attribution.

---

## 1 Introduction

There is now a reasonably consistent empirical pattern in the small-model literature: a stronger scaffold closes a large gap on smaller models, and the gap narrows with parameter count. We think the pattern is real and we are not writing to refute it.

Our objection is narrower. The claim that harness quality *is* the dominant lever is an attribution claim, and it is made on the basis of evidence that cannot support it. A comparison of two harness conditions on one task suite establishes that the two conditions differ on that suite. It does not establish that the scaffold, rather than the suite, is responsible for the difference.

> Scaffolding interventions and task difficulty are not independent in most published harnesses. A stronger scaffold typically lowers the measured difficulty of the same task.

---

## 2 What published comparisons actually vary

Harness is not one variable. A typical comparison holds the model fixed and varies everything else at once: tool-schema typing, retry policy, context-window management, and error-message verbosity are changed together. The resulting number is a property of the bundle. Reported results therefore cannot separate the components, and cannot distinguish a large component from four moderate ones.

We note that this cuts in favour of the strong claim rather than against it. A bundle that produces a large effect suggests at least one large component. Our point is that the published evidence does not identify which, and the natural inference from a bundle-level effect to a scaffold-level claim is not licensed.

---

## 3 Why task suite is a confound

The more serious difficulty is on the evaluation side. A scaffold that inserts a task-state summary, decomposes the task, or offers a repair turn makes the task easier, and the evaluation suite records that as an improvement. The model has not changed. The measurement has.

This is not a hypothetical. It is visible in the spread of published effect sizes, which is far wider than the difference between the bundles being compared can plausibly explain. Where two groups use different suites and different scaffolds and report different effects, at least one of them is measuring suite difficulty and calling it capability.

---

## 4 Variance decomposition

We fitted a mixed-effects decomposition across four task suites and two model families, with scaffold bundle and task suite as grouping terms.

| Term | Our suite | Published suites |
| --- | --- | --- |
| Scaffold bundle | 31% | 82% |
| Task suite | 61% | 24% |

Table 2. Variance explained, per cent, by grouping term.

The inversion is the whole argument. On the suite we constructed independently of any scaffold we were evaluating, the scaffold bundle explains 31% of variance and the task suite explains 61%. On suites taken from published results, the ordering reverses and the magnitudes change. Task suite is not a nuisance term that can be averaged away; on a suite chosen without reference to the scaffold under test, it is the larger effect.

We are explicit about the direction of this finding. It says that the suite matters more than the scaffold in our data. It does not say that any particular published suite is badly built, and we have not audited any.

---

## 5 What would settle it

Three things would materially improve the evidence base, and none is expensive.

Running a fixed scaffold bundle across several independently constructed suites would test whether the effect size is a property of the bundle or of the suite it was tuned on. Holding the suite fixed while swapping one scaffold component at a time would make the bundle decomposable. Reporting per-task rather than per-cell breakdowns would show whether the effect concentrates in the long tasks, where suite design and scaffolding intervention are most likely to be confounded with one another.

The first of these is the one we would prioritise. It converts an unattributable effect into an attributable one.

---

## 6 Conclusion

We do not dispute the crossovers. Several groups report the same ordering on independent rigs, and we think that convergence is meaningful evidence about the ordering.

We dispute the attribution. On a suite chosen independently, the scaffold bundle is the smaller term and the task suite is the larger. Until the attribution is established on an orthogonal suite, the honest statement is that harness quality correlates strongly with tool-call accuracy below roughly 14B parameters, and that the cause of the correlation remains open.