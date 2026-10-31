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
//! raw pointer stream into [`crate::companion::runtime::Runtime`], which owns the tap
//! ladder. The only LVGL handler is on the splash, where a press skips it.
//!
//! # Motion
//!
//! The fox drops in from above the panel and lands with a squash. Touch is
//! held off for the duration (see [`FoxControl::start_entrance`] and
//! [`crate::companion::runtime::Runtime::ignore_touch_until`]).

use crate::shell::components::splash;
use crate::shell::pages;

use alloc::rc::Rc;
use alloc::string::String;
use core::cell::Cell;
use {crate::apps::lifecycle::uninstall::Identity, crate::apps::lifecycle::uninstall::Operation};

use rs_lvgl_ui::{App, Color, Key, Style, Theme, View};

use crate::companion::behavior::Expression;
use crate::companion::render::entrance;
use crate::shell::navigation::gesture::SystemGesture;
use {crate::apps::registry::AppEntry, crate::apps::registry::Registry};
use {crate::companion::widget::FoxControl, crate::companion::widget::FoxWidget};
use {
    crate::shell::components::splash::SplashControl, crate::shell::components::splash::SplashWidget,
};
use {
    crate::shell::model::Hub, crate::shell::navigation::route::AppId,
    crate::shell::navigation::route::Page,
};

/// The six AI states, in tap order.
///
/// Used by the host preview and the `Next` message; the device tap path goes
/// through [`crate::companion::runtime::Runtime`].
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
    /// Terminal reserve warning; navigation cannot replace it.
    LowBattery,
}

/// Whether the custom `ikaros` wordmark plays before the fox.
///
/// The factory bootloader already shows the Mosaico wordmark before `main_app`
/// starts, so the product decision is to skip our own wordmark. Restoring it
/// needs more than this flag: the device hides LVGL and only uploads the fox
/// canvas (the UI task asserts the flag is off).
pub const SHOW_SPLASH: bool = false;

pub use crate::shell::messages::AppMsg;

/// First-screen application.
pub struct MosaicoApp {
    control: Rc<FoxControl>,
    splash: Rc<SplashControl>,
    screen: Cell<Screen>,
    hub: Hub,
    router: crate::shell::navigation::router::Router,
    registry: Registry,
    confirmation: Option<Identity>,
    management_haptic: bool,
    management: Option<Operation>,
    management_busy: bool,
    toast: Option<String>,
    camera_action: Option<crate::services::camera::Command>,
    settings_session: crate::shell::pages::settings::model::Session,
}

impl MosaicoApp {
    /// Create the application around the mounted handles.
    pub fn new(control: Rc<FoxControl>, splash: Rc<SplashControl>) -> Self {
        control.set_radio_status(crate::services::radio::Snapshot::new());
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
            router: crate::shell::navigation::router::Router::new(),
            registry: Registry::default(),
            confirmation: None,
            management_haptic: false,
            management: None,
            management_busy: false,
            toast: None,
            camera_action: None,
            settings_session: crate::shell::pages::settings::model::Session::default(),
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
        self.registry = registry;
    }

    /// The page on screen.
    pub fn page(&self) -> Page {
        self.router.page(&self.registry)
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
                Operation::Remove(_) => pages::launcher::REMOVED_TOAST.into(),
                Operation::Clear(_) => pages::launcher::CLEARED_TOAST.into(),
            },
            Err(e) => e,
        });
    }
    pub fn confirmation(&self) -> Option<Identity> {
        self.confirmation
    }
    pub fn go(&mut self, page: Page) {
        if self.screen.get() == Screen::LowBattery {
            return;
        }
        self.confirmation = None;
        if page == self.page() {
            return;
        }
        let route = self.router.resolve(page, &self.registry);
        self.router.navigate(route);
        self.settings_session.password = crate::shell::pages::settings::model::Password::default();
        self.set_settings_page(crate::shell::pages::settings::model::Page::Root);
        if page == Page::Home && self.screen.get() == Screen::Fox {
            self.control.start_entrance();
        }
    }

    pub fn take_camera_action(&mut self) -> Option<crate::services::camera::Command> {
        self.camera_action.take()
    }
    pub fn route(&self) -> crate::shell::navigation::route::Route {
        self.router.current()
    }
    pub fn page_session(&self) -> crate::shell::navigation::router::Session {
        self.router.session()
    }
    pub fn accepts_page_result(&self, session: crate::shell::navigation::router::Session) -> bool {
        self.screen.get() != Screen::LowBattery && self.router.accepts(session)
    }
    pub fn settings_page(&self) -> crate::shell::pages::settings::model::Page {
        self.router.settings_page()
    }
    pub fn set_settings_page(&mut self, page: crate::shell::pages::settings::model::Page) {
        if self.screen.get() == Screen::LowBattery {
            return;
        }
        self.router.set_settings_page(page);
    }
    pub fn settings_session(&self) -> &crate::shell::pages::settings::model::Session {
        &self.settings_session
    }
    pub fn settings_session_mut(&mut self) -> &mut crate::shell::pages::settings::model::Session {
        &mut self.settings_session
    }
    pub fn take_settings_action(&mut self) -> Option<crate::services::settings::Command> {
        self.settings_session.action.take()
    }
    fn settings_back(&mut self) {
        crate::shell::pages::settings::update::back(&mut self.settings_session, &mut self.router);
    }

    /// Move to the fox screen and start the entrance.
    pub fn enter_fox(&self) {
        if self.screen.get() == Screen::LowBattery {
            return;
        }
        self.screen.set(Screen::Fox);
        self.control.start_entrance();
    }
    pub fn show_low_battery(&self) {
        self.screen.set(Screen::LowBattery);
    }
}

