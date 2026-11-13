//! Navigation contracts across registry replacement, editing and terminal state.
use os::apps::{
    lifecycle::uninstall::Identity,
    registry::{AppEntry, LuaApp, Origin, Registry},
};
use os::shell::{
    navigation::{
        gesture::SystemGesture,
        route::{AppId, Page, Route},
    },
    pages::settings::model::Page as SettingsPage,
};
use os::{AppMsg, Expression, FoxControl, MosaicoApp, SplashControl};
use rs_lvgl_ui::App;

fn entry(id: &str, origin: Origin) -> AppEntry {
    AppEntry {
        app: LuaApp {
            id: id.into(),
            entry: format!("/system/apps/{id}/main.lua"),
            args_json: "{}".into(),
        },
        title: id.into(),
        order: 0,
        origin,
        dir: format!("/system/apps/{id}"),
        icon: None,
        missing: Vec::new(),
    }
}
fn app() -> MosaicoApp {
    MosaicoApp::new(FoxControl::new(Expression::Idle), SplashControl::new())
}

#[test]
fn open_lua_follows_its_identity_through_reorder_removal_and_restoration() {
    let mut app = app();
    app.set_registry(Registry {
        apps: vec![entry("alpha", Origin::System), entry("beta", Origin::Data)],
    });
    app.go(Page::App(AppId::Lua(1)));
    let route = app.route();
    app.set_registry(Registry {
        apps: vec![entry("beta", Origin::Data), entry("alpha", Origin::System)],
    });
    assert_eq!(app.route(), route);
    assert_eq!(app.page(), Page::App(AppId::Lua(0)));
    assert_eq!(app.lua_entry(app.page()).unwrap().app.id, "beta");
    app.set_registry(Registry {
        apps: vec![entry("alpha", Origin::System)],
    });
    assert!(app.lua_entry(app.page()).is_none());
    app.set_registry(Registry {
        apps: vec![entry("alpha", Origin::System), entry("beta", Origin::Data)],
    });
    assert_eq!(app.lua_entry(app.page()).unwrap().app.id, "beta");
}

#[test]
fn confirmation_precedes_parent_navigation_and_changed_sources_require_a_new_action() {
    let mut app = app();
    let identity = Identity::new("alpha", Origin::System).unwrap();
    app.set_registry(Registry {
        apps: vec![entry("alpha", Origin::System)],
    });
    app.go(Page::Launcher);
    app.update(AppMsg::ManageAsk(identity));
    app.update(AppMsg::Gesture(SystemGesture::Back));
    assert_eq!(app.page(), Page::Launcher);
    assert!(app.confirmation().is_none());
    app.update(AppMsg::ManageAsk(identity));
    app.set_registry(Registry {
        apps: vec![entry("alpha", Origin::Data)],
    });
    app.update(AppMsg::ManageConfirm);
    assert!(app.take_management().is_none());
    app.update(AppMsg::OpenLua(identity));
    assert_eq!(app.page(), Page::Launcher);
    app.update(AppMsg::OpenLua(
        Identity::new("alpha", Origin::Data).unwrap(),
    ));
    assert!(matches!(app.route(), Route::Lua(_)));
}

#[test]
fn leaving_password_invalidates_its_results_and_clears_the_edit_before_back() {
    let mut app = app();
    app.go(Page::App(AppId::Settings));
    app.update(AppMsg::SettingsPage(SettingsPage::Password));
    app.update(AppMsg::WifiPassword(
        os::shell::pages::settings::model::Password("private-key".into()),
    ));
    let password_session = app.page_session();
    app.update(AppMsg::Gesture(SystemGesture::Back));
    assert_eq!(app.route(), Route::Settings(SettingsPage::Wifi));
    assert!(app.settings_session().password.0.is_empty());
    assert!(!app.accepts_page_result(password_session));
    app.update(AppMsg::Gesture(SystemGesture::Back));
    assert_eq!(app.route(), Route::Settings(SettingsPage::Wireless));
    app.update(AppMsg::Gesture(SystemGesture::Back));
    assert_eq!(app.route(), Route::Settings(SettingsPage::Root));
    app.update(AppMsg::Gesture(SystemGesture::Back));
    assert_eq!(app.page(), Page::Launcher);
    app.show_low_battery();
    app.go(Page::Home);
    app.update(AppMsg::Open(AppId::Camera));
    assert_eq!(app.page(), Page::Launcher);
    assert_eq!(app.screen(), os::Screen::LowBattery);
    app.set_settings_page(SettingsPage::Wifi);
    assert_eq!(app.page(), Page::Launcher);
}

