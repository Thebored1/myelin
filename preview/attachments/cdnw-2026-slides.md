# CDNW 2026 — Your model is not your agent

Talk given at CDNW 2026, 11 Jun 2026. Selected slides from a 22-slide deck; slide numbers are those of the original.

## Slide 1

**Your model is not your agent**

## Slide 2

**We ship a model and we ship a harness, and we only benchmark the first one**

## Slide 3

**Same weights, same weights, same weights**

## Slide 7

**41 to 88 on the same 8B weights**

The harness is the only thing that changed. No retraining, no distillation, no new checkpoints. The 8B model was already capable of producing a well-formed call; it was simply never told what a malformed one looked like.

## Slide 12

**The gap closes by 14B. That's the honest limit.**

Below roughly 14B parameters, harness work beats parameter work. At and above 14B, the ordering flattens and I would not claim it. I would rather give you a boundary that holds than a graph that points up and to the right.

## Slide 15

**Where the points go**

Every point the instrumented harness recovers is a point the naive harness burned on a malformed argument that the runtime rejected before the model saw it.

## Slide 17

**What this is not**

This is not prompt engineering in the sense of finding a better-sounding instruction. It is error routing. The harness decides what the model is allowed to see after it fails, and how many attempts it gets.

## Slide 19

**It's also 165% more tokens. Pick your poison.**

> The naive harness is cheaper and dumber. The instrumented harness is more expensive and correct. There is no third option in this talk.

## Slide 20

**If you take one thing**

Instrument the failure path before you buy the next checkpoint.

## Speaker notes

Two caveats, both of which I want on the record.

First, this talk was given on 11 Jun 2026, roughly two months before the paper that reports the typing result. The mechanism I describe here is not the mechanism in that paper. I am attributing the recovery to error routing and retry visibility — the harness shows the model its own rejected call and grants a bounded number of repair attempts. The paper attributes the effect to argument typing on the tool surface, and it isolates that by varying enum against string parameters. Those are different claims with different experiments behind them. The numbers on these slides are mine and are not the paper's numbers; the 41 to 88 and the 165% figure appear nowhere in the manuscript.

Second, the crossover I report at 14B here was the first run of that analysis, on an unmatched comparison that I did not have the instrumentation to check at the time. The paper's version of that comparison is matched, and the margin is smaller than the one I gave on this stage. If you take the boundary from the paper rather than from this deck, you will get the defensible number.

Everything from slide 8 onward — the tooling demo, the vendor comparison, the pricing slide — is cut from this capture.