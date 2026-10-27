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

use crate::hub::uninstall::{Identity, Operation};
use alloc::rc::Rc;
use alloc::string::String;
use core::cell::Cell;

use lvgl_ui::{App, Color, Key, Style, Theme, View};

use crate::behavior::Expression;
use crate::fox::entrance;
use crate::hub::gesture::SystemGesture;
use crate::hub::registry::{AppEntry, Registry};
use crate::hub::{AppId, Hub, Page};
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
    OpenLua(Identity),
    ManageAsk(Identity),
    ManageCancel,
    ManageConfirm,
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
    registry: Registry,
    confirmation: Option<Identity>,
    management_haptic: bool,
    management: Option<Operation>,
    management_busy: bool,
    toast: Option<String>,
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
            registry: Registry::default(),
            confirmation: None,
            management_haptic: false,
            management: None,
            management_busy: false,
            toast: None,
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

    /// The Lua apps the launcher lists.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The registry entry of the Lua app on `page`, if it is one.
    pub fn lua_entry(&self, page: Page) -> Option<&AppEntry> {
        match page {
            Page::App(app) => app.lua_index().and_then(|i| self.registry.apps.get(i)),
            _ => None,
        }
    }

    /// Replace the registry (after a rescan). An open Lua app follows its id
    /// to its new index; if it is gone, its page says so.
    pub fn set_registry(&mut self, registry: Registry) {
        if let Page::App(AppId::Lua(i)) = self.hub.page {
            let id = self.registry.apps.get(i as usize).map(|e| e.app.id.clone());
            let new = id.and_then(|id| registry.find(&id));
            self.hub.page = Page::App(AppId::Lua(new.map_or(u8::MAX, |n| n as u8)));
        }
        self.registry = registry;
    }

    /// The page on screen.
    pub fn page(&self) -> Page {
        self.hub.page
    }

    /// Change page. Coming back to the fox replays its entrance.
    pub fn take_management_haptic(&mut self) -> bool {
        core::mem::take(&mut self.management_haptic)
    }
    pub fn management_busy(&self) -> bool {
        self.management_busy
    }
    pub fn take_management(&mut self) -> Option<Operation> {
        self.management.take()
    }
    pub fn management_finished(&mut self, op: Operation, result: Result<(), String>) {
        self.management_busy = false;
        self.toast = Some(match result {
            Ok(()) => match op {
                Operation::Remove(_) => hub_view::REMOVED_TOAST.into(),
                Operation::Clear(_) => hub_view::CLEARED_TOAST.into(),
            },
            Err(e) => e,
        });
    }
    pub fn confirmation(&self) -> Option<Identity> {
        self.confirmation
    }
    pub fn go(&mut self, page: Page) {
        self.confirmation = None;
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
            AppMsg::OpenLua(identity) => {
                if self.hub.page == Page::Launcher
                    && self.confirmation.is_none()
                    && !self.management_busy
                {
                    if let Some(i) = self.registry.find(identity.id()) {
                        if self.registry.apps[i].origin == identity.origin {
                            self.go(Page::App(AppId::Lua(i as u8)));
                        }
                    }
                }
            }
            AppMsg::ManageAsk(identity) => {
                if self.hub.page == Page::Launcher && !self.management_busy {
                    if let Some(i) = self.registry.find(identity.id()) {
                        if self.registry.apps[i].origin == identity.origin {
                            if self.confirmation != Some(identity) {
                                self.management_haptic = true;
                            }
                            self.confirmation = Some(identity);
                            self.toast = None;
                        }
                    }
                }
            }
            AppMsg::ManageCancel => {
                self.confirmation = None;
            }
            AppMsg::ManageConfirm => {
                if let Some(identity) = self.confirmation.take() {
                    if !self
                        .registry
                        .find(identity.id())
                        .is_some_and(|i| self.registry.apps[i].origin == identity.origin)
                    {
                        self.toast = Some(hub_view::SOURCE_CHANGED_TOAST.into());
                        return;
                    }
                    if !self.management_busy {
                        self.management_busy = true;
                        self.management =
                            Some(if identity.origin == crate::hub::registry::Origin::Data {
                                Operation::Remove(identity)
                            } else {
                                Operation::Clear(identity)
                            });
                    }
                }
            }
            AppMsg::Open(app) => {
                if self.hub.page == Page::Launcher
                    && self.confirmation.is_none()
                    && !self.management_busy
                {
                    self.go(Page::App(app));
                }
            }
            AppMsg::Gesture(gesture) => {
                if self.management_busy {
                    return;
                }
                if self.confirmation.take().is_some() {
                    return;
                }
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

    fn view(&self, theme: &Theme) -> View<AppMsg> {
        match self.screen.get() {
            // The splash has no touch target: the wordmark and the drop must
            // play out on their own, so a finger on the glass changes nothing.
            Screen::Splash => lvgl_ui::obj()
                .key(Key::Named("splash-root"))
                .style(Style::screen(theme))
                .child(
                    lvgl_ui::custom(SplashWidget::new(Rc::clone(&self.splash)))
                        .key(Key::Named("splash-widget")),
                ),
            Screen::Fox => match self.hub.page {
                Page::Home => self.fox_view(theme),
                Page::Launcher => hub_view::launcher_managed(
                    &self.registry,
                    self.confirmation,
                    self.management_busy,
                    self.toast.as_deref(),
                ),
                Page::App(app) => hub_view::app(&self.hub, &self.registry, app),
            },
        }
    }
}

impl MosaicoApp {
    fn fox_view(&self, theme: &Theme) -> View<AppMsg> {
        lvgl_ui::obj()
            .key(Key::Named("fox-root"))
            .style(Style::screen(theme))
            .child(
                lvgl_ui::custom(FoxWidget::new(Rc::clone(&self.control)))
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

    // ---- a simulated touch panel for the hub pages ----

    use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};

    static TOUCH_X: AtomicI32 = AtomicI32::new(0);
    static TOUCH_Y: AtomicI32 = AtomicI32::new(0);
    static TOUCH_DOWN: AtomicBool = AtomicBool::new(false);

    unsafe extern "C" fn read_touch(
        _indev: *mut lvgl::sys::lv_indev_t,
        data: *mut lvgl::sys::lv_indev_data_t,
    ) {
        let data = unsafe { &mut *data };
        data.point.x = TOUCH_X.load(Ordering::Relaxed);
        data.point.y = TOUCH_Y.load(Ordering::Relaxed);
        data.state = if TOUCH_DOWN.load(Ordering::Relaxed) {
            lvgl::sys::lv_indev_state_t_LV_INDEV_STATE_PRESSED
        } else {
            lvgl::sys::lv_indev_state_t_LV_INDEV_STATE_RELEASED
        };
    }

    fn run(ui: &mut lvgl_ui::Ui<AppMsg>, app: &mut MosaicoApp, ms: u32) {
        let mut t = 0;
        while t < ms {
            lvgl::tick_inc(core::time::Duration::from_millis(10));
            ui.timer_handler();
            ui.drain(app, 16).expect("drain");
            t += 10;
        }
    }

    fn tap(ui: &mut lvgl_ui::Ui<AppMsg>, app: &mut MosaicoApp, x: i32, y: i32) {
        TOUCH_X.store(x, Ordering::Relaxed);
        TOUCH_Y.store(y, Ordering::Relaxed);
        TOUCH_DOWN.store(true, Ordering::Relaxed);
        run(ui, app, 120);
        TOUCH_DOWN.store(false, Ordering::Relaxed);
        run(ui, app, 120);
    }

    #[test]
    fn a_tap_anywhere_on_a_launcher_tile_opens_its_app() {
        let _guard = test_support::lock();
        // SAFETY: the test lock is the only LVGL user in this test binary.
        unsafe { lvgl::deinit() };
        lvgl::init();
        let display =
            Display::register::<_, { 480 * 48 }>(480, 480, |_| {}).expect("memory display");
        // SAFETY: LVGL is initialized and only this thread uses it.
        let indev = unsafe {
            let indev = lvgl::sys::lv_indev_create();
            lvgl::sys::lv_indev_set_type(indev, lvgl::sys::lv_indev_type_t_LV_INDEV_TYPE_POINTER);
            lvgl::sys::lv_indev_set_read_cb(indev, Some(read_touch));
            indev
        };

        let mut ui = lvgl_ui::Ui::new(&display, theme()).expect("Ui::new");
        let control = FoxControl::new(Expression::Idle);
        let splash = SplashControl::new();
        let mut app = MosaicoApp::new(Rc::clone(&control), Rc::clone(&splash));
        fn system_apps() -> Registry {
            use crate::hub::registry::{AppEntry, LuaApp, Origin};
            Registry {
                apps: ["2048", "snake"]
                    .into_iter()
                    .map(|id| AppEntry {
                        app: LuaApp {
                            id: id.into(),
                            entry: alloc::format!("/system/apps/{id}/scripts/main.lua"),
                            args_json: "{}".into(),
                        },
                        title: id.into(),
                        order: 20,
                        origin: Origin::System,
                        dir: alloc::format!("/system/apps/{id}"),
                        icon: None,
                        missing: Vec::new(),
                    })
                    .collect(),
            }
        }
        // Two filesystem apps follow the three native Rust applications.
        app.set_registry(system_apps());
        ui.mount(&app).expect("mount");

        // Tile centres: rows of three 120 px columns, 16 px apart, centred;
        // the 96 px icon square is the upper part, the name the lower part.
        let tiles = [
            (104, 140, AppId::Settings),
            (240, 140, AppId::Sensors),
            (376, 140, AppId::Timer),
            (172, 298, AppId::Lua(0)),
            (308, 298, AppId::Lua(1)),
        ];
        for (x, icon_y, id) in tiles {
            for y in [icon_y, icon_y + 72] {
                app.go(Page::Launcher);
                ui.mark_dirty();
                run(&mut ui, &mut app, 60);
                tap(&mut ui, &mut app, x, y);
                assert_eq!(app.page(), Page::App(id), "tap at ({x}, {y})");
            }
        }

        // More than two rows: the grid scrolls from the top.
        let mut many = system_apps();
        for n in 0..5 {
            let mut e = many.apps[0].clone();
            e.app.id = alloc::format!("extra{n}");
            e.icon = None;
            many.apps.push(e);
        }
        app.set_registry(many);
        app.go(Page::Launcher);
        ui.mark_dirty();
        run(&mut ui, &mut app, 60);
        tap(&mut ui, &mut app, 104, 132);
        assert_eq!(app.page(), Page::App(AppId::Settings), "scrolling grid");
        app.set_registry(system_apps());

        // Settings: the slider, then both switches.
        app.go(Page::App(AppId::Settings));
        ui.mark_dirty();
        run(&mut ui, &mut app, 60);
        tap(&mut ui, &mut app, 240, 166);
        let brightness = app.hub().settings.brightness;
        assert!(
            (40..=80).contains(&brightness),
            "slider tap set {brightness}%"
        );
        tap(&mut ui, &mut app, 388, 233);
        assert!(!app.hub().settings.lights, "the lights switch toggles");
        tap(&mut ui, &mut app, 388, 309);
        assert!(!app.hub().settings.haptics, "the haptics switch toggles");

        // Timer: pick the 1-minute chip, then start.
        app.go(Page::App(AppId::Timer));
        ui.mark_dirty();
        run(&mut ui, &mut app, 60);
        tap(&mut ui, &mut app, 88, 334);
        assert_eq!(
            app.hub().countdown.preset(),
            0,
            "the first chip selects 1 min"
        );
        tap(&mut ui, &mut app, 154, 390);
        assert_eq!(
            app.hub().countdown.phase(),
            crate::hub::countdown::Phase::Running,
            "the primary pill starts the countdown"
        );

        // A tap between tiles does nothing.
        app.go(Page::Launcher);
        ui.mark_dirty();
        run(&mut ui, &mut app, 60);
        tap(&mut ui, &mut app, 240, 60);
        assert_eq!(app.page(), Page::Launcher);

        ui.unmount();
        drop(ui);
        // SAFETY: nothing references the input device or the display anymore.
        unsafe {
            lvgl::sys::lv_indev_delete(indev);
            display.delete();
            lvgl::deinit();
        }
    }
}