#[test]
fn low_battery_blocks_settings_subroutes_and_late_page_messages() {
    let mut app = app();
    app.go(Page::App(AppId::Settings));
    app.set_settings_page(SettingsPage::Character);
    let session = app.page_session();
    app.show_low_battery();
    app.set_settings_page(SettingsPage::Wifi);
    assert_eq!(app.route(), Route::Settings(SettingsPage::Character));
    assert!(!app.accepts_page_result(session));
}

#[test]
fn leaving_voice_cancels_queued_work_and_late_start_messages_do_not_reopen_it() {
    use os::services::voice::Command;
    let mut app = app();
    app.go(Page::App(AppId::Voice));
    let old = app.page_session();
    app.update(AppMsg::VoiceToggle);
    app.update(AppMsg::Gesture(SystemGesture::Back));
    assert_eq!(app.page(), Page::Launcher);
    assert_eq!(app.take_voice_action(), Some(Command::Cancel));
    assert!(!app.accepts_page_result(old));
    app.update(AppMsg::VoiceToggle);
    assert_eq!(app.take_voice_action(), None);
    app.go(Page::App(AppId::Voice));
    assert_eq!(app.route(), Route::Voice);
    app.show_low_battery();
    app.update(AppMsg::VoiceToggle);
    assert_eq!(app.take_voice_action(), None);
}

#[test]
fn new_voice_session_opens_its_page_but_updates_after_leaving_do_not_reopen_it() {
    use os::services::voice::{Phase, Snapshot};
    let mut app = app();
    app.set_voice(Snapshot {
        session: 1,
        phase: Phase::Done,
        reply: "团子已连接。".into(),
        ..Snapshot::default()
    });
    assert_eq!(app.route(), Route::Voice);
    app.go(Page::Launcher);
    app.set_voice(Snapshot {
        session: 1,
        phase: Phase::Replying,
        ..Snapshot::default()
    });
    assert_eq!(app.page(), Page::Launcher);
    app.set_voice(Snapshot {
        session: 2,
        phase: Phase::Cancelled,
        ..Snapshot::default()
    });
    assert_eq!(app.page(), Page::Launcher);
    app.set_voice(Snapshot {
        session: 3,
        phase: Phase::Preparing,
        ..Snapshot::default()
    });
    assert_eq!(app.route(), Route::Voice);
    app.show_low_battery();
    app.set_voice(Snapshot {
        session: 4,
        phase: Phase::Replying,
        ..Snapshot::default()
    });
    assert_eq!(app.screen(), os::Screen::LowBattery);
}

#[test]
fn voice_page_actions_use_window_offsets_and_wait_for_the_turn_to_stop() {
    use os::services::voice::{Command, Phase, Snapshot, WINDOW_BYTES};
    let mut app = app();
    app.set_voice(Snapshot {
        session: 1,
        phase: Phase::Done,
        reply: "x".repeat(WINDOW_BYTES),
        reply_start: 2048,
        reply_bytes: 8192,
        ..Snapshot::default()
    });
    app.update(AppMsg::VoicePage(false));
    assert_eq!(app.take_voice_action(), Some(Command::Page(1024)));
    app.update(AppMsg::VoicePage(true));
    assert_eq!(app.take_voice_action(), Some(Command::Page(3072)));
    app.set_voice(Snapshot {
        session: 1,
        phase: Phase::Replying,
        ..Snapshot::default()
    });
    app.update(AppMsg::VoicePage(false));
    assert_eq!(app.take_voice_action(), None);
}
