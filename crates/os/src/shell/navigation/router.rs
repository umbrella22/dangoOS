//! The committed route, one replacement target, and page session identity.
use super::route::{AppId, Page, Route};
use crate::apps::registry::Registry;
use crate::shell::pages::settings::model::Page as SettingsPage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Session(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub from: Route,
    pub to: Route,
    pub session: Session,
}

pub struct Router {
    current: Route,
    pending: Option<Route>,
    session: Session,
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

impl Router {
    pub const fn new() -> Self {
        Self {
            current: Route::Home,
            pending: None,
            session: Session(0),
        }
    }
    pub const fn current(&self) -> Route {
        self.current
    }
    pub const fn session(&self) -> Session {
        self.session
    }
    pub fn accepts(&self, session: Session) -> bool {
        self.session == session
    }

    /// A newer intent replaces the one pending target; no navigation history
    /// retains page instances or service results.
    pub fn request(&mut self, target: Route) {
        self.pending = Some(target);
    }
    pub fn commit(&mut self) -> Option<Change> {
        let to = self.pending.take()?;
        if to == self.current {
            return None;
        }
        let from = core::mem::replace(&mut self.current, to);
        self.session.0 = self.session.0.wrapping_add(1);
        Some(Change {
            from,
            to,
            session: self.session,
        })
    }
    pub fn navigate(&mut self, target: Route) -> bool {
        self.request(target);
        self.commit().is_some()
    }
    pub fn page(&self, registry: &Registry) -> Page {
        self.current.page(registry)
    }
    pub const fn settings_page(&self) -> SettingsPage {
        match self.current {
            Route::Settings(page) => page,
            _ => SettingsPage::Root,
        }
    }
    pub fn set_settings_page(&mut self, page: SettingsPage) {
        if matches!(self.current, Route::Settings(_)) {
            self.navigate(Route::Settings(page));
        }
    }
    pub fn resolve(&self, page: Page, registry: &Registry) -> Route {
        match page {
            Page::Home => Route::Home,
            Page::Launcher => Route::Launcher,
            Page::App(AppId::Settings) => Route::Settings(SettingsPage::Root),
            Page::App(AppId::Sensors) => Route::Sensors,
            Page::App(AppId::Timer) => Route::Timer,
            Page::App(AppId::Camera) => Route::Camera,
            Page::App(AppId::Lua(index)) => registry
                .apps
                .get(index as usize)
                .and_then(|entry| {
                    crate::apps::lifecycle::uninstall::Identity::new(&entry.app.id, entry.origin)
                })
                .map(Route::Lua)
                .unwrap_or(Route::UnavailableLua),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_navigation_is_replaced_and_stale_page_results_are_rejected() {
        let mut router = Router::new();
        let old = router.session();
        router.request(Route::Camera);
        router.request(Route::Settings(SettingsPage::Wifi));
        assert_eq!(router.current(), Route::Home);
        let change = router.commit().unwrap();
        assert_eq!(change.to, Route::Settings(SettingsPage::Wifi));
        assert!(!router.accepts(old));
        assert!(router.accepts(change.session));
        assert!(router.commit().is_none());
        router.navigate(Route::Settings(SettingsPage::Wifi));
        assert!(router.accepts(change.session));
    }
    #[test]
    fn settings_subroutes_cannot_exist_outside_settings() {
        let mut router = Router::new();
        router.set_settings_page(SettingsPage::Password);
        assert_eq!(router.current(), Route::Home);
        router.navigate(Route::Settings(SettingsPage::Password));
        assert_eq!(
            router
                .current()
                .after(super::super::gesture::SystemGesture::Back),
            Route::Settings(SettingsPage::Wifi)
        );
    }
}
