# Myelin — Collaboration, Discovery & Indexing Architecture

*Continuation of `myelin-contributors-collaboration-and-reciprocity.md`.*

This document records the clarified product plans discussed afterward. It intentionally omits discarded interpretations and keeps only the intended architecture.

---

## 1. Collaboration Is a Core Growth Mechanism

Collaboration is not a secondary community feature. It is intended to be one of Myelin's main adoption and retention loops.

The core idea is that creators should have an explicit reason to collaborate with authors whose publications already attract relevant readers.

Example:

- an indexed researcher publishes a strong article;
- the article has little or no visual material;
- a photographer, illustrator, designer, or other creator discovers the article;
- the creator already has relevant existing work;
- they offer that existing work for use in the publication;
- the author accepts;
- the publication becomes stronger;
- the creator receives highly visible credit and exposure to the publication's audience.

The creator is not being asked to make something new on demand.

The value comes from **placing existing high-quality work into another high-quality work where it can reach a relevant audience**.

---

## 2. Existing Work Only — Not a Request Marketplace

This remains an explicit product rule.

Myelin collaboration is for discovering, reusing, adapting, citing, or building on work that a creator has already chosen to publish.

It is not intended to become a board where people request unpaid bespoke work.

Suggested product copy:

> **Authors can ask to reuse or build on your published work; you're never asked to create something new for someone else.**

This keeps the collaboration model aligned with creator ownership and avoids turning Myelin into an unpaid commission marketplace.

---

## 3. Why Creators Have a Strong Incentive to Collaborate

A creator may want their existing work included in an indexed author's publication because the collaboration can put their name and work in front of a highly relevant audience.

The value is not generic traffic.

The value is **contextual visibility**.

Examples:

- photography shown inside a respected research or field article;
- illustrations embedded into an indexed essay;
- diagrams incorporated into a technical explanation;
- visual work attached to a historical or scientific investigation;
- data visualizations reused inside another author's argument.

The creator's work appears where readers are already paying attention.

Credit should therefore be prominent rather than hidden in a small caption.

Possible publication metadata:

> **Written by A**  
> **Photography by B**  
> **Interactive figures by C**

Each contributor name should link directly to their Myelin profile.

---

## 4. Collaboration Can Unlock Publisher Access

A meaningful accepted collaboration can qualify the creator for complimentary access to the approximately $5 Publisher/sync tier.

This creates a strong reciprocal loop:

**creator publishes good existing work  
→ indexed author discovers it  
→ creator's work is accepted into a publication  
→ creator receives visible credit  
→ creator receives Publisher access  
→ creator begins publishing their own original work  
→ their work becomes visible throughout Myelin  
→ further readers and collaborators discover them**

The free subscription is viable because the marginal cost of the lower tier is expected to be very small.

It also does not weaken Myelin's anti-spam protections because the creator has already demonstrated value: another person had to accept their work as a meaningful contribution or collaboration.

---

## 5. Contributor Profiles Should Showcase Contributions

Contributor profiles should not merely state that someone is a contributor.

They should visibly show what that person has done.

A profile may include:

- publications they authored;
- publications they collaborated on;
- visual work incorporated into other publications;
- product contributions;
- design contributions;
- research contributions;
- accessibility improvements;
- code contributions;
- datasets;
- illustrations;
- photography;
- teaching material;
- other accepted contributions.

Each contribution should link to the actual artifact, publication, project, feature, or contribution record whenever possible.

For visual disciplines, the profile should show the work visually rather than reducing it to a text list.

A photographer may have image previews.

An illustrator may have diagrams.

A designer may have interface or interaction previews.

A developer may have shipped features or repository contributions.

The goal is for the profile to function as a durable public history of participation.

---

## 6. Public Work Is Discoverable Even When It Is Not Indexed

A critical part of the architecture is that **public** and **indexed** are not the same thing.

A publication can be public without being externally indexed.

Public non-indexed work can still be:

- visible to anyone browsing Myelin;
- shown in Myelin's public feed;
- discoverable through internal search;
- visible on the author's profile;
- linked from other Myelin publications;
- shared directly by URL;
- discussed;
- used for collaboration;
- surfaced through relevant internal discovery systems.

This means new creators are not invisible simply because their work has not yet entered the indexed corpus.

Myelin itself remains a discovery environment.

---

## 7. The Public Feed Is an Important Discovery Layer

Myelin should have a public listing/feed of public work, including work that is not search-engine indexed.

This gives new authors and creators a real path to discovery before they become indexed.

The feed can become a place where readers encounter:

- new publications;
- visual projects;
- research notes;
- experiments;
- essays;
- datasets;
- interactive work;
- collaborations;
- emerging authors.

The feed therefore supports the early-stage ecosystem without weakening the external quality bar.

The public feed is broad.

The externally indexed corpus is selective.

---

## 8. Myelin Indexed and Search-Engine Indexed Are the Same Gate

The intended architecture is:

> **Myelin Indexed = permitted to enter the search-engine-indexed public corpus.**

There is not a separate category where low-quality public work is freely indexed by Google but simply lacks a Myelin badge.

That would defeat the purpose of the quality system.

The reason is domain authority and first impressions.

A new visitor may discover Myelin through Google before ever visiting Myelin directly.

If low-quality or mass-generated pages from the Myelin domain dominate search results, the distinction between "officially endorsed" and "merely hosted" work will not matter to that visitor.

They will associate the low-quality work with Myelin itself.

Therefore the external searchable corpus must be treated as an editorial surface.

---

## 9. Default Public State: `noindex`

Public publications should remain `noindex` by default.

