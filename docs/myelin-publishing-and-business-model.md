# Myelin — Publishing, Collaboration & Business Model Notes

*A record of the planning discussion that began with the idea of building Hypothesis + Google Docs comments + Genius.com-style annotation into Myelin.*

---

## 1. The Annotation & Sharing Concept

Paper proposed replacing the usual workflow of sending someone a PDF plus a separate notes file with a single shareable link. Opening the link — no install, no account required — would show the paper, Paper's highlights, margin notes, an AI summary, and a discussion thread, all in one place. Recipients could add their own annotations in response.

The concept combined three interaction patterns:

- **Inline annotations**, Google Docs-style: select text, add a comment, see an icon in the margin that expands on click.
- **Cross-document annotations**: pointing to a specific section of an attached PDF, with the quoted text and a link back to the original context.
- **Interactive annotations**: clicking jumps to the highlighted text, replies are threaded to specific annotations rather than dumped at the bottom of the page, and the quoted context is always preserved.

This was identified as closely resembling **Hypothesis** (cross-document, academic-leaning web annotation) and **Genius.com** (highlight-and-expand annotation, originally for song lyrics, later extended to any webpage via a browser extension). Genius's highlight/expand interaction pattern was noted as particularly polished and worth studying directly.

### Privacy Clarification

An early concern was raised that a public, unauthenticated share link might conflict with Myelin's local-first, end-to-end-encrypted vault model. Paper clarified that only content the user explicitly *publishes* would ever leave the encrypted vault — publishing is an opt-in, one-way action for a single document, not a change to the platform's privacy policy. The vault stays private by default; publishing is a separate, clearly-labeled mode that a document enters only when the user chooses it.

---

## 2. Scope Decision: No Real-Time Collaboration

Live, simultaneous collaboration (shared cursors, live-typed edits visible to others in real time) was discussed and explicitly ruled out of core scope, for several reasons:

- It requires CRDT-based sync infrastructure (e.g., Yjs, Automerge) and a persistent connection per session — a materially larger technical lift than asynchronous commenting.
- Academic and research annotation is naturally asynchronous; Hypothesis, the closest existing product in this space, does not offer live collaborative cursors and is still the market leader.
- Building real-time sync would pull Myelin's architecture away from its privacy-first, local-first positioning.

Later in the conversation, the question was revisited specifically for **group research use cases** (a research team co-annotating a paper, or co-drafting a synthesis document together). The conclusion held: live co-reading of a source document doesn't match how research groups actually work (they read independently, then discuss synchronously via a call, which already has a natural venue). Live co-*editing* of a shared note was acknowledged as a more legitimate use case, but the decision was to ship the asynchronous, capped private-group-sharing feature first and only build real-time sync if users of that feature explicitly request it — rather than building CRDT infrastructure speculatively.

A further architectural correction emerged from a screenshot of the Myelin interface: annotation (the source-document pane) and note-writing (the right-hand pane) happen in the **same window, in the same session** — this is the core interaction model, not two separate flows. What the user sees while annotating and writing is exactly what gets published. This means any future live-collaboration feature could not be built as two separable pieces ("live annotation" and "live note editing" shipped independently); it would have to be a single unified synced session covering both panes together, since they are not separable in the product as designed. This reinforced the decision to defer real-time collaboration rather than build a partial version of it.

---

## 3. Institutional Licensing & LMS Context

Paper questioned why institutions would pay for something open source. The distinction drawn: institutions aren't paying for the code (free either way) — they're paying for hosting, dedicated support, compliance (FERPA, data processing agreements), LMS integration maintenance, and SLA guarantees. **Hypothesis** was cited as direct precedent: free for individuals, paid institutional licensing based on user count (real examples cited: roughly $4/user/year at a smaller scale, down to about $1.60/user/year at larger scale).

