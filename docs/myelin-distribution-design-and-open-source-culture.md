# Myelin — Distribution, Publishing Identity & Open-Source Culture Notes

*Continuation of `myelin-publishing-and-business-model.md`, compiled from the planning discussion that followed it.*

---

## 1. Business Viability & Distribution Reassessment

The discussion returned to whether Myelin could become a sustainable business if executed properly.

The conclusion became more confident after the publishing model was examined in detail:

- The underlying product/business model is commercially plausible.
- The largest uncertainty is no longer whether Myelin has *any* credible path to distribution.
- The remaining uncertainty is execution: whether the product, especially the public publishing experience, is good enough to turn discovery into curiosity, repeat use, publishing, and eventually a small active community.
- Myelin does not require mass-market traffic to be sustainable. Its unusually low operating-cost target means that a small number of paying users can support continued development.

A central distinction emerged:

> Myelin does not need 50,000 one-off visitors. A few hundred or a thousand highly relevant readers, with perhaps 20–30 genuinely active participants, could be much more valuable.

The desired outcome is a small, high-signal community rather than a large low-engagement audience.

---

## 2. OpenHarn as the Initial Distribution Seed

The OpenHarn repository was reviewed as the likely source of Myelin's first serious public material.

The strongest publication candidates identified were:

1. The formal OpenHarn research paper.
2. The BFCL v4 experiment and its grammar/runtime findings.
3. The “I blamed the model; it was the runtime” engineering story.
4. The small-model tool-calling benchmark.
5. The reasoning-token latency investigation.

The key conclusion was that this material is strong enough to plausibly seed Myelin's first audience because it contains:

- original experiments;
- reproducible results;
- surprising failures;
- concrete benchmark deltas;
- negative results and corrections;
- a formal research artifact behind the shorter posts.

The material should not be published as raw research notes. The main work remaining is editorial:

- reconcile different benchmark runs/configurations;
- distinguish subset scores from leaderboard claims;
- separate final conclusions from lab-history notes;
- turn experiment logs into readable narratives.

The preferred publication sequence was roughly:

1. Runtime bug story.
2. BFCL benchmark story.
3. Reasoning-token latency story.
4. Small-model benchmark.
5. Formal paper / full technical report.

The articles are not expected to guarantee large traffic. Their job is to bring the first few hundred unusually relevant people through the door.

---

## 3. Publishing as Myelin's Distribution Engine

The publishing system is intended to create a product-led distribution loop:

**author creates useful work in Myelin  
→ publishes it  
→ shares it wherever their audience already exists  
→ readers encounter both the work and the Myelin medium  
→ some readers remember or try Myelin  
→ some eventually create and publish work of their own  
→ every new author becomes another distribution node**

At zero outside authors, nearly all discovery comes from Myelin's own research.

At five authors, Myelin has five additional independent networks distributing publications.

At twenty active authors, discovery is no longer dependent on one person's writing.

The key insight is that a Myelin publication should not behave like an advertisement for Myelin. The publication itself should demonstrate why the product exists.

The desired reader reaction is:

> “This is interesting work.”

followed by:

> “This is an unusually good way to present it.”

and eventually:

> “I have something I could present like this.”

---

## 4. Myelin Publications as Interactive Research Objects

The public format should go far beyond a conventional article.

A Myelin publication may combine:

- prose;
- source documents;
- highlights and annotations;
- claims linked directly to evidence;
- figures;
- code;
- equations;
- datasets;
- experiment output;
- interactive visualizations;
- executable cells;
- threaded discussion attached to specific passages or objects;
- author profiles;
- linked repositories and artifacts.

The publication is better understood as a structured research object than a blog post.

### Proposed Interactive Primitives

Several possible author primitives were discussed:

