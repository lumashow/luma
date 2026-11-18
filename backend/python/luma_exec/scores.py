"""luma.scores: your other scores, to read.

`find` lists them and `open` reads one. A reference is read-only: it reads
like `luma.track`, on its own track's bars, and carries its own `venue`, so
every render and filmstrip of a reference shows its rig, not the open one.
Nothing here changes a score.
"""

from __future__ import annotations

import re
from collections import OrderedDict

from .bindings import LumaRecord
from .score import Clip, GraphTrack
from .track import TrackReadOnlyError
from .venue import Venue

_OUTPUTS = ("color", "aim", "strobe", "laser")
# A clip placed once per hit is named for its look plus where it sits:
# "Chant @4.4.1.84", "Green pickup 34.4.3", "Break floor 8".
_PLACE = re.compile(r"[\s@·–-]*\d[\d.\s@·–-]*$")


def _look(name):
    return _PLACE.sub("", name).strip() or name


def _groups(selection):
    expression = str(dict(selection).get("expression", "all"))
    return tuple(part.strip() for part in expression.split("|") if part.strip())


def _output(graph):
    for node in (graph.get("nodes") or {}).values():
        if node.get("kind") in _OUTPUTS:
            return node["kind"]
    return "?"


class Found(tuple):
    """What `find` returned. Index it, and pass an item to `open`. `more`
    says the list was cut."""

    def __new__(cls, rows, more=False):
        found = super().__new__(cls, rows)
        found.more = bool(more)
        return found

    def __repr__(self):
        if not self:
            return "no other scores of yours match"
        lines = [f"{len(self)} score{'' if len(self) == 1 else 's'}"]
        lines += [f"  [{index}] {item}" for index, item in enumerate(self)]
        if self.more:
            lines.append("  more match: narrow the search or raise limit")
        lines.append("open one with luma.scores.open(found[0])")
        return "\n".join(lines)


class FoundScore:
    """One score in a `find` result."""

    __slots__ = ("id", "name", "track_id", "track", "artist", "venue_id", "venue", "clips", "edited")

    def __init__(self, row):
        self.id = row["id"]
        self.name = row.get("name") or ""
        self.track_id = row["track_id"]
        self.track = row.get("title") or ""
        self.artist = row.get("artist") or ""
        self.venue_id = row.get("venue_id") or ""
        self.venue = row.get("venue_name") or ""
        self.clips = int(row.get("clips") or 0)
        self.edited = str(row.get("edited_at") or "")[:10]

    def __repr__(self):
        title = f"{self.track} — {self.artist}" if self.artist else self.track
        name = f' · "{self.name}"' if self.name else ""
        return f"{title} @ {self.venue}{name} · {self.clips} clips · edited {self.edited or 'never'}"


