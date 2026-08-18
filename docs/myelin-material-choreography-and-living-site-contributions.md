# Myelin — Material, Choreography & Living-Site Contribution Model

*Continuation of `myelin-permanent-publishing-and-sustainability-model.md`.*

This document records only the new plans discussed afterward.

---

## 1. From `Doc | Notes` to `Material | Notes`

The current Myelin workspace is built around a document and notes shown side by side, with either side also viewable independently.

That model can be generalized without creating separate product modes for every discipline.

The stronger abstraction is:

> **Material | Notes**

The material side can support different kinds of source objects while the notes side remains the place for interpretation, explanation, context, and linked thinking.

Possible material types include PDFs and documents, photographs, illustration boards, visual series, diagrams, datasets, code/output, maps, video, 3D objects, simulations, and other structured media.

This preserves Myelin's core relationship between source and interpretation while allowing different disciplines to work in their native medium.

---

## 2. Structured Visual Canvas

For visual creators, the material pane can become a structured visual canvas rather than a generic blank infinite canvas.

The goal is to support spatial freedom without turning Myelin into Figma or Miro.

The canvas should understand the objects placed within it, for example:

- image;
- image sequence;
- spread;
- contact sheet;
- figure;
- text fragment;
- reference;
- dataset/plot;
- annotation region.

This lets photographers and illustrators work visually while still allowing Myelin to understand sequence, grouping, attribution, relationships, and publication structure.

The desktop canvas is a working and arrangement environment.

The public site does not need to reproduce that canvas literally. Myelin can translate the structured relationships into a polished responsive publication composition.

---

## 3. Annotations Become Authored Presentation States

Traditional annotation usually does one of two things:

- quotes or highlights the referenced content;
- jumps or scrolls the reader to the referenced location.

Myelin can make annotations substantially more interactive.

An author can define not only **what** part of the source is being referenced, but **how the reader should encounter it**.

For a painting, an authored annotation might zoom into a particular region, pan to a detail, crop the visible area, mirror the image, flip it, rotate it, dim surrounding regions, reveal an overlay, compare two regions, compare two works, reveal or hide a layer, or return to the original view.

This turns annotation from a pointer into an authored presentation state.

---

## 4. Authors Can Choreograph Attention

The important concept is:

> **Authors can choreograph the reader's attention.**

A phrase in the author's prose can trigger an intentional view of the source.

For example:

**"Notice the figure in the doorway."**  
→ zoom to the doorway.

**"The composition becomes clearer when mirrored."**  
→ mirror the work and show the changed state clearly.

**"Compare this gesture with the figure opposite it."**  
→ create a comparison view.

The author intentionally defines these states while writing.

The reader does not need to manually search the source for the detail being discussed.

---

## 5. Choreography Should Be No-Code

The author should never need to write JSON-LD, selectors, coordinate objects, or another technical representation.

The authoring process should be direct manipulation.

A possible workflow:

1. Highlight or select the phrase that should control the source.
2. Choose **Link to material**.
3. Manipulate the material directly into the desired state.
4. Apply optional transformations or overlays.
5. Save the view.

Myelin stores the structured representation underneath.

The author only needs to think:

> **Show the reader this, in this way.**

The technical representation should remain invisible.

---

## 6. Reusable Authored Views

A material object can have reusable named views such as:

- Full work;
- Doorway detail;
- Central figure;
- Mirrored composition;
- X-ray comparison;
- Layer view;
- Benchmark detail;
- Selected rows.

A writer can reference these views repeatedly without rebuilding them.

In edit mode, a prose link could appear as a small visual token representing the saved view.

The author can preview, duplicate, modify, or move that relationship directly from the text.

---

## 7. The Same Primitive Generalizes Across Media

The choreography model should not be image-specific.

The same underlying concept can apply to many material types.

### Image
Viewport + zoom + transformations + overlays.

### PDF
Page + region + highlighted passage + visual focus.

### Map
Bounds + layers + markers + comparison state.

### Video
Timestamp/range + crop + zoom + annotation.

### Dataset
Filter + sort + selected rows + chart state.

### 3D model
Camera + visibility + selected parts + exploded view.

### Diagram
Focused nodes + highlighted edges + revealed layers.

### Simulation
Parameters + state + selected output.

The general primitive is:

> **author-defined view state of material**

This is more powerful than "zoomable annotations" because it becomes a general authoring language for guided explanation.

---

## 8. Provenance and Transformation Clarity

Any transformation of source material must remain visibly distinguishable from the original.

If an author mirrors, crops, enhances, rotates, recolors, overlays, or otherwise modifies the presentation, Myelin should make that state clear.

Possible labels:

- Original;
- Author view — mirrored;
- Author overlay;
- Detail view;
- Comparison view.

Readers should always be able to return to the original material.

This protects scholarly trust and prevents a presentation transformation from being mistaken for the source itself.

---

## 9. Myelin Is No Longer Well Described as a Notes App

"Notes" still describes one important private object, but it no longer describes the product as a whole.

The product is becoming a system where people can work with source material, interpret it, structure it, collaborate around it, and publish it in richer forms.

A simple public description that emerged is:

> **Myelin is where serious work becomes interactive, collaborative, and publishable.**

The word **becomes** matters.

Myelin does not claim to create the original work for the author.

It changes what can happen to work that already exists.

---

## 10. Contributor Discovery Needs Its Own Mechanism