- **Sandboxed Python cells**, ideally through Pyodide/WASM rather than native executables.
- **Sandboxed HTML/CSS/JS blocks** for simulations, diagrams, calculators, small demos, and custom visual explanations.
- **WASM blocks** for advanced technical authors.
- **Interactive data tables** with sorting, filtering, grouping, plotting, and row-level inspection.
- **Parameter widgets** such as sliders, toggles, dropdowns, and numeric inputs connected to calculations.
- **Reproducibility capsules** containing code, data, dependency versions, commands, environment metadata, and expected results.
- **Executable before/after comparisons**, especially useful for debugging or benchmark investigations.
- **Interactive benchmark tables** where individual failures can be expanded into raw outputs.
- **Claim → evidence links**, where a sentence can point directly to a dataset row, PDF passage, code line, image region, equation, commit, or experiment.
- **Versioned claims and corrections**, preserving research history while clearly showing updated conclusions.
- **Reader forks**, allowing readers to duplicate and modify an interactive block while preserving attribution.
- **Inline response artifacts**, where replies can contain graphs, equations, code, counterexamples, or dataset slices rather than text alone.
- **Linked figures**, where chart points can reveal the underlying evidence.
- **Interactive equations** whose variables can be modified.
- **Inspectable diagrams**, where nodes reveal explanation, source, or evidence.
- **Experiment timelines**, showing how hypotheses and results changed over time.
- **Prediction/quiz blocks**, especially useful for teaching.

A security boundary was explicitly identified: arbitrary uploaded native executables should not run on readers' machines. Sandboxed browser/WASM execution provides most of the desired expressive power with much stronger containment.

---

## 5. Paid Publishing & the $5 Tier

A clarification was made that public publishing is intended to require a subscription.

The current lower-tier concept is approximately **$4–5/month** and is aimed at people who want:

- managed sync;
- public publishing;
- a public Myelin profile;

without necessarily paying for hosted cloud AI or larger private-group features.

The logic is:

1. A user can use Myelin privately/local-first.
2. They create something personally valuable.
3. They decide they want to share it publicly.
4. Paying $5 turns that local document into a polished public Myelin publication.
5. They share the link on Twitter, Mastodon, HN, email, a research group, a class, or elsewhere.

The conclusion was **not** to lower the price pre-emptively.

$5 is already low compared with adjacent paid publishing/sync products, while still creating a small amount of intentionality. If real conversion data later shows that $5 materially suppresses publishing, pricing can be revisited.

The goal is to make the publishing experience good enough that users are not mentally comparing:

> Myelin $5 vs free Markdown hosting.

They should instead ask:

> “Can the free platform publish this kind of object at all?”

---

## 6. Indexed Authors as a Reputation System

The strongest long-term reputation goal was stated explicitly:

> People should eventually be proud to say, “I’m an indexed author on Myelin.”

The intended model is:

- Anyone can read public Myelin publications.
- Paying publishers can publish and share via direct links.
- A publisher's Myelin profile can be indexed and act as a public professional portfolio.
- Individual publications are **noindex by default**.
- Individual works are promoted into the indexed corpus only after meeting transparent trust and quality criteria.

This creates a distinction between:

- **publishing capability**, which is purchased;
- **public recognition / indexing**, which is earned.

That boundary must be protected. Prestige must never be purchasable.

The desired long-term meaning is not:

> “Myelin says this author is special.”

It is:

> “When I see an indexed Myelin publication, I expect it to be worth my time.”

If that expectation becomes reliable, indexed-author status naturally becomes a credibility signal.

### Reputation Ladder

The mechanism was framed as a progression:

**Stage 1 — Utility**  
“I can express my work better here.”

**Stage 2 — Audience quality**  
“The people reading Myelin are relevant to what I work on.”

**Stage 3 — Identity**  
“I maintain my serious work on Myelin.”

**Stage 4 — Reputation**  
“Publishing and being indexed on Myelin means something.”

The first authors do not need Myelin to be prestigious. They need the publishing medium itself to be useful.

---

## 7. UI Quality as Distribution Infrastructure

A major conclusion was that UI quality is not cosmetic for Myelin. It is part of the acquisition mechanism.

A visitor should understand almost immediately that a Myelin publication is not a normal blog page.

Without reading documentation, they should be able to notice:

- serious author identity;
- unusual document structure;
- evidence linked directly to claims;
- interactive objects;
- source material beside the argument;
- comments attached to specific passages;
- a coherent, high-quality visual system;
- author freedom without visual chaos.

The desired five-second test:

> Show someone a Myelin publication for five seconds without explaining Myelin. Can they tell that this is a different kind of publishing system?

If the answer is no, the design has lost an important part of its distribution advantage.

The public publication page and author profile were identified as Myelin's storefront. They deserve disproportionate polish because many people will encounter them before ever installing the application.

The public page potentially serves four roles simultaneously:

**content + product demo + landing page + distribution object**

---

## 8. Desktop Notes Interface — Current Direction

The current desktop notes interface was reviewed visually.

The strongest existing decision is the core two-pane model:

**source material on the left  
→ author's notes/thinking on the right**