class Reference(GraphTrack):
    """One of your other scores, read-only, with the venue it was written for.

    `looks()` is the place to start: each distinct look, how often it plays,
    on which groups and where first. `roles()` turns that round: what each
    group of that rig is made of and which looks it carries. Beyond those it
    reads as `luma.track` does: `clips`, each clip's `graph` and
    `graph.source()`, `window(...)` with `timeline()`, `output.heatmap()` and
    `filmstrip()`, and `review()`. A clip's graph goes straight into
    `edit.add_clip(clip.graph, name=clip.name, at=clip.at, selection=...)` on
    the open track. Positions are on this score's own track's bars, which are
    the open track's when it is the same song (`same_track`). `venue` is its
    own room: `describe()`, `groups()`, `fixtures` and `render(t=...)`, which
    shows this score on that rig.
    """

    def __init__(self, opened, track, figures):
        score_id = str(opened["id"])

        def read(method, payload=None):
            return track._call("track.scores_read",
                               {"score": score_id, "method": method, "payload": payload})

        super().__init__(opened["track"], nodes=track._nodes, features=opened["features"],
                         host_call=read, artifact_store=track._artifact_store)
        rig = opened["venue"]
        venue = Venue(LumaRecord({key: rig[key] for key in ("id", "name", "views")}, "reference.venue"),
                      host_call=read, figures=figures, workspace=track._artifact_store.workspace)
        for name, value in (("score_id", score_id), ("name", str(opened.get("name") or "")),
                            ("same_track", bool(opened.get("same_track"))), ("venue", venue),
                            ("_rig", tuple(rig.get("groups") or ()))):
            object.__setattr__(self, name, value)

    @property
    def label(self):
        return f"{self.name or self.title} @ {self.venue.name}"

    def edit(self):
        raise TrackReadOnlyError(
            "a reference score is read-only; take what you need from it and write with luma.track.edit()")

    def _by_look(self):
        looks = OrderedDict()
        for id, clip in sorted(self._document["clips"].items(),
                               key=lambda item: (item[1]["start"], item[0])):
            entry = looks.setdefault(_look(clip.get("name", "")), {
                "count": 0, "groups": OrderedDict(), "output": _output(clip.get("graph") or {}),
                "beats": [], "first": clip["start"], "last": clip["start"], "blend": set(), "id": id})
            entry["count"] += 1
            entry["beats"].append(clip["duration"])
            entry["last"] = clip["start"]
            entry["blend"].add(clip.get("blend_mode", "replace"))
            for group in _groups(clip.get("selection", {"expression": "all"})):
                entry["groups"][group] = entry["groups"].get(group, 0) + 1
        return looks

    def looks(self):
        """Each distinct look: clips, output, typical length, groups, span.

        Clips named for one look and a place ("Snare 17.2", "Snare 17.4") are
        one look. `look(name)` gives one of its clips to read.
        """
        lines = [f"{self!r}"]
        for name, entry in self._by_look().items():
            beats = sorted(entry["beats"])[len(entry["beats"]) // 2]
            span = self.bars.format(entry["first"])
            if entry["count"] > 1:
                span += f" to {self.bars.format(entry['last'])}"
            lines.append(
                f"  {name}: {entry['count']}x {entry['output']} {'/'.join(sorted(entry['blend']))}, "
                f"{beats:g} beats, on {' | '.join(entry['groups'])}, {span}")
        return "\n".join(lines)

    def look(self, name):
        """The first clip of a look, by the name `looks()` prints."""
        entry = self._by_look().get(name)
        if entry is None:
            raise KeyError(f"no look named {name!r}; looks() lists them")
        return Clip.read(entry["id"], self._document["clips"][entry["id"]], self.bars)

    def roles(self):
        """What each group of that rig is made of and which looks it carries."""
        carried = {}
        for name, entry in self._by_look().items():
            for group, count in entry["groups"].items():
                carried.setdefault(group, []).append(f"{name} {count}x")
        made = {group["name"]: group for group in self._rig}
        lines = [f"{self.venue.name}: what each group did"]
        for group in list(made) + [g for g in carried if g not in made]:
            info = made.get(group)
            if info is None:
                what = "the whole rig" if group == "all" else "a group that no longer exists"
            else:
                part = f"{info['cells']} cells of " if info["cells"] else ""
                what = f"{part}{info['fixtures']} fixtures ({', '.join(info['models']) or 'nothing'})"
            looks = ", ".join(carried.get(group, [])) or "unused"
            lines.append(f"  {group}: {what}: {looks}")
        return "\n".join(lines)

    def __repr__(self):
        name = f"{self.name!r} · " if self.name else ""
        return (f"<reference {name}{self.title!r} @ {self.venue.name} "
                f"clips={len(self._document['clips'])} read-only; looks(), roles(), clips, window(), venue>")


class Scores:
    """Your other scores. `find()` lists them; `open(found[0])` reads one."""

    def __init__(self, track, figures=None):
        self._track, self._figures = track, figures

    def find(self, text=None, *, track=None, venue=None, uses=None, limit=20):
        """Your scores, this song's first and then the newest.

        `text` is words that must each appear in the score, song, artist or
        venue name. `track` is a track id, such as `luma.track.id` for this
        song on other rigs. `venue` is a venue name or part of one. `uses` is
        part of a clip name, such as a preset's. Filters combine. The score
        you are editing is left out.
        """
        answer = self._track._call("track.scores_find", {
            "text": None if text is None else str(text),
            "track_id": None if track is None else str(getattr(track, "id", track)),
            "venue": None if venue is None else str(venue),
            "uses": None if uses is None else str(uses),
            "limit": int(limit),
        })
        return Found((FoundScore(row) for row in answer["scores"]), answer.get("more", False))

    def open(self, score):
        """Read one score: a `find` item, or a score id."""
        answer = self._track._call("track.scores_open", {"id": str(getattr(score, "id", score))})
        return Reference(answer, self._track, self._figures)

    def __repr__(self):
        return ("<luma.scores: find(text=None, track=None, venue=None, uses=None), "
                "open(score)>")
