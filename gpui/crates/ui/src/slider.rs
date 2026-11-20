//! The Perform screen's fader: a track its value fills, with a bright edge
//! where the value is. The value box every other number is set with is
//! [`crate::float::scrub`], and the fader drags by the same rules.

use gpui::prelude::FluentBuilder as _;
use gpui::*;

use crate::arg::{drag_fraction, OwnedDrag};
use crate::drag::DragGhost;
use crate::node::{Instrument, Role};
use crate::{glass, ladder, radius};

/// A fader drag in flight, and *which* fader is in it. gpui routes a drag by
/// its payload's type, so every fader hears every fader's drag; the id is how
/// a listener knows the move is its own.
#[derive(Clone)]
struct SliderDrag {
    id: SharedString,
}

impl OwnedDrag for SliderDrag {
    fn owner(&self) -> &SharedString {
        &self.id
    }
}

/// The fader of the Perform screen, live: [`fader_track`] that a drag moves.
/// 0–1, and the same drag rules as [`crate::float::scrub`]: absolute, and
/// only once the pointer moves.
///
/// It has no size of its own: it fills the box its caller gives it, so a
/// tall cell gets a tall fader and a wide one a wide fader.
pub fn luma_fader(
    id: impl Into<SharedString>,
    value: f32,
    axis: Axis,
    tint: Hsla,
    on_change: impl Fn(f32, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let id = id.into();
    let moved = id.clone();
    let vertical = axis == Axis::Vertical;
    fader_track(value, axis, tint)
        // The value, published for a driver, as the slider's readout is.
        .child(div().agent_node(Role::Text, format!("{id} = {:.2}", value.clamp(0., 1.))))
        .id(ElementId::Name(id.clone()))
        .when(vertical, |el| el.cursor_ns_resize())
        .when(!vertical, |el| el.cursor_ew_resize())
        .on_drag(SliderDrag { id }, |_, _, _, cx| {
            cx.stop_propagation();
            cx.new(|_| DragGhost)
        })
        .on_drag_move(drag_fraction(
            moved,
            move |at, _: &SliderDrag, window, cx| {
                on_change(if vertical { 1. - at.y } else { at.x }, window, cx)
            },
        ))
}

/// A fader as it looks, without the drag: a recessed track that the value
/// fills, from the bottom (`Axis::Vertical`) or from the left, in `tint`,
/// with a bright edge where the value is. For a fader shown where a drag
/// means something else, such as a layout being arranged.
pub fn fader_track(value: f32, axis: Axis, tint: Hsla) -> Div {
    let fraction = value.clamp(0., 1.);
    let vertical = axis == Axis::Vertical;
    let fill = div()
        .absolute()
        .bottom_0()
        .left_0()
        .bg(tint.opacity(0.45))
        .when(vertical, |fill| fill.w_full().h(relative(fraction)))
        .when(!vertical, |fill| fill.h_full().w(relative(fraction)));
    // The edge rides the fill's end, inside the track at both ends.
    let edge = div()
        .absolute()
        .bg(ladder::foreground_alpha(0.9))
        .when(vertical, |edge| {
            edge.left_0()
                .w_full()
                .h(px(FADER_EDGE))
                .bottom(relative(fraction))
                .mb(px(-FADER_EDGE * fraction))
        })
        .when(!vertical, |edge| {
            edge.top_0()
                .h_full()
                .w(px(FADER_EDGE))
                .left(relative(fraction))
                .ml(px(-FADER_EDGE * fraction))
        })
        .when(fraction <= 0., |edge| edge.invisible());
    div()
        .relative()
        .overflow_hidden()
        .rounded(px(radius::CONTROL))
        .bg(glass::scrim(0.28))
        .child(fill)
        .child(edge)
}

/// How thick the bright edge at a fader's value is.
const FADER_EDGE: f32 = 2.;

/// How far inside the well a detent fader's fill sits.
const DETENT_INSET: f32 = 2.;
/// The fill of a detent fader at its lowest level: a nub wide enough to keep
/// its corners round and hold the bright edge clear of them.
const DETENT_NUB: f32 = 14.;
/// How far inside the fill's end the bright edge sits, so it never meets a
/// corner at either end of the travel.
const DETENT_EDGE_INSET: f32 = 4.;

/// A horizontal fader for a choice among `count` levels, as it looks: a well
/// with a tick where the edge rests at each level between the ends, a
/// rounded fill in `tint`, and the bright edge just inside the fill's end.
/// `fraction` is where the fill is, which need not be on a level: a drag
/// follows the pointer and settles on a level when it lets go.
///
/// The fill is a nub at 0 and flush with the well's end at 1. Fills the box
/// its caller gives it; [`detent_fraction`] is the pointer's side of the
/// same geometry.
pub fn detent_fader(fraction: f32, count: usize, tint: Hsla) -> Div {
    let fraction = fraction.clamp(0., 1.);
    let steps = count.saturating_sub(1).max(1) as f32;
    let edge_centre = DETENT_EDGE_INSET + FADER_EDGE / 2.;
    div()
        .relative()
        .rounded(px(radius::CONTROL))
        .bg(ladder::foreground_alpha(0.10))
        .child(
            // The fill's travel: its end moves across this box, and its
            // start hangs a nub's width off the left of it.
            div()
                .absolute()
                .left(px(DETENT_INSET + DETENT_NUB))
                .right(px(DETENT_INSET))
                .top(px(DETENT_INSET))
                .bottom(px(DETENT_INSET))
                .children((1..count.saturating_sub(1)).map(|level| {
                    div()
                        .absolute()
                        .left(relative(level as f32 / steps))
                        .ml(px(-edge_centre - 0.5))
                        .top(relative(0.5))
                        .mt(px(-3.))
                        .w(px(1.))
                        .h(px(6.))
                        .bg(ladder::foreground_alpha(0.3))
                }))
                .child(
                    div()
                        .absolute()
                        .left(px(-DETENT_NUB))
                        .right(relative(1. - fraction))
                        .top_0()
                        .bottom_0()
                        .rounded(px(radius::CONTROL - DETENT_INSET))
                        .bg(tint.opacity(0.62))
                        .child(
                            div()
                                .absolute()
                                .right(px(DETENT_EDGE_INSET))
                                .top(px(3.))
                                .bottom(px(3.))
                                .w(px(FADER_EDGE))
                                .rounded(px(FADER_EDGE / 2.))
                                .bg(ladder::foreground_alpha(0.9)),
                        ),
                ),
        )
}

/// Where a pointer `x` across a [`detent_fader`] `width` wide puts its fill:
/// the fraction whose bright edge is under the pointer, so a press on a tick
/// lands on that tick's level, and anything past an end is that end.
#[must_use]
pub fn detent_fraction(x: f32, width: f32) -> f32 {
    let start = DETENT_INSET + DETENT_NUB - DETENT_EDGE_INSET - FADER_EDGE / 2.;
    let travel = width - 2. * DETENT_INSET - DETENT_NUB;
    ((x - start) / travel.max(1.)).clamp(0., 1.)
}

#[cfg(test)]
mod detent_tests {
    use super::{detent_fraction, DETENT_EDGE_INSET, DETENT_INSET, DETENT_NUB, FADER_EDGE};

    /// The pointer and the edge share one geometry: pointing where the edge
    /// rests for a fraction gives that fraction back, and past either end is
    /// that end.
    #[test]
    fn the_pointer_lands_where_the_edge_rests() {
        let width = 232.;
        let travel = width - 2. * DETENT_INSET - DETENT_NUB;
        for step in 0..=10 {
            let fraction = step as f32 / 10.;
            let fill_end = DETENT_INSET + DETENT_NUB + travel * fraction;
            let edge = fill_end - DETENT_EDGE_INSET - FADER_EDGE / 2.;
            assert!((detent_fraction(edge, width) - fraction).abs() < 1e-4);
        }
        assert_eq!(detent_fraction(-20., width), 0.);
        assert_eq!(detent_fraction(0., width), 0.);
        assert_eq!(detent_fraction(width, width), 1.);
        assert_eq!(detent_fraction(width + 40., width), 1.);
    }
}