This already communicates the central Myelin workflow before any explanation.

Strengths identified:

- source material is first-class rather than hidden as an attachment;
- the source and writing surface feel like one workspace;
- dark, restrained styling fits serious technical/research work;
- the interface is dense without feeling like a consumer productivity dashboard;
- the large uninterrupted working surfaces are effective.

Areas to refine:

- reduce apparent toolbar density and create stronger hierarchy among controls;
- improve discoverability of less obvious annotation tools;
- make the editor feel more like a rich intellectual workspace rather than merely a text/code editor;
- consider visual relationships between source passages and the notes/evidence that reference them;
- refine title/header handling for long academic titles;
- introduce slightly more surface depth without turning the UI into a card-heavy dashboard.

A particularly strong future interaction would be a subtle temporary visual relationship across the pane boundary:

**PDF/source passage ↔ note claim**

This would make the relationship between evidence and interpretation visible.

---

## 9. Desktop Personalization vs Public-Site Consistency

Myelin's desktop application already supports extensive theming and accent-color customization.

This was judged to be a good fit for the audience because researchers, developers, students, and long-session users often care deeply about their working environment.

The intended split became:

### Private Desktop Workspace

High personalization:

- dark/light themes;
- custom accent colors;
- editable themes;
- theme import/export;
- user-controlled visual preferences.

### Public Myelin Website

Static, controlled visual identity:

- consistent typography;
- consistent spacing;
- controlled color system;
- consistent annotation and interaction patterns;
- strong accessibility baseline.

Authors should have freedom through **content structure and expressive primitives**, not through unrestricted public theming.

This allows public Myelin pages to remain visually recognizable and protects the perceived coherence of the indexed corpus.

---

## 10. Public Web Design: Not Merely “Clean”

An important correction was made to the earlier description of the desired web aesthetic.

The goal is **not** simply a clean editorial site with a few distinctive interactions.

The desired identity is a collision between:

### Physical / Real

- documentary or atmospheric photography;
- scans;
- papers;
- books;
- figures;
- landscapes;
- materials;
- people;
- grain and blur;
- off-white paper-like surfaces;
- editorial typography.

### Digital / Constructed

- hard rectangular crops;
- saturated synthetic color blocks;
- pixel-like forms;
- ASCII/dot structures;
- grids;
- technical metadata;
- computational marks;
- UI-like symbols;
- deliberately artificial overlays;
- sharp geometric interruptions.

The core phrase that emerged was:

> **That collision is the identity.**

A useful description of the aesthetic is:

> **editorial materiality + computational intervention**

or:

> **real source material + digital intelligence layered on top**

This is conceptually aligned with the product itself.

Myelin takes real intellectual material—papers, books, experiments, photographs, figures, notes—and adds digital structure without replacing the source:

- annotations;
- AI;
- evidence relationships;
- executable objects;
- search;
- discussion;
- computation.

Therefore the branding is not an arbitrary aesthetic attached to the product. The visual metaphor and the product behavior express the same idea.

---

## 11. Visual System & Moodboard Direction

The moodboard references showed several recurring techniques:

- refined editorial typography;
- large quiet compositions;
- documentary or atmospheric photography;
- aggressive cropping;
- synthetic blocks intersecting with photographs;
- pixel structures;
- ASCII/data-like textures;
- bright controlled accent colors;
- compositions that could plausibly exist as posters, books, reports, or physical editorial pieces.

The objective is not to make every paragraph visually experimental.

A better rhythm is:

**high-expression moment  
→ quiet reading  
→ interactive/research object  
→ quiet reading  
→ high-expression moment**

The reading experience remains disciplined while the larger composition becomes highly recognizable.

### Potential Signature Myelin Primitives

- **Digital block** — a saturated geometric element that cuts into source/photographic material.
- **Data texture** — dots, monospace metadata, grids, revision IDs, coordinates, or machine-readable-looking structures.
- **Cropped reality** — real images that overlap, split, or cross structural boundaries.
- **Editorial + technical typography** — physical/editorial type contrasted with computational monospace language.
- **Evidence intervention** — opening evidence visibly alters or interrupts the article composition.
- **Instrument-like interactive blocks** — executable objects that feel like research instruments rather than generic code fences.

A warning was identified: the system must not collapse into “random colorful rectangles over photographs.”

The digital intervention should ideally have semantic purpose:

- evidence;
- computation;
- annotation;
- provenance;
- metadata;
- state;
- linkage;
- interaction.

