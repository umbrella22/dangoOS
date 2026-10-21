//! Grok-ball first screen for the R1 device pipeline.
//!
//! The mounted tree has two phases:
//!
//! 1. **Boot splash** — the `ikaros` wordmark on a 320x120 canvas, ~1.4 s.
//! 2. **Ball** — a full-screen black field plus the 240x240 [`BallWidget`].
//!
//! The phase is application state, so `App::view` returns one or the other and
//! the reconciler swaps the subtree. The sway is keyed on the phase name, which
//! forces exactly one create/delete per widget and no remount on a tap or a
//! later `App::view`.
//!
//! # Touch routing
//!
//! LVGL events are not the source of truth for the ball. `ui_task` drains the
//! raw pointer stream into [`BallRuntime`], which owns the tap ladder, the grab
//! and the drag. The only LVGL handler here is on the splash, where any press
//! skips the wordmark early.
//!
//! # Motion
//!
//! The ball does not appear at its final position: after the wordmark fades,
//! the ball drops in from above the panel and lands with a squash. Touch is
//! held off for the duration (see [`crate::ball_widget::BallControl::start_entrance`]
//! and [`BallRuntime::ignore_touch_until`]).

use alloc::rc::Rc;

use grok_ball::skin;
use grok_ball::types::BallExpression;
use lvgl_ui::{App, Color, Key, Node, Style, Theme};

use mosaico_s31::ball_widget::{BallControl, BallWidget};
use mosaico_s31::boot_splash;
use mosaico_s31::entrance;
use mosaico_s31::splash_widget::SplashWidget;

/// The six AI states, in tap order.
///
/// The host preview and platforms without the personality runtime use this
/// ring; the device tap path goes through [`mosaico_s31::runtime::BallRuntime`].
pub const AI_STATES: [BallExpression; 6] = [
    BallExpression::Idle,
    BallExpression::Listening,
    BallExpression::Thinking,
    BallExpression::Speaking,
    BallExpression::Happy,
    BallExpression::Error,
];

/// The next AI state after `current`.
pub fn next_ai_state(current: BallExpression) -> BallExpression {
    match AI_STATES.iter().position(|&state| state == current) {
        Some(index) => AI_STATES[(index + 1) % AI_STATES.len()],
        None => BallExpression::Idle,
    }
}

/// Which screen is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// The `ikaros` wordmark is playing.
    Splash,
    /// The ball is on screen (possibly still dropping in).
    Ball,
}

/// Whether the custom `ikaros` wordmark plays before the ball.
///
/// The factory bootloader already shows the Mosaico wordmark before `main_app`
/// starts; that image cannot be removed without rewriting the bootloader, so
/// the product decision is to skip our own wordmark and go straight to the
/// ball. Keep the splash implementation and set this to `true` to restore it.
pub const SHOW_SPLASH: bool = false;

/// Messages consumed by the application.
///
/// `Next` is only produced by the host preview and tests; the device's touch
/// path drives the personality runtime directly, so the firmware never
/// constructs it.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BallMsg {
    /// The splash finished naturally or was skipped; switch to the ball.
    SplashDone,
    /// Advance through the six AI states. The host preview and the test suite
    /// use it; the device touch path goes through the personality runtime.
    Next,
}

/// First-screen application.
pub struct BallApp {
    control: Rc<BallControl>,
    splash: Rc<mosaico_s31::splash_widget::SplashControl>,
    screen: core::cell::Cell<Screen>,
}

impl BallApp {
    /// Create the application around the mounted handles.
    pub fn new(control: Rc<BallControl>, splash: Rc<mosaico_s31::splash_widget::SplashControl>) -> Self {
        control.set_skin(skin());
        if !SHOW_SPLASH {
            // The ball canvas is mounted on the first reconcile; arming the
            // entrance before that is what the splash swap does too.
            control.start_entrance();
        }
        Self {
            control,
            splash,
            screen: core::cell::Cell::new(if SHOW_SPLASH { Screen::Splash } else { Screen::Ball }),
        }
    }

    /// The screen that should currently be mounted.
    pub fn screen(&self) -> Screen {
        self.screen.get()
    }

    /// When the touch hold-off ends, in the same millisecond clock as the input
    /// task: the ball must not be grabbed while it is still falling.
    ///
    /// The splash timeline and the entrance are both pure functions of time, so
    /// the deadline is known from the start.
    pub const fn touch_hold_off_ms() -> u64 {
        if SHOW_SPLASH {
            boot_splash::TOTAL_MS + entrance::TOTAL_MS
        } else {
            entrance::TOTAL_MS
        }
    }

    /// Move to the ball screen and start the entrance.
    pub fn enter_ball(&self) {
        self.screen.set(Screen::Ball);
        self.control.start_entrance();
    }
}

impl App for BallApp {
    type Msg = BallMsg;

