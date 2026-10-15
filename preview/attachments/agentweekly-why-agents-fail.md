# Why agents fail at tool use

28 Feb 2026. Agent Weekly survey.

## Failure taxonomy

The survey codes each failed task run into one of four failure modes, applied to 1,400 logged agent sessions across eleven deployments.

| Rank | Failure mode | Share of failures |
| --- | --- | --- |
| 1 | Attribute errors | 46% |
| 2 | Retrieval misses | 28% |
| 3 | Genuine reasoning errors | 18% |
| 4 | Other, uncoded | 8% |

Attribute errors are calls in which the correct tool was selected and the arguments did not conform — a string where the surface wanted an enum member, a missing required field, a unit the surface never accepted. Retrieval misses are calls that named a resource the agent could not see, often because the tool list was paginated. Genuine reasoning errors are the residue: wrong tool for the situation, wrong order of calls, wrong reading of a result.

The headline finding is the shape of the distribution, not the presence of any one category:

> Nearly half of all agent failures are interface conformance failures in which the model demonstrated it knew what to do.

## How the survey was run

Sessions were supplied by the participating deployments and coded blind by two annotators, with a third adjudicating disagreements. Coding was applied to the trajectory, not to the final answer, so a run that recovered after several malformed calls is counted once, at its first failure. Category counts are per run, not per call.

## What is absent

The survey has no parameter control. It does not report which models the eleven deployments ran, and it does not claim that failures are comparable across them.

It also has no scaffold manipulation. Every deployment was observed as it was already configured. Nothing in the design varies the tool surface while holding the task fixed, so the survey can rank failure modes but cannot attribute them to a cause.

That the largest category is interface conformance is a description of where runs stop. It is not a measurement of what would happen if the surface were changed.

## Provenance note

The taxonomy ordering matches what the paper finds independently. The paper reaches that ordering by construction — varying argument typing at fixed model and fixed task — which the survey cannot do.

The survey is background reading. It establishes that attribute errors are the commonest failure mode in deployed agents, and it is useful for readers who want to know whether the paper's mechanism is worth caring about operationally. It is not cited as evidence for the mechanism, and no claim in the paper depends on it.