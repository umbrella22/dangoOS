//! The UI copies changed service text at most ten times per second.
use super::*;
#[derive(Default)]
pub(super) struct Sync {
    revision: u32,
    last_ms: u64,
}
impl Sync {
    pub fn poll(&mut self, app: &mut MosaicoApp, ui: &mut Ui<AppMsg>, now_ms: u64) {
        use crate::services::voice::{device, Command};
        crate::services::resources::voice_font::activate_prepared();
        if let Some(action) = app.take_voice_action() {
            match action {
                Command::Toggle => {
                    device::toggle();
                }
                Command::Cancel => device::cancel(),
                Command::Page(offset) => device::page(offset),
                _ => (),
            }
        }
        let revision = device::revision();
        if revision != self.revision && now_ms.saturating_sub(self.last_ms) >= 100 {
            self.revision = revision;
            self.last_ms = now_ms;
            app.set_voice(device::snapshot());
            if app.page() == Page::App(AppId::Voice) {
                ui.mark_dirty();
            }
        }
    }
}