    fn update(&mut self, msg: BallMsg) {
        match msg {
            BallMsg::SplashDone => {
                if self.screen.get() == Screen::Splash {
                    self.enter_ball();
                }
            }
            BallMsg::Next => {
                self.control.set_expression(next_ai_state(self.control.expression()));
            }
        }
    }

    fn view(&self, theme: &Theme) -> Node<BallMsg> {
        match self.screen.get() {
            // The splash has no touch target: the wordmark and the drop must
            // play out on their own, so a finger on the glass changes nothing.
            Screen::Splash => Node::obj()
                .key(Key::Named("splash-root"))
                .style(Style::screen(theme))
                .child(
                    Node::custom(SplashWidget::new(Rc::clone(&self.splash)))
                        .key(Key::Named("splash-widget")),
                ),
            Screen::Ball => Node::obj()
                .key(Key::Named("ball-root"))
                .style(Style::screen(theme))
                .child(
                    Node::custom(BallWidget::new(Rc::clone(&self.control)))
                        .key(Key::Named("ball-widget")),
                ),
        }
    }
}

/// First-screen theme: a black field behind the ball.
pub fn theme() -> Theme {
    Theme {
        background: Color::rgb(0x00, 0x00, 0x00),
        ..Theme::dark()
    }
}

/// The skin the firmware shows. Swap this constant (or make it configurable)
/// to change the character.
pub fn skin() -> &'static skin::Skin {
    &skin::BLOB
}

#[cfg(test)]
mod tests {
    extern crate std;

    use lvgl::Display;

    use super::*;
    use crate::test_support;

    #[test]
    fn the_app_starts_on_the_configured_first_screen() {
        let _guard = test_support::lock();

        // SAFETY: the test lock is the only LVGL user in this test binary.
        unsafe { lvgl::deinit() };
        lvgl::init();
        let display = Display::register::<_, { 64 * 64 }>(64, 64, |_| {}).expect("memory display");

        let mut ui = lvgl_ui::Ui::new(&display, theme()).expect("Ui::new");
        let control = BallControl::new(BallExpression::Idle);
        let splash = mosaico_s31::splash_widget::SplashControl::new();
        let mut app = BallApp::new(Rc::clone(&control), Rc::clone(&splash));
        ui.mount(&app).expect("initial screen mounts");

        if SHOW_SPLASH {
            assert_eq!(app.screen(), Screen::Splash);
            assert_eq!(splash.create_count(), 1);
            assert_eq!(control.create_count(), 0, "the ball is not mounted yet");

            for _ in 0..3 {
                ui.tick(16).expect("Ui::tick");
            }
            assert!(splash.frames() > 0, "the splash rasterizes");

            // A skip message swaps the tree: splash deleted, ball created once.
            ui.post(BallMsg::SplashDone).expect("post");
            ui.drain(&mut app, 8).expect("drain");
            assert_eq!(app.screen(), Screen::Ball);
            assert_eq!(control.create_count(), 1, "one ball widget, no remount");
            assert!(control.entrance_active(), "the ball starts its entrance");

            // Later reconciles keep the ball instance.
            ui.tick(16).expect("tick");
            ui.drain(&mut app, 8).expect("drain");
            assert_eq!(control.create_count(), 1);
        } else {
            assert_eq!(app.screen(), Screen::Ball);
            assert_eq!(splash.create_count(), 0, "the splash is disabled");
            assert_eq!(control.create_count(), 1, "the ball mounts directly");
            assert!(control.entrance_active(), "the entrance starts at mount");
        }

        // Advance past the entrance: it finishes and reports the edge.
        let mut elapsed = 0u32;
        while elapsed < entrance::TOTAL_MS as u32 + 100 {
            ui.tick(50).expect("tick");
            elapsed += 50;
        }
        assert!(!control.entrance_active(), "the entrance completed");
        assert!(control.take_entrance_finished());

        ui.unmount();
        assert_eq!(control.delete_count(), 1, "unmount deletes the ball once");
        assert_eq!(ui.stats().mounted_nodes, 0);

        drop(ui);
        // SAFETY: the UI is unmounted; no LVGL object for this display remains.
        unsafe { display.delete() };
        // SAFETY: the display is gone and this test is the last LVGL user.
        unsafe { lvgl::deinit() };
    }

    #[test]
    fn the_touch_hold_off_covers_the_active_animations() {
        let expected = if SHOW_SPLASH {
            boot_splash::TOTAL_MS + entrance::TOTAL_MS
        } else {
            entrance::TOTAL_MS
        };
        assert_eq!(BallApp::touch_hold_off_ms(), expected);
        assert!(BallApp::touch_hold_off_ms() >= entrance::TOTAL_MS);
    }

    #[test]
    fn the_ai_state_ring_still_cycles() {
        let mut state = BallExpression::Idle;
        for _ in 0..AI_STATES.len() {
            state = next_ai_state(state);
        }
        assert_eq!(state, BallExpression::Idle);
        assert_eq!(
            next_ai_state(BallExpression::Tired),
            BallExpression::Idle,
            "an expression outside the ring returns to Idle"
        );
    }
}