They can still participate fully inside Myelin, but search engines are instructed not to include them in the external corpus.

This protects:

- domain authority;
- perceived quality;
- search reputation;
- the meaning of Myelin indexing;
- new users' first impressions of the platform.

A work is promoted out of `noindex` only when Myelin is willing for that publication to represent the domain externally.

That is the practical meaning of becoming indexed.

---

## 10. What "Indexed" Means

Indexing is not simply a badge.

It is an endorsement with a concrete consequence:

> **Myelin is willing for this work to represent the platform to the outside web.**

An indexed work:

- is eligible for search-engine indexing;
- becomes part of Myelin's externally visible corpus;
- contributes to the domain's search reputation;
- carries a stronger quality signal;
- can strengthen the author's authority on Myelin.

The long-term goal remains for "indexed on Myelin" to become a meaningful reputation signal.

---

## 11. The $5 Publishing Fee Is a Protection Layer

The approximately $5 publishing tier is not intended primarily as an exclusivity mechanism.

Its main strategic role is to create friction against low-effort abuse and mass content dumping.

Someone who wants to publish publicly has to make an intentional decision to do so.

This makes it less attractive to use Myelin as a destination for disposable AI-generated content or mass spam.

The fee therefore acts as an anti-garbage layer.

It does **not** buy indexing.

A paying publisher can publish publicly, but their work remains `noindex` until it earns entry into the indexed corpus.

---

## 12. Contributor Access Does Not Weaken the Protection Layer

Meaningful contributors may receive the Publisher tier for free.

This is viable because the contribution itself already creates an alternative trust filter.

A person cannot simply declare themselves a contributor.

Something they made must be accepted as a meaningful contribution or accepted collaboration.

Examples:

- a code contribution is merged;
- a design contribution is adopted;
- an illustration is accepted into a publication;
- a photographic work is accepted for use;
- a dataset or research contribution is incorporated;
- an accessibility improvement is accepted;
- another substantive contribution is approved.

This means complimentary access is not equivalent to unrestricted free publishing.

The person has already demonstrated some value to the ecosystem.

---

## 13. Three Different Protection Layers

The architecture therefore has three distinct filters.

### Publishing fee

Filters for **intent**.

It raises the cost of mass low-value publishing.

### Accepted contribution / collaboration

Provides an alternative route based on **demonstrated value**.

It can qualify someone for complimentary Publisher access.

### Indexing

Filters for **external quality and reputation**.

It decides which work Myelin is willing to have represent the domain in search engines.

These layers serve different purposes and should remain separate.

---

## 14. The Collaboration Snowball

The intended network effect is:

**strong indexed authors attract high-quality readers  
→ creators see an opportunity to place relevant existing work into respected publications  
→ collaborations improve those publications  
→ creators receive prominent attribution and Publisher access  
→ creators publish their own original work  
→ Myelin's public feed gives that work visibility  
→ readers and other creators discover them  
→ more collaborations happen  
→ consistently excellent creators may eventually become indexed authors themselves  
→ their authority attracts additional readers and collaborators  
→ the quality and density of the network increase**

This creates a self-reinforcing system built around high-quality people rather than raw traffic.

---

## 15. High-Quality Visitors Matter More Than Raw Volume

The network does not need enormous traffic for this loop to be useful.

A comparatively small audience can be valuable if it is composed of people who are:

- researchers;
- designers;
- photographers;
- illustrators;
- teachers;
- engineers;
- scientists;
- writers;
- independent creators;
- other serious practitioners.

A few thousand highly relevant readers can create more meaningful collaboration and adoption than tens of thousands of low-intent visits.

The aim is density of quality rather than maximum reach.

---

## 16. Collaboration Strengthens the Network Moat

As this system develops, Myelin's defensibility becomes less about software features alone.

The stronger moat becomes the combination of:

- good authors;
- good creators;
- strong publications;
- visible collaboration histories;
- contributor profiles;
- internal discovery;
- external indexing standards;
- accumulated reputation;
- excellent presentation.

Each part increases the value of the others.

The more serious work exists on Myelin, the more attractive collaboration becomes.

The more collaboration happens, the stronger the work becomes.

The stronger the work becomes, the more valuable indexing becomes.

The more valuable indexing becomes, the stronger the incentive is to publish excellent work.

---

## 17. Observable as a UX Reference

Observable is a useful reference for the interaction quality of Myelin's publishing experience.

The useful lesson is not to copy Observable's visual identity.

The useful lesson is that executable and interactive material can live directly inside a readable editorial document without making the page feel like an IDE.

Relevant principles include:

- contextual insertion of interactive objects;
- clean separation between document reading and editing controls;
- prose remaining visually dominant;
- computation appearing naturally inside the document;
- interactive blocks feeling like document objects rather than external embeds.

For Myelin, this interaction discipline can be combined with its own much more distinctive physical/editorial + computational/digital visual identity.

A useful shorthand is:

> **Observable-level document interaction discipline + Myelin's own visual language.**

---

## 18. Distilled Architecture

The current plan can be summarized as:

**Anyone can read.**

**Publisher users can publish publicly.**

**Contributors and accepted collaborators can receive Publisher access at no cost.**

**Public work is discoverable inside Myelin even when it is not externally indexed.**

**The public feed allows emerging creators to gain visibility.**

**Search-engine indexing is selective and represents Myelin's external quality bar.**

**Collaboration lets creators place existing work into other serious publications with prominent attribution.**

**Successful collaboration can turn creators into publishers.**

**Strong publishers can become indexed.**

**Indexed work attracts more high-quality readers and collaborators.**

The intended result is a compounding network of serious work and serious people rather than an unrestricted content platform.