impl App for MosaicoApp {
    type Msg = AppMsg;

    fn update(&mut self, msg: AppMsg) {
        if self.screen.get() == Screen::LowBattery {
            return;
        }
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
                if self.page() == Page::Launcher
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
                if self.page() == Page::Launcher && !self.management_busy {
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
                        self.toast = Some(pages::launcher::SOURCE_CHANGED_TOAST.into());
                        return;
                    }
                    if !self.management_busy {
                        self.management_busy = true;
                        self.management =
                            Some(if identity.origin == crate::apps::registry::Origin::Data {
                                Operation::Remove(identity)
                            } else {
                                Operation::Clear(identity)
                            });
                    }
                }
            }
            AppMsg::Open(app) => {
                if self.page() == Page::Launcher
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
                if gesture == SystemGesture::Back
                    && self.page() == Page::App(AppId::Settings)
                    && self.settings_page() != crate::shell::pages::settings::model::Page::Root
                {
                    self.settings_back();
                    return;
                }
                if self.screen.get() == Screen::Fox {
                    let next = self.route().after(gesture).page(&self.registry);
                    self.go(next);
                }
            }
            AppMsg::CameraCapture => {
                self.camera_action = Some(crate::services::camera::Command::Capture)
            }
            AppMsg::CameraRetry => {
                self.camera_action = Some(crate::services::camera::Command::Start)
            }
            AppMsg::TimerPreset(index) => self.hub.countdown.select(index),
            AppMsg::TimerToggle => {
                let now = self.hub.now_ms;
                self.hub.countdown.toggle(now);
            }
            AppMsg::TimerReset => self.hub.countdown.reset(),
            msg => {
                if crate::shell::pages::settings::update::update(
                    &mut self.hub,
                    &mut self.settings_session,
                    &mut self.router,
                    msg,
                ) {
                    self.go(Page::Home);
                }
            }
        }
    }

    fn view(&self, theme: &Theme) -> View<AppMsg> {
        match self.screen.get() {
            Screen::LowBattery => pages::low_battery::low_battery(),
            // The splash has no touch target: the wordmark and the drop must
            // play out on their own, so a finger on the glass changes nothing.
            Screen::Splash => rs_lvgl_ui::obj()
                .key(Key::Named("splash-root"))
                .style(Style::screen(theme))
                .child(
                    rs_lvgl_ui::custom(SplashWidget::new(Rc::clone(&self.splash)))
                        .key(Key::Named("splash-widget")),
                ),
            Screen::Fox => match self.page() {
                Page::Home => self.fox_view(theme),
                Page::Launcher => pages::launcher::launcher_managed(
                    &self.registry,
                    self.confirmation,
                    self.management_busy,
                    self.toast.as_deref(),
                ),
                Page::App(AppId::Settings) => pages::settings::view::view(
                    &self.hub,
                    &self.settings_session,
                    self.settings_page(),
                ),
                Page::App(app) => pages::app(&self.hub, &self.registry, app),
            },
        }
    }
}

impl MosaicoApp {
    fn fox_view(&self, theme: &Theme) -> View<AppMsg> {
        rs_lvgl_ui::obj()
            .key(Key::Named("fox-root"))
            .style(Style::screen(theme))
            .child(
                rs_lvgl_ui::custom(FoxWidget::new(Rc::clone(&self.control)))
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
mod tests;
