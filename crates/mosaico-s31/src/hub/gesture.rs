//! System gestures that start at the screen edges (RFC 0006 section 1).
//!
//! The fox reacts to touches anywhere on the glass, and app pages have their
//! own controls, so the shell only claims touches that *start* in an edge
//! band. Everything else passes through untouched, sample for sample.
//!
//! - Bottom edge, upward: [`SystemGesture::Up`] (home <-> launcher).
//! - Left edge, rightward: [`SystemGesture::Back`].
//!
//! A touch that starts in a band belongs to the shell until it lifts, whether
//! or not it turns into a gesture: delivering half a touch to the fox or to
//! LVGL would leave them with a press and no release, or a release and no
//! press.

use crate::behavior::personality::PointerSample;

/// Touches that start at or below this row belong to the bottom band. The fox
/// canvas ends at row 424, so the band is below the fox on the home page.
pub const BOTTOM_BAND_Y: i32 = 420;
/// Touches that start at or left of this column belong to the left band.
pub const LEFT_BAND_X: i32 = 28;
/// Travel along the gesture axis before it fires, px.
pub const SWIPE_MIN_PX: i32 = 80;
/// A swipe slower than this from press to threshold is ignored, ms.
pub const SWIPE_MAX_MS: u64 = 1000;

/// A recognized system gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemGesture {
    /// Swipe up from the bottom edge.
    Up,
    /// Swipe right from the left edge.
    Back,
}

/// What the caller does with one pointer sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// Deliver the sample to the page (the fox or LVGL).
    Pass,
    /// The shell holds this touch; drop the sample.
    Held,
    /// The shell holds this touch and it just became a gesture.
    Gesture(SystemGesture),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    Bottom,
    Left,
}

#[derive(Clone, Copy, Debug)]
enum Contact {
    None,
    Pass,
    Edge {
        edge: Edge,
        x0: i32,
        y0: i32,
        t0: u64,
        fired: bool,
    },
}

/// Edge-gesture recognizer over the raw pointer stream.
#[derive(Debug)]
pub struct EdgeGestures {
    contact: Contact,
}

impl Default for EdgeGestures {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgeGestures {
    pub const fn new() -> Self {
        Self {
            contact: Contact::None,
        }
    }

    /// Whether the shell currently holds a touch.
    pub fn holding(&self) -> bool {
        matches!(self.contact, Contact::Edge { .. })
    }

    /// Route one sample. Samples must arrive in order.
    pub fn feed(&mut self, sample: PointerSample) -> Route {
        match self.contact {
            Contact::None => {
                if !sample.pressed {
                    // A stray release (for example the tail of a touch that
                    // began before a page switch) goes to the page, which
                    // ignores releases it never saw pressed.
                    return Route::Pass;
                }
                let edge = if sample.y >= BOTTOM_BAND_Y {
                    Some(Edge::Bottom)
                } else if sample.x <= LEFT_BAND_X {
                    Some(Edge::Left)
                } else {
                    None
                };
                match edge {
                    Some(edge) => {
                        self.contact = Contact::Edge {
                            edge,
                            x0: sample.x,
                            y0: sample.y,
                            t0: sample.at_ms,
                            fired: false,
                        };
                        Route::Held
                    }
                    None => {
                        self.contact = Contact::Pass;
                        Route::Pass
                    }
                }
            }
            Contact::Pass => {
                if !sample.pressed {
                    self.contact = Contact::None;
                }
                Route::Pass
            }
            Contact::Edge {
                edge,
                x0,
                y0,
                t0,
                fired,
            } => {
                if !sample.pressed {
                    self.contact = Contact::None;
                    return Route::Held;
                }
                if fired || sample.at_ms.saturating_sub(t0) > SWIPE_MAX_MS {
                    return Route::Held;
                }
                let dx = sample.x - x0;
                let dy = sample.y - y0;
                let gesture = match edge {
                    // Mostly vertical: |dy| at least 1.5 x |dx|.
                    Edge::Bottom if -dy >= SWIPE_MIN_PX && 2 * (-dy) >= 3 * dx.abs() => {
                        Some(SystemGesture::Up)
                    }
                    Edge::Left if dx >= SWIPE_MIN_PX && 2 * dx >= 3 * dy.abs() => {
                        Some(SystemGesture::Back)
                    }
                    _ => None,
                };
                match gesture {
                    Some(gesture) => {
                        self.contact = Contact::Edge {
                            edge,
                            x0,
                            y0,
                            t0,
                            fired: true,
                        };
                        Route::Gesture(gesture)
                    }
                    None => Route::Held,
                }
            }
        }
    }

