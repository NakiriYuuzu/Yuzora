//! How a dialog, a popup or a toast arrives (docs/beta2/motion.md): it fades in and settles a few px into its
//! place, in a named speed of `tokens.json`'s `motion`. Where it arrived from is the only thing that moves; where it
//! rests is where the window always put it. The window keeps one fact per overlay, when it was added; everything
//! else is worked out from that and the clock, so nothing here can be left over from an overlay that has gone.

use crate::window::OverlayKind;
use gpui::{Div, EntityId, Stateful, Styled, Window};
use std::time::{Duration, Instant};

/// How far an arriving overlay is from its place at the first frame, in px (a toast comes up from below, the rest
/// come down from above).
const TRAVEL: f32 = 6.0;

/// Where an overlay is `elapsed` into its arrival: how much of it shows, and how far it still has to go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Phase {
    /// 0 (nothing) to 1 (all of it).
    pub opacity: f32,
    /// Px below (+) or above (-) its place.
    pub dy: f32,
    arriving: bool,
}

/// An overlay that has arrived, or one that never does: exactly as the window has always drawn it.
pub const AT_REST: Phase = Phase {
    opacity: 1.0,
    dy: 0.0,
    arriving: false,
};

impl Phase {
    /// `body` as far along as this phase, and the next frame asked for. Once arrived: `body` itself and no frame, so
    /// nothing but the frames of the arrival differs from before.
    ///
    /// `content` is the view the overlay's mod draws in, which GPUI redraws only when it changed or moved: its quads
    /// would keep the share of fade they were first painted with. It moves in whole px, and the fade does not, so it
    /// is told to redraw with every frame of the arrival, as a mod's own `transition` does.
    pub fn apply(
        self,
        body: Stateful<Div>,
        window: &Window,
        content: Option<EntityId>,
    ) -> Stateful<Div> {
        if !self.arriving {
            return body;
        }
        window.request_animation_frame();
        if let Some(view) = content {
            window.on_next_frame(move |_, cx| cx.notify(view));
        }
        body.opacity(self.opacity).top(crate::settings::z(self.dy))
    }
}

/// The dialogs and popups taken out since the last frame, for the one that takes their place. A dialog that closes and
/// another that opens in the same update (a settings page handing over to a section, a palette to what it ran) is one
/// thing changing to whoever is looking: it goes on from where the first was, so the dim layer never drops out and
/// nothing fades in a second time. The frame that follows empties it, so it holds nothing for a later, new overlay.
#[derive(Default)]
pub struct Handover(Vec<(OverlayKind, Option<Instant>)>);

impl Handover {
    /// An overlay of `kind` that was added at `entered` is gone.
    pub(crate) fn closed(&mut self, kind: OverlayKind, entered: Option<Instant>) {
        if matches!(kind, OverlayKind::Dialog | OverlayKind::Popup) {
            self.0.push((kind, entered));
        }
    }

    /// When a new overlay of `kind` counts as added at `now`: when the overlay it takes the place of was, if one of its
    /// kind has just closed (it takes that one's place only once), else now.
    pub(crate) fn arrives(&mut self, kind: OverlayKind, now: Instant) -> Option<Instant> {
        match self.0.iter().rposition(|(k, _)| *k == kind) {
            Some(i) => self.0.remove(i).1,
            None => Some(now),
        }
    }

    /// A frame has been drawn: what closed before it is no longer just closed.
    pub fn frame_drawn(&mut self) {
        self.0.clear();
    }
}

/// The phase `elapsed` into an arrival of `duration`, `ease`d, from `from` px away. Before the start it is the start;
/// from the end on (and for no duration at all) it is at rest.
pub fn at(elapsed: Duration, duration: Duration, ease: fn(f32) -> f32, from: f32) -> Phase {
    if elapsed >= duration {
        return AT_REST;
    }
    let done = ease(elapsed.as_secs_f32() / duration.as_secs_f32());
    Phase {
        opacity: done,
        dy: from * (1.0 - done),
        arriving: true,
    }
}

fn ease_named(name: &str) -> fn(f32) -> f32 {
    use gpui_base::animation::{ease_in_cubic, ease_in_out_cubic, ease_out_cubic};
    match name {
        "linear" => |t| t,
        "ease-in" => ease_in_cubic,
        "ease-in-out" => ease_in_out_cubic,
        _ => ease_out_cubic,
    }
}

