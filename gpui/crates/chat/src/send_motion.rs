//! New-turn room and the rise under a sent message. The message is a row of
//! the transcript from the moment it is sent, and the transcript glides it to
//! the head of the view: nothing is drawn over the list.
use std::collections::VecDeque;

use gpui::SharedString;

pub const PEEK: f32 = 48.0;
/// How much faster than a streaming follow the transcript rises under a sent
/// message. At 1.5 the stick spring covers 90% of the way in about 250 ms and
/// lands in about 680 ms — Comet's own-send glide (90% in ~230 ms). Adapted
/// from Comet, MIT, (c) 2026 Wing.
pub const SCROLL_RATE: f32 = 1.5;
pub struct Pending {
    pub id: SharedString,
    pub text: String,
    /// The line under the bubble saying what the editor sent along, as the
    /// accepted row will show it; empty when it sent nothing.
    pub attached: String,
}

#[derive(Default)]
pub struct SendMotion {
    pub pending: VecDeque<Pending>,
    /// The transcript is rising under a message just sent, at
    /// [`SCROLL_RATE`].
    pub glide: bool,
    pub anchor: Option<SharedString>,
    pub anchor_offset: Option<f32>,
    pub room: f32,
    /// [`Self::room`] before it is floored at zero, and the height of the
    /// tool card folding in this exchange when it was measured, if one was —
    /// see [`Self::room_with_fold`].
    slack: f32,
    slack_fold: Option<f32>,
    pub rendered_pending: usize,
    sequence: u64,
}

impl SendMotion {
    pub fn start(&mut self, text: String, attached: String, reduced: bool) {
        self.sequence += 1;
        let id: SharedString = format!("pending-send-{}", self.sequence).into();
        self.anchor = Some(id.clone());
        self.anchor_offset = None;
        self.glide = !reduced;
        self.pending.push_back(Pending { id, text, attached });
    }

    /// Transfer the presentation identity when the backend accepts a prompt.
    pub fn accept(&mut self, id: &str) {
        let Some(pending) = self.pending.pop_front() else {
            return;
        };
        if self.anchor.as_ref() == Some(&pending.id) {
            self.anchor = Some(id.to_string().into());
        }
    }

    /// Measure the room from a finished layout: what the viewport has left
    /// under the current exchange. `fold` is the height of the tool card
    /// folding in this exchange in that layout, if one is.
    pub fn measure(&mut self, viewport: f32, turn_height: f32, fold: Option<f32>) -> f32 {
        self.slack = viewport - PEEK - turn_height;
        self.slack_fold = fold;
        room(viewport, turn_height)
    }

    /// The room for the layout being made, given the folding card's height
    /// in it.
    ///
    /// [`Self::measure`] reads a finished layout, so on its own the room
    /// answers a fold one frame late: a card that shrinks first shortens the
    /// list, which pulls the view down, and only then does the room grow
    /// back. Taking the card's change since the measure keeps the exchange
    /// and its room one height in the same layout. Before a measure has seen
    /// the fold, this is the measured room.
    #[must_use]
    pub fn room_with_fold(&self, fold: f32) -> f32 {
        match self.slack_fold {
            Some(measured) => (self.slack - (fold - measured)).max(0.0),
            None => self.room,
        }
    }
}

/// Reserve only the space the current exchange has not filled yet.
pub fn room(viewport: f32, turn_height: f32) -> f32 {
    (viewport - PEEK - turn_height).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_consumes_reserved_room_before_growing_the_scroll_range() {
        assert_eq!(room(800.0, 100.0), 652.0);
        assert_eq!(room(800.0, 500.0), 252.0);
        assert_eq!(room(800.0, 900.0), 0.0);
    }

    /// While a card folds, the exchange and its room keep one height: what
    /// the card gives up the room takes, and what it grows the room gives, in
    /// the same layout and not one frame later.
    #[test]
    fn a_folding_card_trades_height_with_the_room() {
        let mut state = SendMotion::default();
        let open = 120.0;
        state.room = state.measure(800.0, 300.0 + open, None);
        // The fold has not been measured yet: the room is the measured one.
        assert_eq!(state.room_with_fold(0.0), state.room);
        let room = state.measure(800.0, 300.0 + open, Some(open));
        assert_eq!(state.room_with_fold(open), room);
        for fold in [90.0, 40.0, 0.0] {
            assert_eq!(state.room_with_fold(fold) + fold, room + open);
        }
        // A card that grows past the room's slack cannot make it negative.
        let tight = state.measure(800.0, 700.0, Some(0.0));
        assert!(state.room_with_fold(200.0) >= 0.0);
        assert!(state.room_with_fold(200.0) < tight);
    }

    /// The transcript rises under a sent message faster than it follows a
    /// streaming reply.
    #[test]
    fn the_rise_outpaces_a_streaming_follow() {
        let rise = 600.0;
        let risen = |rate: f32| {
            let mut spring = crate::transcript::StickSpring::default();
            let mut pos = 0.0;
            for _ in 0..12 {
                pos = spring.step(pos, rise, rate);
            }
            pos
        };
        assert!(risen(SCROLL_RATE) > risen(1.0));
        assert!(risen(SCROLL_RATE) <= rise);
    }

    #[test]
    fn rapid_sends_keep_the_latest_anchor_when_earlier_prompts_arrive() {
        let mut state = SendMotion::default();
        state.start("first".into(), String::new(), false);
        state.start("second".into(), String::new(), false);
        state.accept("first-durable");
        assert_eq!(state.anchor.as_deref(), Some("pending-send-2"));
        state.accept("second-durable");
        assert_eq!(state.anchor.as_deref(), Some("second-durable"));
    }
}
