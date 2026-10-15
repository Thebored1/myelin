# Figure 2 — Tool-call accuracy against parameter count, two harness conditions

*Figure record. Source grid: `tasksuite-v4`, 240 tasks, three seeds per cell. Attached alongside the paper's Table 4.2.*

## 1 Purpose

Figure 2 is the whole argument of the paper in one panel. It plots tool-call exact match against parameter count for the same five model sizes under two harness conditions, and it is the figure a reader who reads nothing else will be shown.

---

## 2 The two series

The naive series is drawn dashed and carries the lower values throughout: 41.2 at 8B, 73.8 at 14B, 74.3 at 20B, 81.6 at 32B, 83.9 at 70B.

The instrumented series is drawn solid: 88.1 at 8B, 92.7 at 14B, 90.8 at 20B, 95.1 at 32B, 96.0 at 70B.

Both curves flatten in the upper half of the range, for the same reason: past a certain accuracy the remaining failures are no longer format failures, so neither condition has anything left to recover.

One detail is deliberate. The instrumented curve dips between 14B and 20B, 92.7 to 90.8, where the data dip. The figure does not smooth this, and neither should any redraw.

---

## 3 Axes and conventions

The vertical axis is exact match, linear, running 0% to 100% with ticks at 0%, 50% and 100%. It is not truncated.

The horizontal axis is parameter count in billions, but the five measured sizes are plotted at ordinal positions rather than on a linear scale. The spacing between 8, 14 and 20 is visibly identical to the spacing between 32 and 70, and it is not. The axis encodes rank order, not magnitude, and a reader comparing horizontal distances is reading nothing.

Seed variance is averaged into each point and no error bars are drawn. Per-cell dispersion is in the attached suite.

---

## 4 Annotations

The crossover at 14B is marked by a short vertical rule at that position. Its meaning is the exchange of rank described in the paper, not an intersection of the two lines: the instrumented series lies above the naive series at every measured size, but below 14B the separation exceeds anything the naive curve can still be given by scaling, and above it the naive curve's remaining headroom is exhausted.

The gap at 8B is annotated with a dashed vertical span between the two series, labelled +46.9 — the largest separation on the grid, and the number the headline claim rests on.

---

## 5 Caption

> Figure 2. Tool-call exact match against parameter count, two harness conditions. `tasksuite-v4`, 240 tasks, three seeds per cell. The instrumented series lies above the naive series at every measured size. Separation narrows from 46.9 points at 8B to 12.1 at 70B, and the two conditions exchange rank at 14B. Horizontal positions are ordinal, not proportional to parameter count.

*Revision note. These are the original Table 4.2 values, which compared a q4_0 8B against a q4_k_m 20B. A matched rerun moves the 20B naive point to about 80.1 and the 8B gap annotation to roughly 8 points.*