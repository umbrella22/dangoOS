//! The LVGL screen tree: boot splash, then the fox.
//!
//! - [`MosaicoApp`]: the application. Its screen is state, so `App::view`
//!   returns either the splash or the fox and the reconciler swaps the
//!   subtree; each subtree is keyed, so every widget is created once.
//! - [`fox_widget`]: the fox as an LVGL custom widget.
//! - [`canvas_pool`]: the two frame buffers the widget renders into and the
//!   panel plane uploads from.
//! - [`splash`]: the optional `ikaros` wordmark (off on the device, see
//!   [`SHOW_SPLASH`]).
//!
//! # Touch routing
//!
//! LVGL events are not the source of truth for the fox. The UI task feeds the
//! raw pointer stream into [`crate::behavior::Runtime`], which owns the tap
//! ladder. The only LVGL handler is on the splash, where a press skips it.
//!
//! # Motion
//!
//! The fox drops in from above the panel and lands with a squash. Touch is
//! held off for the duration (see [`FoxControl::start_entrance`] and
//! [`crate::behavior::Runtime::ignore_touch_until`]).

pub mod canvas_pool;
pub mod fox_widget;
pub mod hub_font;
pub mod hub_view;
pub mod splash;

use alloc::rc::Rc;
use core::cell::Cell;

use lvgl_ui::{App, Color, Key, Node, Style, Theme};

use crate::behavior::Expression;
use crate::hub::gesture::SystemGesture;
use crate::hub::{AppId, Hub, Page};
use crate::fox::entrance;
use fox_widget::{FoxControl, FoxWidget};
use splash::{SplashControl, SplashWidget};

/// The six AI states, in tap order.
///
/// Used by the host preview and the `Next` message; the device tap path goes
/// through [`crate::behavior::Runtime`].
pub const AI_STATES: [Expression; 6] = [
    Expression::Idle,
    Expression::Listening,
    Expression::Thinking,
    Expression::Speaking,
    Expression::Happy,
    Expression::Error,
];

/// The next AI state after `current`.
pub fn next_ai_state(current: Expression) -> Expression {
    match AI_STATES.iter().position(|&state| state == current) {
        Some(index) => AI_STATES[(index + 1) % AI_STATES.len()],
        None => Expression::Idle,
    }
}

/// Which screen is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// The `ikaros` wordmark is playing.
    Splash,
    /// The fox is on screen (possibly still dropping in).
    Fox,
}

/// Whether the custom `ikaros` wordmark plays before the fox.
///
/// The factory bootloader already shows the Mosaico wordmark before `main_app`
/// starts, so the product decision is to skip our own wordmark. Restoring it
/// needs more than this flag: the device hides LVGL and only uploads the fox
/// canvas (the UI task asserts the flag is off).
pub const SHOW_SPLASH: bool = false;

/// Messages consumed by the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppMsg {
    /// The splash finished or was skipped; switch to the fox.
    SplashDone,
    /// Advance through the six AI states (host preview and tests).
    Next,
    /// A launcher tile was tapped.
    Open(AppId),
    /// A system edge gesture (from the UI task, not LVGL).
    Gesture(SystemGesture),
    /// Settings: panel brightness, percent.
    Brightness(i32),
    /// Settings: interaction-board lights.
    Lights(bool),
    /// Settings: vibration motor.
    Haptics(bool),
    /// Timer: select a preset (index into `PRESETS_MIN`).
    TimerPreset(usize),
    /// Timer: start, pause or resume.
    TimerToggle,
    /// Timer: back to the full preset.
    TimerReset,
}

/// First-screen application.
pub struct MosaicoApp {
    control: Rc<FoxControl>,
    splash: Rc<SplashControl>,
    screen: Cell<Screen>,
    hub: Hub,
}

impl MosaicoApp {
    /// Create the application around the mounted handles.
    pub fn new(control: Rc<FoxControl>, splash: Rc<SplashControl>) -> Self {
        if !SHOW_SPLASH {
            // The fox mounts on the first reconcile; arming the entrance
            // before that is what the splash swap does too.
            control.start_entrance();
        }
        Self {
            control,
            splash,
            screen: Cell::new(if SHOW_SPLASH {
                Screen::Splash
            } else {
                Screen::Fox
            }),
            hub: Hub::new(),
        }
    }

    /// The screen that should currently be mounted.
    pub fn screen(&self) -> Screen {
        self.screen.get()
    }

    /// When the touch hold-off ends, in the same millisecond clock as the input
    /// task: the fox must not react while it is still falling.
    ///
    /// The splash timeline and the entrance are both pure functions of time, so
    /// the deadline is known from the start.
    pub const fn touch_hold_off_ms() -> u64 {
        if SHOW_SPLASH {
            splash::TOTAL_MS + entrance::TOTAL_MS
        } else {
            entrance::TOTAL_MS
        }
    }

    /// Hub state (page, settings, app state).
    pub fn hub(&self) -> &Hub {
        &self.hub
    }

