# Myelin — Open Platform, Author Trust & Visible Practice

*Continuation of `myelin-culture-public-improvement-and-excellence.md`.*

This document records the decisions and product principles established afterward.

---

## 1. The Online Platform Should Remain Open Source

Myelin's online platform should remain open source.

The distinction is not:

> **open source vs secure**

It is:

> **open product, private operations**

The product code can remain inspectable, forkable, modifiable, and self-hostable without exposing production secrets or operational defenses.

### Open-source surfaces can include

- the website and frontend;
- publication rendering;
- profiles and feeds;
- Myelin Commons;
- annotations and choreography;
- collaboration and proposal systems;
- backend application code;
- APIs and schemas;
- private collaboration protocols;
- self-hosting implementation;
- design system;
- accessibility implementation;
- moderation interfaces.

### Production-only material should remain private

- credentials and API keys;
- database and storage secrets;
- signing keys;
- production environment configuration;
- privileged deployment access;
- unreleased vulnerability details;
- internal incident-response material;
- exact anti-fraud fingerprints;
- exact abuse-detection thresholds;
- operational defenses whose effectiveness depends on not publishing their precise configuration.

The governing principle is:

> **Everything whose transparency benefits users and contributors should be open. Everything whose secrecy is operationally necessary should remain private.**

Security should not depend on hiding the source code.

---

## 2. Open Source Extends Beyond Engineering

Keeping the online platform open source is also important because Myelin's contribution model is broader than code.

The public website itself is intended to be a collaborative artifact.

Contributors may improve:

- typography;
- motion;
- visual systems;
- accessibility;
- interaction patterns;
- layouts;
- imagery;
- editorial composition;
- documentation;
- moderation tooling;
- technical infrastructure.

Closing the web platform would cut off part of the cultural model Myelin is trying to build.

---

## 3. Indexing Moves From Per-Work to Per-Author

Indexing should be primarily an **author-level trust decision**, not a repeated per-publication approval process.

The normal model becomes:

### Standard author

- public work is shareable by URL;
- public work appears in Myelin's internal feed/search/profile surfaces;
- public work is `noindex` externally by default.

### Indexed author

- their public work is externally indexable by default;
- their profile is externally indexable;
- Myelin trusts their judgment enough that each new work does not require pre-approval.

This simplifies the system substantially.

---

## 4. What “Indexed Author” Means

Indexed status should mean:

> **Myelin trusts this person's ongoing judgment enough for their public work to represent the platform on the open web without pre-approval.**

It should not mean:

> **this person once made one good thing**

The designation is therefore about demonstrated practice and judgment across time.

This also strengthens author identity.

“Indexed author on Myelin” has a clear meaning because the trust attaches to the person rather than individual posts.

---

## 5. Indexing Is Not a Launch Prestige Mechanic

At launch, indexing should not be pitched as status.

Prestige does not exist before Myelin has earned it.

Initially, author-level indexing is mainly:

- domain protection;
- quality control;
- a practical trust mechanism;
- a way to ensure the work representing Myelin externally meets the desired standard.

The first invited or founding authors can simply be designated indexed because Myelin deliberately selected them.

Over time, the meaning can evolve naturally:

**curated trust → recognized consistency → reputation → aspiration**

Prestige is a downstream consequence, not something Myelin should pretend already exists.

---

## 6. Publishing Access and Indexing Remain Separate

Payment never purchases indexed status.

Publisher access gives someone the ability to publish.

Indexed status expresses trust.

A person can enter the publishing ecosystem by:

- paying for Publisher access;
- or earning access through meaningful accepted contribution.

Neither route automatically grants indexing.

The cultural model is:

> **publishing is accessible through intent or contribution; indexing is earned through trust**

---

## 7. Per-Work `noindex` Still Exists as an Exception

Although indexing is author-level, individual works still need an exceptional `noindex` override.

Possible reasons include:

- author choice;
- legal concerns;
- copyright issues;
- moderation;
- accidental disclosure;
- compromised accounts;
- temporary or unusual publication states.

This should not become a second hidden per-post approval system.

The default rule stays simple:

> **indexed author + public work = indexable**

unless an explicit exception applies.

---

## 8. Indexed Status Can Be Suspended or Revoked

Indexed status should be trust, not an irrevocable trophy.

Myelin needs the ability to suspend or revoke it if:

- an account is compromised;
- an author begins publishing spam;
- moderation problems become serious;
- trust is materially violated;
- behavior changes enough that the original judgment no longer applies.

Historical authorship and accepted contributions should remain visible.

Indexing status and contribution history are separate concepts.

---

## 9. Coauthored Work Should Not Require Every Contributor to Be Indexed

An indexed author should be able to publish a collaboration with a non-indexed photographer, illustrator, researcher, engineer, or other contributor without losing indexing eligibility.

The canonical publisher / responsible author controls the indexing default.

This avoids making collaboration unnecessarily restrictive.

A transfer of canonical ownership to a non-indexed author can change that default.

---

## 10. Imported Work Needs Its Own Rule

An indexed author importing a large archive of older material should not necessarily cause every imported artifact to become externally indexed immediately.

Bulk imports are a separate operational case from normal publication.

Myelin can apply an import-specific rule without returning to per-post prestige review.

---

## 11. Indexed Status Should Be Understandable but Not Gameable

Users should understand the broad basis of indexed trust.

Possible factors include:

- sustained quality;
- demonstrated judgment;
- meaningful contributions;
- moderation history;
- a body of serious work;
- useful collaboration;
- consistency over time.

Myelin does not need to publish an exact scoring formula.

The standard should be legible without becoming a checklist for gaming the system.

---

## 12. Myelin Is Also a Place to Practice

Myelin is not only a place for finished masterpieces.

It is also a place where people practice becoming better.

A central cultural line is:

> **Myelin is a place to make excellent work, and a place to become capable of making it.**

Not everyone needs to be an indexed author.

Not everything someone makes needs to be a masterpiece.

But serious work needs somewhere to exist while it is still becoming better.

People need somewhere to:

- put work into the world;
- get seen;
- compare themselves with stronger people;
- receive advice;
- collaborate;
- revise;
- try again;
- keep going.

This is a core reason the public feed exists.

---

## 13. The Feed Is a Visible Workshop Floor

The feed should not only represent polished endpoints.

It should show meaningful movement in people's work.

Feed events may include:

- new publications;
- substantial revisions;
- accepted proposals;
- collaborations;
- new versions;
- contributions to other people's work;
- old work becoming materially better.

The feed therefore becomes a visible workshop floor rather than a stream of status updates.

---

## 14. Profiles Need Both a Body of Work and a Trajectory

A person's profile should answer two different questions:

### Body of work

> **What has this person made?**

### Trajectory

> **How did this person get here?**

Most platforms preserve polished endpoints but lose the path that produced them.

Myelin should preserve both.

---

## 15. Author Timelines Should Make the Pursuit Visible

Profiles should contain an actual timeline of meaningful work activity.

Examples:

> Published first exploration of a subject  
> Revised it after reader feedback  
> Contributed a figure to another publication  
> Had a correction accepted  
> Published a second experiment  
> Collaborated with a photographer  
> Reworked an older publication with new evidence  
> Became an indexed author  
> Released a substantially expanded version two years later

This allows someone discovering an exceptional author to go backward and understand how that ability developed.

---

## 16. The Timeline Should Not Become an Activity Log

The timeline should preserve events that say something about the person's work.

It should not be polluted by meaningless social telemetry.

Good timeline events include:

- publish;
- revise;
- contribute;
- collaborate;
- improve;
- create a new version;
- be cited or reused;
- materially help another work;
- receive accepted contribution credit.

Bad timeline events include:

- likes;
- follows;
- passive impressions;
- arbitrary engagement milestones.

The timeline should reveal development, not addiction mechanics.

---

## 17. Visible Trajectory Makes Exceptional People More Useful to Learners

An exceptional person should not appear as though they materialized fully formed.

Readers should be able to see:

- earlier attempts;
- changing interests;
- revisions;
- collaborations;
- accepted corrections;
- experiments;
- mistakes worth preserving;
- increasing sophistication over time.

That makes strong authors more educational and less abstract.

The implied message is:

> **Look at what they can do now. Then look at what they were doing years ago. Keep going.**

---

## 18. Excellence Is Pursued — and Myelin Makes the Pursuit Visible

The motto gains a second product-level statement:

> **Excellence is pursued. Myelin makes the pursuit visible.**

This connects:

- practice;
- public work;
- the feed;
- revisions;
- collaboration;
- proposals;
- contribution history;
- profiles;
- author timelines;
- indexing;
- long-term relevance.

Excellence is not a static badge at the end of a road.

It is visible in the accumulated history of someone continuing to make, revise, learn, and contribute.

---

## 19. Early Community Seeding

Myelin's first serious users cannot be guaranteed.

The early strategy should therefore be direct, personal outreach to people whose existing work genuinely fits the platform.

The invitation should not center on:

> **free software if you post**

It should center on:

> **I saw your work, I think it belongs here, and Myelin can let you do something with it that existing platforms cannot. I am removing the price barrier so you can experiment.**

The free subscription is a friction-removal mechanism, not payment for populating the platform.

The first cohort does not need celebrity.

It needs enough genuinely strong people that a new visitor can immediately see that serious work is happening.

---

## 20. Sustainability Goal

The near-term financial definition of success remains deliberately modest:

> **within six months after launch, Myelin should earn enough to cover its own operating costs so it no longer requires personal subsidy**

This does not require Myelin to become culturally dominant.

It requires a relatively small number of paying users or groups because the system is designed to keep infrastructure costs low.

Public publishing acts primarily as:

- archive;
- discovery;
- distribution;
- reputation;
- community formation.

Recurring private services remain the more durable revenue engine:

- sync;
- private collaboration;
- shared workspaces;
- storage;
- permissions;
- version history;
- recurring group use.

---

## 21. Distilled Architecture

**The online platform remains open source.**

**Production operations remain private where secrecy is operationally necessary.**

**Indexing is granted primarily at the author level.**

**Indexed status means ongoing trust, not one successful post.**

**Payment never buys indexing.**

**Per-work `noindex` exists only as an exception.**

**Indexed status can be suspended or revoked when trust is violated.**

**Collaboration does not require every contributor to be indexed.**

**Myelin is a place for practice as well as exceptional finished work.**

**The feed is a visible workshop floor.**

**Profiles show both a body of work and a trajectory.**

**Timelines preserve meaningful development rather than engagement noise.**

**Exceptional people become more useful when others can see how they developed.**

**Excellence is pursued. Myelin makes the pursuit visible.**
