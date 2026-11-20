---
name: movers
description: Moving heads, beams, spots and moving washes — where they point, when they are lit, and why. Read before the first clip on any fixture that pans and tilts.
---
# Movers

A par lights a patch of floor. A mover is the only light in the rig that
reaches out of itself: it draws a line through the air over people's heads,
and the line can swing, cross another, or come down on them. That reach is
the whole reason it is hanging there. Use it for that, or leave it dark.

## A beam is an event

Beams that are always on become furniture. Ten minutes in, nobody looks up.
A beam that was dark and is suddenly there splits the room, and every head
turns.

The housings are painted black so nobody sees them think. Move in the dark.
Arrive lit. Set the aim while the light is off, then open on the sound.

A beam lit before anything aimed it points wherever the fixture rests, and a
mover standing on the floor rests pointing straight up, so the show opens
with a row of dots on the ceiling. Give every lit beam an aim from the bar
it lights.

A lit beam rocking slowly from side to side is what a mover does when nobody
told it anything. The room reads it as a machine idling. If a beam is on, it
is on because a sound put it there: the first bar of the drop, the phrase of
the lead, the last two bars of a riser. Then it goes away again. The less
they are lit, the more the room looks up when they are.

## Where they point

Every beam leaning stage left is a row of lamps that all turned their heads.
Nothing meets anything, so there is nothing to watch, and the next thing it
can do is lean stage right. A shape needs beams that differ from each other.

- **Mirror.** The two sides of the rig reach for each other, or open away.
  Crossing beams are the picture people remember from a show. One input does
  it: a head that `mirror` folds takes the opposite yaw.
- **Fan.** Yaw spread along the row. Narrow is a blade. Wide is a crown over
  the crowd.
- **Blossom.** A fan that opens: closed on the breath before, wide on the
  hit. The opening is the event.
- **Phase.** One motion, each head a little later, so a wave runs down the
  row. A motion every head makes at once is one big lamp; the same motion
  passed from head to head is something the eye can follow.
- **Still.** Straight down is a cage of pillars. Straight up is a wall of
  light behind the DJ. A relentless sub can want beams that hold one shape
  and let level do the violence. Stillness is a choice for a section, made
  against the motion around it; a mover that never moves all show is a par
  that cost more.

Choose by the sound. A hard stab: snap between two shapes in the dark and
arrive on the hit. A swelling pad: a slow blossom. A riser: pitch climbing
for exactly as long as the riser does. A glittering arp: small fast circles,
specks of light flying round the room. A sound that pans: a wave that
travels the same way.

```python
D = (0, 0.5, -0.87)          # out over the crowd and down

cross = aim(heads=mirror(), direction=D, yaw=30)

fan = aim(direction=D, yaw=curve(space(), "Ramp up", low=-35, high=35))

opening = curve(time(), [[0, 0, "ease-out"], [1, 1]], low=0, high=40)
blossom = aim(heads=mirror(), direction=D, yaw=opening)

lag = curve(space(), "Ramp up", high=216)
nod = curve(time(every=4, phase=lag), "Sine", low=-20, high=20)
wave = aim(direction=D, pitch=nod)      # place with blend="offset"
```

`effect-catalog` has more: Scissor, Circle, Figure-8, Pinwheel, Ballyhoo.

## Light across the beams

Once the beams hold a shape, the best thing left is to play light over it. A
chase that runs down a row of crossing beams, then turns round on the next
phrase, is worth more than any amount of the beams moving. The beams stay
where they are; a second clip on `multiply` decides which are lit.

```python
edit.add_clip(color(color="#ff2010"), name="Beams", at=drop, selection="beams")

run = space(shift=curve(time(every=2), "Ramp up", low=-0.3, high=1), scale=0.3)
edit.add_clip(color(brightness=curve(run, "Soft")), name="Run down the beams",
              at=drop, selection="beams", blend="multiply")
```

The same goes for a punchy kick: one clip on `subtract` over everything
ducks the room on each hit, and the beams breathe with the drums without a
single aim changing.

## What a beam lands on

Look at where each one ends. A beam that stops on the truss a metre in front
of it, on the back of the stage, or on the wall beside it is a lit piece of
metal and no beam at all. Render every shape you hold, from the audience and
from overhead, and aim it over the crowd, across the room or into the air.

## Jumps

An aim that jumps while the light is on is a fixture whipping across the
room in full view. There are three honest ways from one shape to the next.
The travel is the effect, and it takes as long as the sound it follows. Or
it happens in the dark. Or, when where the beam goes on the way does not
matter, the aim fades: two Replace aim clips overlap and the later one comes
in with `alpha=curve(time(), "Fade in")`, so the beams glide across instead
of snapping.

## Moving washes and a centrepiece

Moving washes follow the same reasons with softer edges: they sway and
bloom where beams cut. A row of them coming on together and rocking in step
is the idling machine again; give them a mirror or a phase.

A fixture that hangs alone in the middle of the room and can pan and tilt is
the best trick the venue has. A show that never turns it has left the room's
one surprise in its case.