    /// Mutable hub state, for the UI task (clock, sensors, about).
    pub fn hub_mut(&mut self) -> &mut Hub {
        &mut self.hub
    }

    /// The page on screen.
    pub fn page(&self) -> Page {
        self.hub.page
    }

    /// Change page. Coming back to the fox replays its entrance.
    pub fn go(&mut self, page: Page) {
        if page == self.hub.page {
            return;
        }
        self.hub.page = page;
        if page == Page::Home && self.screen.get() == Screen::Fox {
            self.control.start_entrance();
        }
    }

    /// Move to the fox screen and start the entrance.
    pub fn enter_fox(&self) {
        self.screen.set(Screen::Fox);
        self.control.start_entrance();
    }
}

impl App for MosaicoApp {
    type Msg = AppMsg;

    fn update(&mut self, msg: AppMsg) {
        match msg {
            AppMsg::SplashDone => {
                if self.screen.get() == Screen::Splash {
                    self.enter_fox();
                }
            }
            AppMsg::Next => {
                self.control
                    .set_expression(next_ai_state(self.control.expression()));
            }
            AppMsg::Open(app) => {
                if self.hub.page == Page::Launcher {
                    self.go(Page::App(app));
                }
            }
            AppMsg::Gesture(gesture) => {
                if self.screen.get() == Screen::Fox {
                    let next = self.hub.page.after(gesture);
                    self.go(next);
                }
            }
            AppMsg::Brightness(percent) => self.hub.settings.set_brightness(percent),
            AppMsg::Lights(on) => self.hub.settings.lights = on,
            AppMsg::Haptics(on) => self.hub.settings.haptics = on,
            AppMsg::TimerPreset(index) => self.hub.countdown.select(index),
            AppMsg::TimerToggle => {
                let now = self.hub.now_ms;
                self.hub.countdown.toggle(now);
            }
            AppMsg::TimerReset => self.hub.countdown.reset(),
        }
    }

    fn view(&self, theme: &Theme) -> Node<AppMsg> {
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
            Screen::Fox => match self.hub.page {
                Page::Home => self.fox_view(theme),
                Page::Launcher => hub_view::launcher(),
                Page::App(app) => hub_view::app(&self.hub, app),
            },
        }
    }
}

impl MosaicoApp {
    fn fox_view(&self, theme: &Theme) -> Node<AppMsg> {
        Node::obj()
                .key(Key::Named("fox-root"))
                .style(Style::screen(theme))
                .child(
                    Node::custom(FoxWidget::new(Rc::clone(&self.control)))
                        .key(Key::Named("fox-widget")),
                )
    }
}

/// A black field behind the fox.
pub fn theme() -> Theme {
    Theme {
        background: Color::rgb(0x00, 0x00, 0x00),
        ..Theme::dark()
    }
}

#[cfg(test)]
mod tests {
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
        let control = FoxControl::new(Expression::Idle);
        let splash = SplashControl::new();
        let mut app = MosaicoApp::new(Rc::clone(&control), Rc::clone(&splash));
        ui.mount(&app).expect("initial screen mounts");

        if SHOW_SPLASH {
            assert_eq!(app.screen(), Screen::Splash);
            assert_eq!(splash.create_count(), 1);
            assert_eq!(control.create_count(), 0, "the fox is not mounted yet");

            for _ in 0..3 {
                ui.tick(16).expect("Ui::tick");
            }
            assert!(splash.frames() > 0, "the splash rasterizes");

            // A skip message swaps the tree: splash deleted, fox created once.
            ui.post(AppMsg::SplashDone).expect("post");
            ui.drain(&mut app, 8).expect("drain");
            assert_eq!(app.screen(), Screen::Fox);
            assert_eq!(control.create_count(), 1, "one fox widget, no remount");
            assert!(control.entrance_active(), "the fox starts its entrance");

            // Later reconciles keep the fox instance.
            ui.tick(16).expect("tick");
            ui.drain(&mut app, 8).expect("drain");
            assert_eq!(control.create_count(), 1);
        } else {
            assert_eq!(app.screen(), Screen::Fox);
            assert_eq!(splash.create_count(), 0, "the splash is disabled");
            assert_eq!(control.create_count(), 1, "the fox mounts directly");
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
        assert_eq!(control.delete_count(), 1, "unmount deletes the fox once");
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
            splash::TOTAL_MS + entrance::TOTAL_MS
        } else {
            entrance::TOTAL_MS
        };
        assert_eq!(MosaicoApp::touch_hold_off_ms(), expected);
        assert!(MosaicoApp::touch_hold_off_ms() >= entrance::TOTAL_MS);
    }

    #[test]
    fn the_ai_state_ring_still_cycles() {
        let mut state = Expression::Idle;
        for _ in 0..AI_STATES.len() {
            state = next_ai_state(state);
        }
        assert_eq!(state, Expression::Idle);
        assert_eq!(
            next_ai_state(Expression::Tired),
            Expression::Idle,
            "an expression outside the ring returns to Idle"
        );
    }
}