This would allow readers to gradually learn Myelin's visual grammar.

The target reaction:

> “I don't know exactly why, but this looks like it was published on Myelin.”

---

## 12. Distinctiveness & Novelty

The individual design ingredients are not new by themselves.

Editorial typography, documentary photography, geometric overlays, grids, grain, pixel structures, and monospace details all have precedent.

The potential novelty comes from the **complete system** and its functional alignment with Myelin.

The combination was judged roughly as:

- very strong conceptual fit with Myelin;
- high potential distinctiveness;
- moderate novelty at the level of individual techniques;
- high novelty potential when those techniques become a coherent product/interaction language.

The recommendation was to commit to exploring this direction rather than retreating into a generic clean publishing aesthetic.

---

## 13. Open Source Without the Usual Quality Tradeoff

A broader ambition was articulated around Myelin's open-source identity.

The premise is not that open-source software is inherently lower quality. However, many open-source consumer-facing projects are perceived as less polished because their communities and contribution models are often engineering-first.

Open-source projects commonly optimize for:

- code;
- features;
- integrations;
- performance;
- bug fixes.

Design, editorial quality, visual identity, illustration, photography, motion, writing, and accessibility often become secondary.

Myelin aims to challenge that pattern.

The goal is:

> **Open source should not mean engineering-first and everything-else-second.**

Myelin should treat non-code creative and intellectual work as first-class contribution.

Potential contribution areas include:

- Code
- Design
- Research
- Editorial
- Illustration
- Photography
- Accessibility
- Templates
- Teaching

A photographer contributing a serious visual series, an illustrator building a visual explanation system, a typographer improving mathematical reading, a teacher producing an exceptional annotated lesson, or a researcher publishing careful experiments should all be understood as meaningful contributions to Myelin.

This makes Myelin closer to:

> a public intellectual and creative project whose software is also open source

rather than merely:

> source code that programmers are allowed to modify.

---

## 14. Early Indexed Authors Should Span Disciplines

A key strategic refinement was to avoid seeding the first indexed-author cohort exclusively from technical computing fields.

If the first ten indexed authors are all:

- local-LLM engineers;
- Rust developers;
- systems programmers;
- ML researchers;

then Myelin may become permanently categorized as a technical publishing platform.

Instead, early authors should ideally include strong people from different fields, for example:

- machine-learning researchers;
- photographers;
- architects;
- biologists;
- illustrators;
- mathematicians;
- historians;
- teachers;
- engineers;
- independent researchers.

The common quality is not profession or subject matter.

It is **seriousness, originality, rigor, and boundary-pushing work**.

The desired meaning of “indexed Myelin author” is:

> this person produces work worth paying attention to.

Not:

> this person writes about computers.

It was also noted that early authors do not need to be famous.

Myelin should ideally become somewhere people *discover* excellent thinkers and creators, rather than simply reproducing existing status hierarchies.

The strongest possible long-term signal would be:

> “I hadn't heard of them before, but they're an indexed Myelin author, so I checked their work.”

---

## 15. Current Strategic Priorities

After the full discussion, the main execution priorities appear to be:

1. **Make the public publication UI exceptional.**
2. **Turn the strongest OpenHarn investigations into polished flagship Myelin publications.**
3. **Use those publications to demonstrate the medium rather than advertise it.**
4. **Invite a very small number of strong authors from different fields early.**
5. **Give authors expressive primitives that ordinary publishing platforms cannot match.**
6. **Keep public visual identity coherent while allowing substantial private desktop personalization.**
7. **Protect indexed status so that it cannot be purchased or diluted.**
8. **Treat design, research, editorial, visual art, teaching, and accessibility as genuine open-source contribution paths.**
9. **Optimize for a small, respected, high-signal community rather than traffic volume.**

---

## 16. Distilled North Star

The product, visual identity, publishing system, reputation model, and open-source culture are converging on the same idea:

> **Myelin takes real work seriously, then gives people better computational tools to understand, develop, present, and discuss it.**

The public platform should therefore feel neither like a conventional blog nor like a software dashboard.

It should feel like a new publishing medium where physical/editorial material and computational structure coexist.

The long-term cultural ambition is equally important:

> Myelin should be a place where excellent people across disciplines can produce unusually expressive work, where readers expect high signal, and where being an indexed author becomes something worth being proud of.

---

*Compiled from the continuation of the original Myelin planning conversation. This document records current direction and reasoning rather than final product specifications.*
