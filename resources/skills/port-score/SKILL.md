---
name: port-score
description: Use another of the user's scores as the reference for this one — port a song's show from another venue to this rig, or light this song in the style of a show made elsewhere. How to find the reference, read its intent, see it on its own rig, translate roles for the lights this room has, and check the result against it. Load when the user says port, bring over, copy from another venue, or "like my show at…".
---
# Porting a score

A port is not a copy. The other rig has lights this room does not have, in
places this room does not have them. Keep what the reference was *for*: when
things happen, where the eye goes, how hard it hits, what colour it is.
Rebuild how it does that from the lights that are here.

You read the reference. You write only `luma.track`.

## 1. Find the reference

```python
found = luma.scores.find(track=luma.track.id)     # this song in other venues
found = luma.scores.find("gasworks")              # by track, artist, venue or score name
found = luma.scores.find(venue="gasworks", uses="chase")
ref = luma.scores.open(found[0])
```

Only the user's own scores are listed. If several match and the user's words
do not pick one, ask which. `ref` reads like `luma.track` and has its own
`ref.venue`.

## 2. Compare the rigs

```python
print(ref.roles())                 # each group of that rig: what it is made of, what it carried
print(ref.venue.describe())
print(luma.venue.describe()); print(luma.venue.groups())
```

For each rig, note what it can do: colour, movement, pixels in a row, lasers,
strobes, and where each group sits (front, back, sides, overhead, floor).
Write down what the reference has that this room lacks, and the reverse.

## 3. Read the intent, section by section

Start with the whole show, then go through it a section at a time, 8 or 16
bars each:

```python
print(ref.looks())      # each distinct look: how often, on which groups, where
w = ref.window(bars=(33, 41))
w.timeline()            # which clips, on which layers
w.filmstrip()           # the reference rig through these bars
print(ref.look("Snare").graph.source())   # how one look does it
```

Look at the filmstrip first: it is the show. For each section, write one
line: which sound it follows, where on stage the light is, how bright,
what colour, still or moving, where the hits land. That list is what you
port. `roles()` says which group played each part.

## 4. Translate roles, not groups

Give each role in the reference a carrier in this room. These are design
calls, and they are yours to make:

- **A like-for-like group** (a wash for a wash): reuse the graph,
  `edit.add_clip(clip.graph, name=clip.name, at=clip.at, selection="house_pars")`.
- **A different kind of light**: keep the gesture, rebuild the graph. A laser
  fan that sweeps the room becomes a chase along the pars. A pixel chase on
  LED bars becomes a stepped chase across par positions. Mover sweeps become
  a brightness or colour move across fixed lights in the same direction.
- **No carrier**: fold the role into another group, or drop it. A room of
  pars cannot do everything a laser rig does. Fewer, clearer roles are better
  than every role done badly.
- **Something this room has that the reference lacked**: use it where the
  reference was reaching for more, at the peaks.

A laser clip is bound to the reference venue's zones and does not move as it
is. An aim clip needs lights that move. Load `effect-catalog` for the graph
of the gesture you need.

## 5. Write it

Same track (`ref.same_track`): keep the reference's timings and names.
`at=clip.at` puts a clip on the same bars. Keep relative brightness between sections: the peak here
is where the peak was there.

Different track: the reference's bars mean nothing here. Carry the style
(palette, layering, how it treats a build, a drop and a hit) and place it
against this track's own sections from `luma.music`.

Open the edit, add every clip, and apply once. Do not stop to ask the user to
approve a list of roles: the stage is easier to judge than a table.

## 6. Check against the reference

```python
ref.window(bars=(33, 41)).filmstrip()
edit.window(bars=(33, 41)).filmstrip()
print(ref.review())
print(edit.review())
```

The same bars give the same moments on both rigs. The pictures will not
match, and should not. Check that the light sits on the same side and
height, that the colours agree, and that hits land on the same positions.
The two reviews should rise, peak and go dark over the same bars. Check the
loudest section, the quietest, and one transition, then fix what drifted
before applying.

## 7. Tell the user what you decided

After applying, give a short table: each role in the reference, what
carries it here, and what you dropped and why. Say which sections you
checked.