The contributor model cannot require non-paying creators to publish on Myelin before they are discoverable.

That would create an unfair loophole where visual artists publish for free while writers pay.

The stronger solution is to separate **discovery** from **publishing**.

Two new mechanisms solve this:

- Creator Cards;
- Myelin Commons.

---

## 11. Creator Cards

A Creator Card is a lightweight discoverability object, not a publication.

It can contain:

- name;
- discipline;
- short bio;
- external portfolio links;
- selected preview images or examples;
- collaboration availability;
- areas of interest.

It does not grant free public publishing, unrestricted hosting, main-feed publishing privileges, or automatic indexing.

The purpose is simply to let authors discover people whose work may fit a publication.

A Creator Card can therefore exist before someone becomes a Myelin contributor.

---

## 12. Myelin Commons

Myelin Commons is a curated pool of existing work offered for potential reuse or collaboration.

Possible Commons material includes:

- photography;
- illustrations;
- diagrams;
- datasets;
- templates;
- teaching material;
- visual assets;
- other reusable existing work.

The Commons is not a free publishing feed.

Its purpose is:

> **make existing high-quality work discoverable for meaningful reuse**

Work admitted to the Commons can be curated rather than automatically accepted.

This preserves quality while allowing creators to participate before becoming full publishers.

---

## 13. The Website Itself Can Be a Contribution Surface

Myelin should treat visual and cultural contributions as seriously as code contributions.

Some contributions may affect the product or website at a much larger scale.

Examples include typography systems, photography, illustration, homepage visual direction, documentation art direction, motion treatments, seasonal visual interventions, experimental layouts, release-page design, and community-page design.

These contributions do not need to alter the core interaction system.

They can operate on surfaces that are safe to evolve frequently.

---

## 14. Stable Core, Living Surface

The website can be divided conceptually into two layers.

### Stable Core

These should remain consistent and carefully controlled:

- navigation behavior;
- annotation interactions;
- publication grammar;
- accessibility;
- attribution conventions;
- performance requirements;
- core layouts;
- interaction affordances;
- important brand cues.

### Living Surface

These can evolve much more freely:

- homepage compositions;
- documentation landing pages;
- release pages;
- community pages;
- visual campaigns;
- photography;
- illustration;
- typography experiments;
- motion;
- temporary art direction.

The principle is:

> **contributors can change the expression without destroying the grammar.**

---

## 15. The Graffiti-Wall Model

A useful metaphor is a public wall specifically made for graffiti:

- the structure remains;
- artists are invited to make the surface their own;
- the work is documented and archived;
- the wall is repainted;
- new work appears.

Myelin can apply the same concept digitally.

A visual contribution may exist temporarily on the live site, but its version is preserved permanently in an archive.

For example:

> **Myelin Web — Week 31, 2027**  
> Visual direction by X  
> Photography by Y  
> Typography by Z

The temporary nature of the live surface does not make the contribution disposable.

It becomes part of Myelin's visible history.

---

## 16. Archived Website Iterations

Past visual versions of Myelin should remain browsable.

Someone could inspect previous homepage designs, past documentation art direction, earlier photography, temporary typography systems, release-specific visual treatments, and experimental contributions.

Each archived iteration should credit the people involved and link to their contributor profiles.

The website itself therefore becomes a historical artifact of the community.

---

## 17. Contributor Profiles Can Show Living-Site Work

Contributor profiles should record these broader contributions alongside more conventional ones.

A photographer might show:

- Commons photography;
- work used in indexed publications;
- homepage visual contribution;
- publication collaborations.

A typographer might show:

- Myelin Web typography contribution;
- mathematical typography improvements;
- publication collaborations.

A developer might show:

- indexing improvements;
- canvas annotation renderer;
- accessibility work;
- release contributions.

This places code, art, research, design, and other serious work on the same contribution continuum.

---

## 18. Myelin as a Living Gallery

The goal is for people to have reasons to visit Myelin even when they are not currently looking for an article.

Someone interested in web design, typography, photography, illustration, interaction design, open-source culture, or technical systems may visit simply to see what the site and community are currently making.

That gives Myelin a cultural surface in addition to its research and publishing surface.

The project becomes alive rather than visually frozen.

---

## 19. Open Source as a Cultural Model

The larger principle is:

> **Open source should describe more than who can modify the code.**

Myelin can be collaborative in all of its forms.

Programmers can change the system.

Designers can change how parts of it are expressed.

Photographers can shape its visual culture.

Illustrators can build new explanatory languages.

Typographers can improve how knowledge is read.

Researchers can contribute serious work.

Teachers can contribute teaching structures.

The website itself becomes one of the things the community collectively creates.

---

## 20. Distilled Direction

**Myelin works with material, not just documents.**

**Visual creators can use a structured canvas without forcing their work into a text-first model.**

**Annotations can define authored view states, not just static references.**

**Authors can choreograph how readers encounter evidence and material.**

**The choreography system should be no-code and generalize across many media types.**

**Creator Cards solve discovery without giving away free publishing.**

**Myelin Commons creates a curated pool of reusable existing work.**

**The core product remains stable while selected public surfaces can evolve through community contributions.**

**Temporary visual contributions are archived permanently.**

**Contributor profiles record code, research, design, photography, illustration, typography, and other forms of participation.**

**The website itself becomes a living collaborative gallery rather than a static shell around the product.**