/// The phase of an overlay of `kind` that was added at `entered`, as of `now`. Windows (the dock's drawer, feedback)
/// stay where they are put, and an overlay with no time of arrival (one of blocks and terminals, whose views GPUI
/// draws on their own) does not arrive. A system that asks for less motion has everything at rest.
pub(crate) fn phase(
    kind: OverlayKind,
    entered: Option<Instant>,
    now: Instant,
    reduce_motion: bool,
) -> Phase {
    let Some(entered) = entered else {
        return AT_REST;
    };
    let (speed, from) = match kind {
        OverlayKind::Window => return AT_REST,
        OverlayKind::Toast => ("base", TRAVEL),
        OverlayKind::Dialog | OverlayKind::Popup => ("fast", -TRAVEL),
    };
    if reduce_motion {
        return AT_REST;
    }
    let token = &yuzora_core::tokens::motion()[speed];
    let duration = Duration::from_millis(token["duration"].as_u64().unwrap_or(0));
    let ease = ease_named(token["easing"].as_str().unwrap_or_default());
    at(now.saturating_duration_since(entered), duration, ease, from)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: fn(u64) -> Duration = Duration::from_millis;
    fn out(t: f32) -> f32 {
        gpui_base::animation::ease_out_cubic(t)
    }

    #[test]
    fn it_starts_from_nothing_and_ends_at_rest() {
        let start = at(MS(0), MS(120), out, -6.0);
        assert_eq!((start.opacity, start.dy, start.arriving), (0.0, -6.0, true));
        assert_eq!(at(MS(120), MS(120), out, -6.0), AT_REST, "at the end");
        assert_eq!(at(MS(5000), MS(120), out, -6.0), AT_REST, "long after");
        assert_eq!(
            at(MS(0), MS(0), out, -6.0),
            AT_REST,
            "a speed of no duration is no arrival"
        );
        assert_eq!(at(MS(7), MS(0), out, 6.0), AT_REST);
    }

    #[test]
    fn it_is_the_eased_share_half_way_and_never_leaves_0_to_1() {
        let mid = at(MS(60), MS(120), out, -6.0);
        assert_eq!(mid.opacity, 0.875, "ease-out, cubic: 1 - (1 - 1/2)^3");
        assert_eq!(mid.dy, -0.75, "an eighth of the way left");
        let mut last = (0.0, 6.0_f32);
        for ms in 0..=130 {
            let p = at(MS(ms), MS(120), out, 6.0);
            assert!(
                (0.0..=1.0).contains(&p.opacity) && p.dy >= 0.0 && p.dy <= 6.0,
                "{ms} ms: {p:?}"
            );
            assert!(
                p.opacity >= last.0 && p.dy <= last.1,
                "{ms} ms: it only moves toward rest"
            );
            last = (p.opacity, p.dy);
        }
    }

    #[test]
    fn the_speeds_are_the_named_ones_and_a_toast_comes_from_the_other_side() {
        let t0 = Instant::now();
        let end_of = |kind, speed: &str| {
            let ms = yuzora_core::tokens::motion()[speed]["duration"]
                .as_u64()
                .unwrap();
            (
                phase(kind, Some(t0), t0 + MS(ms - 1), false).arriving,
                phase(kind, Some(t0), t0 + MS(ms), false).arriving,
            )
        };
        for kind in [OverlayKind::Dialog, OverlayKind::Popup] {
            assert_eq!(
                end_of(kind, "fast"),
                (true, false),
                "a dialog and a popup take the fast speed"
            );
        }
        assert_eq!(
            end_of(OverlayKind::Toast, "base"),
            (true, false),
            "a toast takes the base speed"
        );
        assert_eq!(
            phase(OverlayKind::Dialog, Some(t0), t0, false).dy,
            -6.0,
            "a dialog comes down from 6 px above"
        );
        assert_eq!(
            phase(OverlayKind::Popup, Some(t0), t0, false).dy,
            -6.0,
            "so does a popup"
        );
        assert_eq!(
            phase(OverlayKind::Toast, Some(t0), t0, false).dy,
            6.0,
            "a toast comes up from 6 px below"
        );
        let base = yuzora_core::tokens::motion()["base"]["duration"]
            .as_u64()
            .unwrap();
        for kind in [OverlayKind::Dialog, OverlayKind::Popup, OverlayKind::Toast] {
            assert!(
                !phase(kind, Some(t0), t0 + MS(base), false).arriving,
                "no arrival outlasts the base speed"
            );
        }
    }

    #[test]
    fn a_window_a_system_that_asks_for_less_and_a_clock_gone_back_are_handled() {
        let t0 = Instant::now();
        assert_eq!(
            phase(OverlayKind::Window, Some(t0), t0, false),
            AT_REST,
            "windows are put and stay"
        );
        assert_eq!(
            phase(OverlayKind::Dialog, None, t0, false),
            AT_REST,
            "no time of arrival, no arrival"
        );
        for kind in [OverlayKind::Dialog, OverlayKind::Popup, OverlayKind::Toast] {
            assert_eq!(
                phase(kind, Some(t0), t0, true),
                AT_REST,
                "reduced motion: there at once"
            );
            assert_eq!(
                phase(kind, Some(t0 + MS(500)), t0, false),
                phase(kind, Some(t0), t0, false),
                "now before it was added: its start"
            );
        }
    }

    #[test]
    fn a_closed_dialog_hands_its_time_on_once_and_only_until_the_next_frame() {
        let (t0, now) = (Instant::now(), Instant::now() + MS(900));
        let mut h = Handover::default();
        h.closed(OverlayKind::Dialog, Some(t0));
        h.closed(OverlayKind::Popup, None);
        assert_eq!(
            h.arrives(OverlayKind::Dialog, now),
            Some(t0),
            "goes on from the dialog it replaces"
        );
        assert_eq!(
            h.arrives(OverlayKind::Dialog, now),
            Some(now),
            "a second one is new: a closed overlay is replaced once"
        );
        assert_eq!(
            h.arrives(OverlayKind::Popup, now),
            None,
            "one of blocks had no arrival, and neither has what replaces it"
        );
        h.closed(OverlayKind::Dialog, Some(t0));
        h.frame_drawn();
        assert_eq!(
            h.arrives(OverlayKind::Dialog, now),
            Some(now),
            "after a frame it is new"
        );
        h.closed(OverlayKind::Dialog, Some(t0));
        assert_eq!(
            h.arrives(OverlayKind::Popup, now),
            Some(now),
            "a popup does not take the place of a dialog"
        );
    }

    #[test]
    fn a_toast_and_a_window_are_never_replaced() {
        let (t0, now) = (Instant::now(), Instant::now() + MS(900));
        let mut h = Handover::default();
        h.closed(OverlayKind::Toast, Some(t0));
        h.closed(OverlayKind::Window, Some(t0));
        assert_eq!(h.arrives(OverlayKind::Toast, now), Some(now));
        assert_eq!(h.arrives(OverlayKind::Window, now), Some(now));
    }
}
