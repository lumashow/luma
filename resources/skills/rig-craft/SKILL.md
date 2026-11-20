---
name: rig-craft
description: What different rigs can say. Match effects to the geometry and instruments actually hanging — dense axes speak granular motion, sparse axes speak states. Read before authoring motion on an unfamiliar venue.
---
# Rig craft

A rig is an instrument, and every rig plays some things beautifully and some
things not at all. Half of design is knowing which is which *before* you
author. Study `luma.venue` — positions, groups, fixture kinds — and build a
mental model of what this rig can articulate. Author to its strengths; the
same idea forced onto the wrong rig reads as a glitch.

## Geometry decides the vocabulary

Where the lights are dense, motion can be granular; where sparse, speak in
states. Concrete shapes:

- **Stacked verticals** (LED bars on towers, crates, columns): granular
  up/down is the superpower — rising sweeps, ceiling-to-floor slams, gravity
  play. If the stacks also spread horizontally, left/right works too but
  coarser — waves across stacks, not smooth chases. Author vertical fine,
  horizontal broad.
- **A horizontal line** (bars along one truss, strip on a shelf): chases and
  wipes are the native tongue. Center-out mirroring doubles the vocabulary
  (`mirror`), and `u` runs along the line no matter how the room is rotated.
- **A grid or ceiling array**: 2D territory — radial blooms, diagonal wipes,
  rain. The rare rig where spatial patterns are the show.
- **Sparse pars** (a handful of cans): no motion to speak of — alternation,
  color states, and group contrast ARE the vocabulary. Two groups of three
  pars alternating reads clean; a "chase" across five scattered cans reads as
  malfunction.
- **Fixtures with heads** (multi-head bars, pixel fixtures): sub-fixture
  detail exists — use it for texture (shimmer, scan) while whole fixtures
  carry the structure. A whole group held at one flat color looks unfinished
  on these; give the cells a shape, a gradient or a share that is dark.
- **A ring around the room** (bars on the walls): rotation is the native
  motion. Check whether a shape travels across cells or only across
  fixtures: cells that sit at nearly the same place along the path light as
  one block.

- **In the round** (a ring truss, the DJ in the middle, crowd on every
  side): there is no downstage. Anything that travels front to back is
  lighting a stage that is not there, and half the room watches it leave.
  Light it like a kaleidoscope: the same thing on every side at once,
  rotating, pulsing out from the centre and back.

Use `u`/`v` (rig-intrinsic axes) rather than world x/y/z when authoring —
patterns written on `u` survive any venue's rotation and layout.

## Instruments have roles

Like an orchestra, not interchangeable:

- **Wash/pars** — atmosphere and color. The strings: the foundation when a
  section needs one. Many do not; a wash left on under everything lifts the
  floor and flattens every hit above it (see contrast-and-darkness).
- **LED bars/pixels** — motion and texture. The rhythm section: chases,
  shimmer, gradients.
- **Strobes** — the hardest thing the rig can do to a body. They belong to
  the sounds that do the same: a sub that shakes the floor, a growl, a noise
  sweep at its top. How often is the music's call, not a quota. A relentless
  drop can live in them; a gentle track never earns one.
- **Movers/beams** — the soloists, when the venue has them: the only lights
  that reach across the room. Load `movers` before the first clip on them.
- **Blinders/audience-facing warm** — connection: the singalong chorus, the
  hands-up moment. Sparingly; the crowd blinded is the crowd removed.

A rig missing an instrument means substituting, not forcing: no strobes → the
punch comes from a full-rig white pop of one frame; no movers → the "aerial"
moment becomes the whole rig breathing in sync; no washes → bars at low
saturation carry the atmosphere.

## Fit check

Before authoring a section, ask: which axis carries this idea, and is that
axis dense enough here? What instrument plays the accent, and does this venue
have it? Preview on the heatmap: if a motion idea needs squinting to see in
the heatmap's spatial order, the room won't see it either — replace it with a
state change the rig can actually pronounce.
