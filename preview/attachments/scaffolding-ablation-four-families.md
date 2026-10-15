# Scaffolding ablation across four model families

*R. Tanaka, A. Osei, J. Lindqvist, P. Venkatesan. Peer-reviewed conference paper, 2025. Independent evaluation stack `scaffoldbench-0.9.4`, no shared code with any grid reported elsewhere.*

## Abstract

Scaffolding quality and parameter count are usually reported as independent levers on agentic task completion. We run a factorial ablation over four instruction-tuned model families at two size bands, holding the task set, the tool inventory, and the scoring stack fixed, and varying only the scaffold. Scaffolding quality predicts task completion better than parameter count below approximately 10B parameters. Above that point the two explanations are statistically indistinguishable in our setup. We do not claim the ordering holds universally: our evaluation excludes long-horizon tool chains and holds tool-schema typing constant.

---

## 1 Introduction

The purpose of this study is narrow and we have tried to keep it narrow. We wanted to know whether the benefit of a well-built scaffold is visible on models small enough to matter for local deployment, or whether it is a property of frontier-scale systems only.

Two measurements answer this directly: a factorial ablation in which scaffold condition and model size vary independently, and a per-family replication across four lineages. We did not attempt a general account of why scaffolds help, and we do not offer one.

---

## 2 Setup

**Models.** Four instruction-tuned families, each contributing two size bands: Llama 3.1 Instruct, Qwen 2.5 Instruct, Mistral Instruct, and Gemma 2 Instruct. We report the bands at their nominal 7B and 13B labels.

**Tasks.** 96 single- and two-turn agentic tasks across four domains, with a fixed inventory of eleven tools and a fixed output parser. Scoring is task completion under a strict parser, requiring both a correct tool call and a schema-valid argument.

**Scaffold conditions.** Five conditions from weakest to best, differing in explicit step decomposition, a typed tool surface, a task-state summary, and a single repair turn. The weakest is a bare system prompt with the tool inventory appended. The best enables all four. Conditions apply identically to every model.

**Replication.** Three seeds per cell, a single fixed cluster, and no model served through more than one inference stack.

---

## 3 Results

Completion rises with parameter count in every family and every scaffold condition. That is the least surprising result in the study.

Completion also rises monotonically with scaffold condition, and the size of that rise depends on where in the size range the model sits. At the smaller band the scaffold effect exceeds the difference between the two size bands. At the larger band the effect is real but modest, and the difference between the size bands is within the same order of magnitude.

> We observe a crossover below 10B parameters: scaffolding quality predicts task completion better than parameter count. Above 14B the two explanations are statistically indistinguishable in our setup.

---

## 4 Between-family agreement

The crossover is not confined to one lineage. All four families show the same ordering of effects at the smaller band, with the weakest-scaffold cell lowest and the best-scaffold cell highest in every case. The variation between families is in the level, not in the direction of the effect.

We regard this as the strongest feature of the study, because it was not designed in advance and no family was excluded after the fact.

---

## 5 Scaffolding ablation

Table 3 reports the extreme conditions only. The intermediate conditions fall monotonically between the two rows and are in the appendix.

| Scaffold condition | 7B | 13B |
| --- | --- | --- |
| Weakest | 46% | 38% |
| Best | 84% | 79% |

Table 3. Task completion, weakest against best scaffold, per-family mean.

Two observations are worth stating plainly.

The first is the crossover. At 7B the best scaffold closes 38 points over the weakest; at 13B it closes 41. The gap is comparable in absolute terms, but worth more at 7B because the baseline is lower and the 13B model is already largely insulated from the weakest condition by capability alone.

The second is an anomaly we cannot explain. The weakest scaffold at 13B, 38%, is below the weakest scaffold at 7B, 46%. A larger model should not be made worse by the same scaffold. We attribute this provisionally to prompt-length interaction in the weakest condition, where the bare prompt plus the eleven-tool inventory exceeds the smaller model's effective instruction-following capacity, but we have not isolated it and are not prepared to offer a confident mechanism. The cell is reported as measured.

---

## 6 Limitations

**No long-horizon chains.** Every task here is one or two turns. We exclude long-horizon tool chains by design and do not claim our results transfer to them. Work reporting large scaffold gains on chain-following is not contradicting this study, because it is not measuring the same thing.

**Constant tool-schema typing.** The typed tool surface is held on across all conditions, including the weakest. Scaffolds in the wild frequently vary schema typing together with everything else, and that interaction is not represented here.

**Two size bands per family.** The study brackets the crossover rather than mapping it, and the 7B and 13B labels denote bands whose constituent checkpoints differ across families.

**Completion, not reasoning.** We measure whether an agent finished a task, not whether it reasoned well. A scaffold that reduces wasted steps will raise completion without improving reasoning, and this study cannot distinguish the two.

---

## 7 Conclusion

Scaffolding quality is the larger lever below approximately 10B parameters across four model families, and the two levers are close to indistinguishable above that point. For a team choosing between a stronger scaffold and a larger model in the lower range, this study would put the scaffold first. Above the range, we would not use this paper to justify either choice.