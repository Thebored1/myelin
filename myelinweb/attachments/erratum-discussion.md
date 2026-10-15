# Discussion — quantisation mismatch in Table 4.2

Public thread attached to the manuscript record. Posts are reproduced in order with timestamps and vote counts as displayed.

## kv_quant_nerd

12 Oct 2026, 3 days ago. 214 upvotes.

Table 4.2 is not comparing what the caption says it is comparing. The 8B is `q4_0`. The 20B is `q4_k_m`. That is two changed variables in a two-cell comparison, and the quality difference between those two quantisers on tool-call argument formatting is not small. I would expect the quantiser to account for most of the 14-point gap before any harness effect is involved at all.

The harness contrast is real, but the number in the table is inflated by the quantiser, and the abstract inherits it.

A rerun of the 20B naive cell at `q4_0` would settle it in an afternoon. I would run three seeds if you have them.

## author

13 Oct 2026, 2 days ago.

Agreed, and it is our error. The table was assembled from a run directory without a quantisation check, and the mismatch was not caught in review. Thank you for the rerun.

The 20B naive cell will be reissued at `q4_0` with the naive harness unchanged. We agree that the cell as published should not stand. The crossover analysis and the abstract will be revised rather than merely footnoted, because the corrected margin moves where the crossover sits.

The typing result is unaffected — that comparison is within one model at one quantisation.

## skeptical_sam

14 Oct 2026, 1 day ago. 88 upvotes.

Worth reading alongside this: arXiv 2602.04119, *Limits of harness effects in tool-calling evaluation*. They sweep harness structure at fixed parameters across several families and find the effect shrinks faster than most published harnesses assume, and in some configurations reverses. Not the same question as yours, and their task mix is different, but the ceiling they report is lower than the ceiling in your Table 4.2.

If your corrected margin is around 8 points, that sits much closer to their range than the 14 does. Probably worth an explicit comparison section, and worth knowing before you reissue the cell.

---

Thread remains open pending the revised manuscript.