**Canvas, Blackboard, and Moodle** were explained as Learning Management Systems — the software universities already use for gradebooks, enrollment, assignment submission, and course administration. The guidance was explicit: Myelin should not attempt to replicate LMS functionality. The correct posture, mirroring Hypothesis's approach, is integration (an LTI plugin so a professor can embed Myelin inside Canvas) rather than competition.

**Live in-class annotation** (a professor's class annotating the same paragraph simultaneously, seeing interpretations form live) was explained as a genuine but narrow pedagogical technique from humanities/close-reading instruction — valuable in that context, but not a natural fit for engineering or CS material, where there's less interpretive disagreement to surface live.

---

## 4. Teaching Use Case: Annotated PDFs as Lesson Material

Paper proposed that teachers could send an annotated PDF (highlights, margin notes, questions) instead of building a slide deck — students get the source material plus the teacher's thinking, rather than the teacher's lossy summary of it in bullet points. This was validated as a strong use case requiring no new engineering: it's the existing publish/annotation feature applied to a classroom setting.

A follow-up feature table proposed:

| Idea | Assessment |
|---|---|
| Annotated PDF as the lesson | Already covered by the publish feature |
| Embedded questions next to figures | New but low-complexity — a special annotation type (`type: question`) layered on the existing annotation system |
| Live annotations during class | Reintroduces the real-time collaboration complexity already ruled out; not adopted |
| Student annotations/responses | Already covered by the async comment/reply system |
| Export as a shareable "course pack" | Already covered — mostly a packaging/export format problem, not a new system |

The "library of resources teachers share with other teachers" idea was identified as a **separate product** (a content marketplace/discovery platform, akin to TES Resources or Course Hero) — deferred indefinitely, to be considered only if organic demand for it appears after the core publish feature ships.

The final position: nothing live, but the teaching-context application of the existing publish/annotation feature is a strong, validated extension of it.

---

## 5. Business Model

### Core Structure

Myelin is planned as **fully free and open source**, with no feature gating and full self-hosting support for anyone who wants it. Paid plans exist solely for managed convenience:

- **Sync** — encrypted sync across devices and browser access.
- **AI** — hosted cloud inference via Cloudflare Workers AI with zero data retention, plus anonymous end-to-end-encrypted VPS hosting.

Paper's stated intent: **no margin is taken on the AI tier specifically** — it's priced to cover compute cost, not to generate profit, as a deliberate trust signal in a market where AI subscriptions are often heavily marked up. **Hosting/sync does carry real margin**, since a single user's usage doesn't come close to consuming a full shared VPS — this is treated as normal multi-tenant SaaS economics, not a contradiction of the "no margin" AI promise.

### Market Validation

The market was assessed as real and proven: MarginNote, GoodNotes, Notability, LiquidText, and PDF Expert all have large paid userbases. Paper's audience — engineering and research students — was identified as underserved relative to text-heavy fields like law and medicine, and as a high-willingness-to-pay segment (a PhD or med student will pay $50–100 for a tool that genuinely saves time).

### Distribution & Marketing Strategy

Paper expressed skepticism that posting on Hacker News or Reddit, or "building in public," would meaningfully help — preferring instead to rely on the platform's own technical blog and original research (building on Paper's existing novel work in local AI) ranking organically in search over time.

The counter-position raised: SEO is a real and durable channel, especially for genuinely original technical content, but it is a *compounding*, not fast, channel — a new domain typically takes 6–12+ months to rank for competitive terms, and AI/LLM-adjacent keyword space is heavily saturated. It was argued that HN/Reddit and an SEO strategy aren't in tension — a well-written technical post (e.g., explaining the VRAM-safe offload formula) posted to HN is the same asset as the blog post, and the traffic and backlinks it generates are what accelerate the SEO ramp, rather than competing with it. Paper's clarified position: he will still post there, but does not expect much from it beyond backlinks, and prefers patient, stable, non-spammy growth of a respected platform over cross-posting for quick reach. Both sides agreed that gaming HN/Reddit purely for backlinks would be counterproductive and was never the recommendation.