    /// Forget the current touch (the page under it is going away).
    ///
    /// A pass-through touch keeps passing until it lifts so the new page sees
    /// its release; an edge touch stays held.
    pub fn reset_pass(&mut self) {
        if matches!(self.contact, Contact::Pass) {
            self.contact = Contact::None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(at_ms: u64, x: i32, y: i32, pressed: bool) -> PointerSample {
        PointerSample {
            at_ms,
            x,
            y,
            pressed,
        }
    }

    fn run(g: &mut EdgeGestures, samples: &[PointerSample]) -> Vec<Route> {
        samples.iter().map(|&sample| g.feed(sample)).collect()
    }

    #[test]
    fn a_touch_in_the_middle_passes_through_untouched() {
        let mut g = EdgeGestures::new();
        let routes = run(
            &mut g,
            &[s(0, 240, 240, true), s(20, 240, 100, true), s(40, 240, 100, false)],
        );
        assert_eq!(routes, [Route::Pass; 3]);
        assert!(!g.holding());
    }

    #[test]
    fn a_swipe_up_from_the_bottom_fires_once() {
        let mut g = EdgeGestures::new();
        let routes = run(
            &mut g,
            &[
                s(0, 240, 460, true),
                s(30, 242, 420, true),
                s(60, 245, 370, true),
                s(90, 246, 300, true),
                s(120, 246, 300, false),
            ],
        );
        assert_eq!(
            routes,
            [
                Route::Held,
                Route::Held,
                Route::Gesture(SystemGesture::Up),
                Route::Held,
                Route::Held
            ]
        );
        assert!(!g.holding());
    }

    #[test]
    fn a_swipe_right_from_the_left_edge_is_back() {
        let mut g = EdgeGestures::new();
        let routes = run(
            &mut g,
            &[s(0, 10, 200, true), s(40, 100, 210, true), s(80, 100, 210, false)],
        );
        assert_eq!(
            routes,
            [Route::Held, Route::Gesture(SystemGesture::Back), Route::Held]
        );
    }

    #[test]
    fn an_edge_tap_is_swallowed_without_a_gesture() {
        let mut g = EdgeGestures::new();
        let routes = run(&mut g, &[s(0, 240, 470, true), s(80, 240, 470, false)]);
        assert_eq!(routes, [Route::Held, Route::Held]);
    }

    #[test]
    fn a_diagonal_or_wrong_way_drag_does_not_fire() {
        let mut g = EdgeGestures::new();
        // From the bottom band, mostly sideways.
        let routes = run(
            &mut g,
            &[s(0, 100, 460, true), s(40, 300, 360, true), s(80, 300, 360, false)],
        );
        assert!(!routes.contains(&Route::Gesture(SystemGesture::Up)));
        // From the left band, upward.
        let routes = run(
            &mut g,
            &[s(200, 10, 300, true), s(240, 40, 100, true), s(280, 40, 100, false)],
        );
        assert!(!routes.iter().any(|r| matches!(r, Route::Gesture(_))));
    }

    #[test]
    fn a_slow_drag_does_not_fire() {
        let mut g = EdgeGestures::new();
        let routes = run(
            &mut g,
            &[
                s(0, 240, 460, true),
                s(SWIPE_MAX_MS + 1, 240, 300, true),
                s(SWIPE_MAX_MS + 20, 240, 300, false),
            ],
        );
        assert_eq!(routes, [Route::Held; 3]);
    }

    #[test]
    fn a_drag_into_the_band_is_not_a_gesture() {
        // Starts in the middle, so it belongs to the page even when it
        // crosses the bottom band.
        let mut g = EdgeGestures::new();
        let routes = run(
            &mut g,
            &[s(0, 240, 300, true), s(30, 240, 470, true), s(60, 240, 300, true), s(90, 240, 300, false)],
        );
        assert_eq!(routes, [Route::Pass; 4]);
    }

    #[test]
    fn two_gestures_in_a_row_both_fire() {
        let mut g = EdgeGestures::new();
        let first = run(&mut g, &[s(0, 240, 460, true), s(40, 240, 340, true), s(60, 240, 340, false)]);
        let second = run(&mut g, &[s(200, 5, 240, true), s(240, 120, 240, true), s(260, 120, 240, false)]);
        assert_eq!(first[1], Route::Gesture(SystemGesture::Up));
        assert_eq!(second[1], Route::Gesture(SystemGesture::Back));
    }

    #[test]
    fn a_stray_release_passes() {
        let mut g = EdgeGestures::new();
        assert_eq!(g.feed(s(0, 240, 240, false)), Route::Pass);
    }
}
