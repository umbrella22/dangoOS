//! Settings interactions and their service requests.
use super::model::{Password, Session};
use crate::shell::{messages::AppMsg, model::Hub, navigation::router::Router};

pub(crate) fn back(session: &mut Session, router: &mut Router) {
    session.password = Password::default();
    session.password_hidden = false;
    router.set_settings_page(router.settings_page().back());
}

/// Returns whether the shell must go Home after recording the operation.
pub(crate) fn update(
    hub: &mut Hub,
    session: &mut Session,
    router: &mut Router,
    msg: AppMsg,
) -> bool {
    let mut go_home = false;
    match msg {
        AppMsg::Brightness(percent) => hub.settings.set_brightness(percent),
        AppMsg::Lights(on) => hub.settings.lights = on,
        AppMsg::Haptics(on) => hub.settings.haptics = on,
        AppMsg::SettingsPage(page) => {
            router.set_settings_page(page);
            session.message = None;
            if page == crate::shell::pages::settings::model::Page::Proxy {
                session.proxy_address = hub
                    .settings
                    .update_proxy
                    .map(crate::services::update::HttpProxy::address)
                    .unwrap_or_default();
            }
        }
        AppMsg::SettingsBack => back(session, router),
        AppMsg::SelectSkin(id) => {
            if !session.info.busy
                && session.info.storage_ready
                && session.loading_skin.is_none()
                && id != session.selected_skin
            {
                session.loading_skin = Some(id.clone());
                session.message = None;
                session.action = Some(crate::services::settings::Command::SelectSkin(id));
            }
        }
        AppMsg::AudioVolume(value) => hub.settings.audio_volume = value.clamp(0, 100) as u8,
        AppMsg::AudioMute(on) => hub.settings.audio_muted = on,
        AppMsg::CameraMirror(on) => hub.settings.camera_mirror = on,
        AppMsg::JpegQuality(quality) => {
            if [75, 85, 95].contains(&quality) {
                hub.settings.jpeg_quality = quality;
            }
        }
        AppMsg::WifiScan => {
            session.action = Some(crate::services::settings::Command::Radio(
                crate::services::radio::Command::WifiScan,
            ))
        }
        AppMsg::Wifi(on) => {
            session.action = Some(crate::services::settings::Command::Radio(if on {
                crate::services::radio::Command::WifiOn
            } else {
                crate::services::radio::Command::WifiOff
            }))
        }
        AppMsg::Bluetooth(on) => {
            session.action = Some(crate::services::settings::Command::Radio(if on {
                crate::services::radio::Command::BtOn
            } else {
                crate::services::radio::Command::BtOff
            }))
        }
        AppMsg::ScreenOff => {
            go_home = true;
            session.action = Some(crate::services::settings::Command::ScreenOff);
        }
        AppMsg::WifiChoose(network) => {
            let known = session.wifi_saved.networks.contains(&network.ssid);
            session.selected = Some(network);
            session.password = crate::shell::pages::settings::model::Password::default();
            session.password_hidden = false;
            session.message = None;
            router.set_settings_page(if known {
                crate::shell::pages::settings::model::Page::KnownNetwork
            } else {
                crate::shell::pages::settings::model::Page::Password
            });
        }
        AppMsg::WifiAutomatic(on) => {
            session.action = Some(crate::services::settings::Command::Radio(
                crate::services::radio::Command::WifiAutomatic(on),
            ))
        }
        AppMsg::WifiReenterPassword => {
            session.password = crate::shell::pages::settings::model::Password::default();
            session.message = None;
            router.set_settings_page(crate::shell::pages::settings::model::Page::Password);
        }
        AppMsg::WifiSavedConnect | AppMsg::WifiForget => {
            if let Some(network) = &session.selected {
                let command = if matches!(msg, AppMsg::WifiForget) {
                    crate::services::radio::Command::WifiForget(network.ssid.clone())
                } else {
                    crate::services::radio::Command::WifiSavedConnect(network.ssid.clone())
                };
                session.action = Some(crate::services::settings::Command::Radio(command));
                session.password = crate::shell::pages::settings::model::Password::default();
                session.message = None;
                router.set_settings_page(crate::shell::pages::settings::model::Page::Wifi);
            }
        }
        AppMsg::WifiPassword(password) => {
            session.password = password;
            session.message = None;
        }
        AppMsg::WifiPasswordHidden(hidden) => session.password_hidden = hidden,
        AppMsg::AutoUpdateCheck(on) => hub.settings.auto_update_check = on,
        AppMsg::UpdateProxyToggle(on) => {
            if on && hub.settings.update_proxy.is_none() {
                router.set_settings_page(crate::shell::pages::settings::model::Page::Proxy);
                session.proxy_address.clear();
            } else {
                hub.settings.update_proxy_enabled = on;
            }
        }
        AppMsg::UpdateProxyText(text) => {
            session.proxy_address = text;
            session.message = None;
        }
        AppMsg::UpdateProxyApply => {
            match crate::services::update::HttpProxy::parse(&session.proxy_address) {
                Ok(proxy) => {
                    hub.settings.update_proxy = Some(proxy);
                    hub.settings.update_proxy_enabled = true;
                    router.set_settings_page(crate::shell::pages::settings::model::Page::Update);
                    session.message = None;
                }
                Err(error) => session.message = Some(error),
            }
        }
        AppMsg::UpdateProxySet(proxy) => {
            hub.settings.update_proxy_enabled = proxy.is_some();
            if let Some(proxy) = proxy {
                hub.settings.update_proxy = Some(proxy);
            }
        }
        AppMsg::Update(command) => {
            session.action = Some(crate::services::settings::Command::Update(command))
        }
        AppMsg::WifiConnect => {
            if let Some(network) = &session.selected {
                match crate::services::radio::Credentials::wpa2(
                    network.ssid.clone(),
                    session.password.0.as_bytes().to_vec(),
                ) {
                    Ok(credentials) => {
                        session.action = Some(crate::services::settings::Command::Radio(
                            crate::services::radio::Command::WifiConnect(credentials),
                        ));
                        session.password =
                            crate::shell::pages::settings::model::Password::default();
                        session.message = None;
                        router.set_settings_page(crate::shell::pages::settings::model::Page::Wifi);
                    }
                    Err(_) => session.message = Some("密码需为 8–63 个英文字符"),
                }
            }
        }
        _ => {}
    }
    go_home
}