Paper was direct that quality content over two years does **not guarantee** reaching 100 paying users — it guarantees authority and respect within the niche, but discovery, conversion (reading isn't paying), product/pricing fit, and the pace of change in the local-AI space are all separate risk factors outside the content strategy alone.

---

## 6. Timeline & User Acquisition Projections

Initial estimate for reaching 100 paying users, from a solo developer with genuinely differentiated technical work, ranged **6–18 months**, with 9–14 months as the realistic middle case.

Paper set an internal goal of pushing this to roughly **3 months**, while stating clearly he would not give up if that goal wasn't hit — treating it as a stretch target rather than a hard deadline.

Paper's stated financial threshold: **50 paying users at $10/month ($500/month)** would be enough to justify going full-time — framed less as meaningful income on its own and more as proof that a real funnel exists.

A rough Q1–Q4 year-one projection was produced based on the finalized plan (mobile-first, content-led growth, tiered pricing, no institutional sales yet):

- **Q1:** low hundreds of free/self-host installs; single digits to ~15 paying users.
- **Q2:** mobile ships, paying users climb to roughly 30–70 cumulative.
- **Q3:** community starts forming; 70–150 cumulative.
- **Q4:** 100–250 cumulative paying users is a defensible year-one range.

This was later revised upward after Paper confirmed mobile could ship on day one (see Section 7), pulling Q1 paying users to an estimated 20–40 rather than single digits, since the historically higher-converting iPad/stylus segment would be in the funnel from launch rather than arriving three months late.

---

## 7. Mobile Strategy

Paper initially floated mobile/tablet apps as a way to grow Sync-tier adoption, reasoning that mobile/tablet users are less technical and less likely to self-host than the FOSS-enthusiast desktop crowd. This was later escalated to a stronger position: that mobile/tablet access is **the key to acquiring any paying customers at all**, not merely a secondary driver — since the technical, self-hosting desktop crowd is structurally unlikely to ever convert to a paid plan.

This was supported by re-examining the reference apps discussed earlier in the conversation: MarginNote is Mac and iPad only, with no self-hosting option, and has a substantial paying userbase — as do GoodNotes and Notability, both fundamentally tablet-centric, stylus-driven products. The refined conclusion was that the paying market in this category is specifically **iPad-plus-stylus**-centric, not "mobile" in the broad phone-inclusive sense — a phone screen is a poor form factor for annotating research papers.

Execution risks noted: Myelin's Tauri 2 stack does support mobile targets, reducing (but not eliminating) the cost of a second platform; stylus/handwriting responsiveness inside a webview may lag native rendering and was flagged as worth prototyping early; Apple's 30% subscription cut would affect the already-thin-margin AI tier if routed through the App Store.

Paper later confirmed mobile/tablet could ship on **day one** by excluding full LaTeX authoring (assessed as a desktop-only workflow with negligible mobile demand) in favor of a lazy-loaded preview instead of the instant rendering achieved on desktop. Every other feature was reported as already working on phone at that point in development.

**No local AI is planned for mobile devices** — mobile AI features route through the cloud tier exclusively, which was noted as a natural incentive to purchase the paid plan on mobile specifically, rather than a compromise.

---

## 8. The Publish Feature: Moderation, Domain Authority & Indexing

### The Core Tension

Paper raised a concern: if anyone can publish on Myelin, does it degrade into something like Medium, where reading a post is often less useful than an AI-generated search overview? Two mechanisms were discussed as protection.

### Payment as a Partial Filter

Paid-only publishing (already the plan — publish is not available on the free tier) filters out casual low-effort spam, but was noted as a weak defense against dedicated abuse (SEO content farms, professional spammers), since $10/month is a low cost for anyone deriving backlink value from posting.

### Domain Authority Dilution — The More Important Problem

A more structural risk was identified: mixing user-generated content on the same domain as Paper's own carefully-researched blog posts could drag down how search engines evaluate the *entire domain's* trust and quality, since modern search quality evaluation increasingly operates at the domain level, not purely per-page. This is the same failure mode that has historically affected heavy-UGC platforms like Medium, Quora, and Reddit.

A subdomain-isolation approach (hosting published documents on a separate subdomain from the research blog) was initially proposed as a fix, but Paper rejected it: publishing on an established, authoritative domain is precisely the incentive for using Myelin's publish feature over a personal blog or Medium — isolating it to a subdomain would remove that incentive entirely.

### The Resolved Approach

The final design: all published pages live on the **main domain**, set to **noindex by default** (so direct link-sharing works immediately, but the page doesn't enter search results). Pages are promoted to **indexed** status only after meeting **transparent, ideally partly automated** trust and quality criteria — for example, account age, an absence of reports, and a minimum quality signal, potentially pre-screened using Myelin's own AI stack before any manual review.

This design was explicitly modeled on, and intended to avoid the failure mode of, **old Twitter's blue-check verification**: genuinely prestigious and respected while the criteria were followed, but a source of significant backlash because the actual bar for approval was opaque and perceived as favoring already-prominent people. Transparent, mechanical criteria were identified as the way to get the prestige effect without the opacity backlash.

### Profile Pages as a Separate, Always-Indexed Asset

A late insight (recorded via a handwritten note) refined this further: even while an individual post remains noindexed, a publisher's **profile page** — aggregating their published works into a portfolio — can be indexed by default. This mirrors how GitHub, ORCID, and Google Scholar profiles work: the aggregated professional presence has standalone discovery value independent of whether any single piece of work has been individually promoted. Because Myelin controls the profile page's structure (name, affiliation, list of works, activity), it carries much lower domain-quality risk than indexing arbitrary raw post content, and gives every publisher — even unverified ones — a tangible reason to build a public presence on Myelin from day one.

### Reputation as a Reason to Pay

Paper noted that, once established, people may want to pay specifically to publish on a well-regarded, high-authority platform — the way researchers pay journal fees for prestige and citation value rather than for editing tools. This was validated as a real and proven dynamic (academic publishing, Substack's top writers), but flagged as **circular by nature**: it only becomes a viable pitch after the platform has built real authority, so it can't be the argument used to acquire the first wave of users — only later ones, once Paper's own research has demonstrably built the reputation.

---

## 9. Persona & Tier Segmentation

Three user segments were mapped against the product's tiers:

- **Researchers** — use the publish/posting platform the most, for both reading and publishing high-quality original work. Paper is personally committed to building the platform's authority through his own research writing over a period of roughly two years.
- **Students & teachers** — an annotation-and-publish flow requiring at least one paying subscriber (typically the teacher) to publish notes and annotations (noindexed by default); everyone else reads and comments for free, Hypothesis-style. Over time this builds a reusable "bank of notes" tied to a given textbook, valuable for years as long as the textbook remains in use, and useful even to informal or non-enrolled learners.
- **Independent learners** — for students without a teacher publishing content for them, a separate **sync-only tier (no AI)** provides cross-device cloud storage of their own notes.

### Private Group Sharing

A related open question was resolved during the discussion: whether small-group private sharing (as distinct from public publishing) belongs on the individual paid plan or only on an institutional plan. The resolution: private sharing is available on the **$10 individual plan, capped at roughly 5–10 people** — covering the realistic study-group or small-lab use case without requiring an institutional sale. **Uncapped, admin-managed group sharing** is reserved for the institutional plan.

A further refinement flagged the risk that a single teacher's individual subscription already grants an entire class free reading and commenting access via public publish — meaning institutional plans need to sell something an individual account genuinely cannot provide: multi-teacher admin oversight, groups larger than the private-sharing cap, LMS integration, compliance paperwork (e.g., FERPA, data processing agreements), and potentially an institution-level verified badge on published content.

---

## 10. Pricing

### Cloud AI Cost Modeling

Initial cost estimates for the AI tier were grounded in Cloudflare Workers AI pricing for an 8B-class model (roughly $2.50 per million input tokens, $10 per million output tokens), suggesting an estimated $2–8 per active user per month in raw inference cost.

Paper subsequently identified **Gemma 4 26B A4B** — a Mixture-of-Experts model (26B total parameters, roughly 4B active per token) released by Google DeepMind, Apache 2.0 licensed, with native vision and document/PDF understanding, and benchmark performance close to a much larger dense model despite its lower active-parameter compute cost. Pricing was found to be roughly $0.10 per million input tokens and $0.30 per million output tokens — around 25–30x cheaper than the earlier 8B-model estimate. Revised cost estimates put realistic AI cost per active user closer to $1–2 per month, giving the "no margin on AI" pricing promise substantially more headroom than initially modeled. A caveat was noted: multimodal/vision token pricing (relevant if PDF pages are sent to the model as images rather than extracted text) may run higher than plain text token pricing, and should be checked before finalizing this as the backend.

### Fair Pricing at Scale

Comparing Myelin's planned bundle (sync, cloud AI, publish platform, capped group sharing, AI-generated diagrams, mobile and desktop access, privacy-first architecture) against single-purpose competitors — Obsidian Sync, Readwise Reader, ChatGPT Plus, MarginNote — the $10/month price was assessed as substantially underpriced relative to the value delivered. This was framed as an intentional, strategic choice for the early adoption phase (removing price as an objection while proving the funnel works), not a long-term mistake — with the caveat that the pricing page needs clear messaging so the number of bundled capabilities doesn't create the same "too much at once" confusion previously flagged about the app's dashboard UI.

A rough revenue model at 1,000 paying users, using the revised Gemma-4-based AI cost estimate and existing hosting-margin assumptions, suggested that a mature price in the **$15–18/month** range would be defensible and still cheaper than competitors offering far less, while giving meaningfully more operating margin than the launch price.

### Finalized Pricing Decisions

- New signups pay **$15/month** for the combined Sync + AI + Publish + capped private-group plan once the platform reaches 1,000 paying users.
- The **first 1,000 paying users are grandfathered at $10/month permanently**, as a loyalty and trust-building move — assessed as low-risk and sustainable given the much lower AI cost estimates from the Gemma 4 pricing discovery.
- A new **lower tier at $4–5/month** is under consideration, covering sync and **public-only** publishing (no cloud AI, no private/select-group sharing) — aimed at users, particularly self-hosters running local AI already, who want managed sync and public publishing without paying for cloud inference they won't use. This tier was initially framed as "half of Obsidian's price."

### Pricing Correction

A comparison to Obsidian's actual pricing (Sync: $5/month; Publish: $10/site/month, sold as a separate product) corrected an earlier assumption that Obsidian Sync cost $8/month. This strengthened rather than weakened Myelin's pricing story: a user wanting both sync and publishing on Obsidian pays $5 + $10 = $15/month across two separate products, whereas Myelin's proposed $4–5 tier would bundle equivalent functionality into a single price — closer to 70% cheaper for the combined capability than the original "half price" framing suggested.

Obsidian's pricing page was also noted as visually more polished than Myelin's current state — attributed to Obsidian's years of iteration and revenue-funded design work rather than a fundamental gap, with the practical takeaway that the pricing page specifically (as the conversion moment) deserves disproportionate design attention early on, even while the rest of the app remains a work in progress.

---

*Compiled from a single planning conversation. Reflects Paper's stated decisions and open questions as of the time of writing; several items (the $4–5 tier, the AI backend model choice, and vision-token cost verification) remained explicitly undecided or pending validation at the end of the discussion.*
