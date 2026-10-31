//! Preference reconciliation and Settings service completion receipts.
use super::*;
use crate::services::settings::preferences::Settings;
pub(super) struct Sync {
    pub value: Settings,
    preferences_loaded: bool,
    settings_edited: bool,
    last_settings_page: crate::shell::pages::settings::model::Page,
    operations: crate::shell::pages::settings::operations::Operations,
}
impl Sync {
    pub fn new(value: Settings) -> Self {
        Self {
            value,
            preferences_loaded: false,
            settings_edited: false,
            last_settings_page: crate::shell::pages::settings::model::Page::Root,
            operations: crate::shell::pages::settings::operations::Operations::new(
                crate::services::settings::device::result(),
            ),
        }
    }
    pub fn skin_activated(
        &mut self,
        app: &mut MosaicoApp,
        id: alloc::string::String,
        choices: alloc::vec::Vec<crate::companion::skins::Choice>,
    ) {
        let current = self
            .operations
            .skin_activated()
            .is_some_and(|session| app.accepts_page_result(session));
        let session = app.settings_session_mut();
        session.selected_skin = id;
        session.skins = choices;
        session.loading_skin = None;
        if current {
            session.message = None;
        }
    }
    pub fn poll(
        &mut self,
        app: &mut MosaicoApp,
        ui: &mut Ui<AppMsg>,
        port_busy: bool,
        frames: &mut frames::Frames,
        radio: crate::services::radio::Snapshot,
    ) -> crate::services::settings::device::Preferences {
        let preferences = crate::services::settings::device::preferences();
        if app.hub().about.free_kib != preferences.heap_free_kib {
            app.hub_mut().about.free_kib = preferences.heap_free_kib;
            if app.page() == Page::App(AppId::Settings)
                && app.settings_page() == crate::shell::pages::settings::model::Page::System
            {
                ui.mark_dirty();
            }
        }
        if app.page() != Page::App(AppId::Settings)
            || app.settings_page() != crate::shell::pages::settings::model::Page::Character
        {
            // Once the chooser is hidden, activation need not wait for its frame.
            crate::services::settings::device::skin_feedback_painted();
            frames.skin_loading_frame_submitted = false;
        } else if frames.skin_loading_frame_submitted && !port_busy {
            // The loading frame has reached the panel before any resource I/O.
            crate::services::settings::device::skin_feedback_painted();
            frames.skin_loading_frame_submitted = false;
        }
        if app.page() != Page::App(AppId::Settings)
            || matches!(
                app.settings_page(),
                crate::shell::pages::settings::model::Page::Password
                    | crate::shell::pages::settings::model::Page::Proxy
            )
        {
            crate::services::settings::device::feedback_painted(preferences.saving_revision);
        } else if frames.saving_frame_submitted == preferences.saving_revision && !port_busy {
            // The shadow lease has returned: the indicator reached the panel.
            crate::services::settings::device::feedback_painted(frames.saving_frame_submitted);
        }
        if preferences.saving_revision != frames.saving_frame_submitted {
            frames.saving_frame_submitted = 0;
        }
        if !self.preferences_loaded && preferences.loaded {
            // Edits made before mounting storage take precedence over saved
            // boot values. Loading preferences never replays haptics.
            if !self.settings_edited && app.hub().settings == self.value {
                if let Some(initial) = preferences.initial {
                    app.hub_mut().settings = initial;
                    self.value = initial;
                    plane::set_brightness(initial.brightness);
                    crate::services::audio::task::volume(initial.audio_volume);
                    crate::services::audio::task::mute(initial.audio_muted);
                    lights::set_enabled(initial.lights);
                    haptic::set_enabled(initial.haptics);
                    crate::services::camera::task::set_preferences(initial);
                    ui.mark_dirty();
                }
            }
            self.preferences_loaded = true;
        }
        // Collect the previous completion before accepting another request.
        // A completion racing this read leaves the tracker occupied, so
        // its result cannot be assigned to a newer page or action.
        let result = crate::services::settings::device::result();
        if let Some(completion) = self.operations.complete(result) {
            app.settings_session_mut().networks = crate::services::radio::device::wifi::networks();
            if completion
                .session
                .is_some_and(|session| app.accepts_page_result(session))
            {
                app.settings_session_mut().message = if completion.failed {
                    Some("操作失败，请重试")
                } else {
                    None
                };
            }
            if completion.failed {
                app.settings_session_mut().loading_skin = None;
            }
            ui.mark_dirty();
        }
        if let Some(action) = app.take_settings_action() {
            let selects_skin = matches!(&action, crate::services::settings::Command::SelectSkin(_));
            let accepted = self
                .operations
                .submit(app.page_session(), selects_skin, || {
                    crate::services::settings::device::submit(action)
                });
            if !accepted {
                app.settings_session_mut().loading_skin = None;
            }
            app.settings_session_mut().message = if accepted {
                None
            } else {
                Some("请等待当前操作完成")
            };
            ui.mark_dirty();
        }
        if let Some(proxy) = crate::services::settings::device::take_proxy_change() {
            app.update(AppMsg::UpdateProxySet(proxy));
            ui.mark_dirty();
        }
        if app.settings_page() != self.last_settings_page {
            self.last_settings_page = app.settings_page();
            if self.last_settings_page == crate::shell::pages::settings::model::Page::Storage {
                crate::services::settings::device::refresh_storage();
            }
        }
        if app.page() == Page::App(AppId::Settings) {
            let wifi_saved = crate::services::radio::device::wifi::saved();
            if app.settings_session().wifi_saved != wifi_saved {
                app.settings_session_mut().wifi_saved = wifi_saved;
                ui.mark_dirty();
            }
            let update = crate::services::update::task::snapshot();
            if app.settings_session().update != update {
                app.settings_session_mut().update = update;
                if matches!(
                    app.settings_page(),
                    crate::shell::pages::settings::model::Page::Root
                        | crate::shell::pages::settings::model::Page::Update
                ) {
                    ui.mark_dirty();
                }
            }
            let sound = crate::services::audio::task::snapshot();
            if app.hub().settings.audio_volume == self.value.audio_volume
                && sound.volume != self.value.audio_volume
            {
                app.hub_mut().settings.audio_volume = sound.volume;
                ui.mark_dirty();
            }
            if app.hub().settings.audio_muted == self.value.audio_muted
                && sound.muted != self.value.audio_muted
            {
                app.hub_mut().settings.audio_muted = sound.muted;
                ui.mark_dirty();
            }
            let current = app.hub().settings;
            let info = crate::shell::pages::settings::model::Info {
                wifi: radio.wifi,
                bluetooth: radio.bluetooth,
                ip: radio.ip,
                audio: crate::services::audio::task::snapshot().state,
                busy: crate::services::settings::device::busy()
                    || crate::services::radio::device::wifi::busy()
                    || app.settings_session().wifi_saved.saving
                    || app.settings_session().loading_skin.is_some()
                    || crate::services::update::ota::device::active()
                    || crate::services::update::task::busy(),
                storage_ready: preferences.storage_ready,
                data_total_kib: preferences.total_kib,
                data_free_kib: preferences.free_kib,
                saved: preferences.persisted == Some(current),
                save_error: preferences.error || app.settings_session().wifi_saved.error,
                saving: preferences.saving_revision != 0
                    || app.settings_session().wifi_saved.saving,
                candidate: crate::services::update::ota::device::candidate(),
                boot_slot: crate::services::update::ota::device::booted_slot().ok(),
            };
            if app.settings_session().info != info {
                app.settings_session_mut().info = info;
                ui.mark_dirty();
            }
        }
        let current = app.hub().settings;
        crate::services::update::task::configure(current);
        if app.page() == Page::App(AppId::Camera) {
            if let Some(action) = app.take_camera_action() {
                match action {
                    crate::services::camera::Command::Capture => {
                        crate::services::camera::task::request_capture();
                    }
                    crate::services::camera::Command::Start => {
                        crate::services::camera::task::set_ui_preview(true)
                    }
                    _ => {}
                }
            }
        }
        if current != self.value {
            if current.audio_volume != self.value.audio_volume {
                crate::services::audio::task::volume(current.audio_volume);
            }
            if current.audio_muted != self.value.audio_muted {
                crate::services::audio::task::mute(current.audio_muted);
            }
            if current.brightness != self.value.brightness {
                plane::set_brightness(current.brightness);
            }
            if current.lights != self.value.lights {
                lights::set_enabled(current.lights);
                if current.lights {
                    // Switched on in Settings: replay the wipe on the strip.
                    lights::replay_preview();
                }
            }
            haptic::set_enabled(current.haptics);
            if current.haptics && !self.value.haptics {
                // Switched on: one buzz so the motor answers the switch.
                haptic::play(HapticPattern::Satisfied);
            }
            crate::services::camera::task::set_preferences(current);
            self.settings_edited = true;
            // Returning a control to its saved value must also cancel an older
            // queued edit; the preference task skips the resulting no-op write.
            crate::services::settings::device::save(current);
            self.value = current;
        }

        preferences
    }
}
