//! Ordered device-effect dispatch after the protocol releases its VFS guard.
use super::logging::{reboot_reply_sent, reply};
use crate::apps::device as apps;
use crate::console::protocol::{Effect, Session};
use crate::services::update::ota::device::DeviceUpdater;
use alloc::vec::Vec;
use embassy_time::Timer;
use log::{info, warn};

pub(super) async fn apply(effects: Vec<Effect>, session: &mut Session, ota: &mut DeviceUpdater) {
    for effect in effects {
        let passive = matches!(
            effect,
            Effect::Power(_) | Effect::NandInfo | Effect::NandStatus
        ) || matches!(
            &effect,
            Effect::Audio(crate::services::audio::Command::Status)
        ) || matches!(&effect, Effect::Radio(command) if command.status())
            || matches!(
                &effect,
                Effect::Update(crate::services::update::Command::Status)
            );
        if !passive {
            crate::services::power::coordinator::wake();
        }
        match effect {
            Effect::Audio(command) => {
                reply(&crate::services::audio::task::execute(command).await).await
            }
            Effect::Camera(command) => {
                reply(&crate::services::camera::task::execute(command).await).await
            }
            Effect::Radio(command) => {
                reply(&crate::services::radio::device::execute(command).await).await
            }
            Effect::Update(command) => {
                let response = if command == crate::services::update::Command::Status {
                    crate::services::update::task::status()
                } else if crate::services::update::task::submit(command) {
                    "@ok update queued".into()
                } else {
                    "@err update busy or unavailable".into()
                };
                reply(&response).await;
            }
            Effect::NandStatus => reply(&crate::platform::storage::nand::status()).await,
            Effect::NandFormat => {
                let result = if crate::services::update::ota::device::active() {
                    Err(::fs::Errno::BUSY)
                } else if !crate::services::power::coordinator::ensure_awake().await {
                    Err(::fs::Errno::IO)
                } else {
                    crate::platform::storage::nand::format_volume().await
                };
                let text = match result {
                    Ok(()) => {
                        crate::boot::storage::finish_data_init();
                        crate::apps::device::request(true, None);
                        "@ok nand-format mounted=1".into()
                    }
                    Err(error) => alloc::format!("@err nand-format {error}"),
                };
                reply(&text).await;
            }

            Effect::NandInfo => {
                let text = match crate::platform::drivers::nand::identify().await {
                    Ok(id) => {
                        let profile = if let Some(model) = id.model() {
                            use crate::platform::drivers::nand;
                            alloc::format!(
                                "model={model} geometry={}+{}x{}x{} ecc=8/512",
                                nand::DATA_BYTES,
                                nand::SPARE_BYTES,
                                nand::PAGES_PER_BLOCK,
                                nand::BLOCKS
                            )
                        } else {
                            alloc::string::String::from(
                                "model=unknown geometry=unknown ecc=unknown",
                            )
                        };
                        alloc::format!(
                            "@ok nand-info id={:02x}{:02x}{:02x}{:02x} a0={:02x} b0={:02x} c0={:02x} {profile} readonly=1",
                            id.id[0],
                            id.id[1],
                            id.id[2],
                            id.id[3],
                            id.protection,
                            id.configuration,
                            id.status
                        )
                    }
                    Err(error) => alloc::format!("@err nand-info {}", error.name()),
                };
                reply(&text).await;
            }

            Effect::Power(command) => {
                reply(&crate::services::power::coordinator::execute(command).await).await
            }

            Effect::OtaCancel => {
                ota.abort();
                session.set_ota_active(false);
            }
            Effect::Ota(command) => {
                if !matches!(command, crate::services::update::ota::Command::Status)
                    && crate::services::update::task::busy()
                {
                    reply("@err OTA network update busy").await;
                    continue;
                }
                if matches!(command, crate::services::update::ota::Command::Begin { .. }) {
                    if let Err(error) = ota.reserve() {
                        reply(&alloc::format!("@err ota-begin: {error}")).await;
                        continue;
                    }
                }
                if matches!(command, crate::services::update::ota::Command::Begin { .. })
                    && !crate::services::camera::task::quiesce().await
                {
                    ota.abort();
                    reply("@err OTA camera shutdown failed").await;
                    continue;
                }
                if matches!(command, crate::services::update::ota::Command::Begin { .. })
                    && !crate::services::radio::device::quiesce().await
                {
                    ota.abort();
                    reply("@err OTA wireless shutdown failed").await;
                    continue;
                }
                if !crate::services::power::coordinator::ensure_awake().await {
                    ota.abort();
                    reply("@err OTA display restore failed").await;
                    continue;
                }
                let result = ota.execute(command).await;
                session.set_ota_active(crate::services::update::ota::device::active());
                match result {
                    Ok((text, reboot)) => {
                        if reboot {
                            if reboot_reply_sent(&text).await
                                && !crate::services::power::button_task::shutdown_requested()
                            {
                                esp_hal::system::software_reset();
                            } else {
                                // The candidate is selected, but no successful ACK
                                // delivery is established: leave reboot to the operator.
                                warn!(
                                    "ota: reboot ACK delivery timed out; candidate selected, automatic reset withheld"
                                );
                                ota.abort();
                                session.set_ota_active(false);
                            }
                        } else {
                            reply(&text).await;
                        }
                    }
                    Err(text) => reply(&text).await,
                }
            }
            Effect::Manage(op) => {
                let id = op.identity();
                let result = apps::manage(op).await;
                let text = match result {
                    Ok(()) => alloc::format!("@ok {} {}", op.verb(), id.id()),
                    Err(e) => alloc::format!("@err {} {}: {e}", op.verb(), id.id()),
                };
                reply(&text).await;
            }
            Effect::Reply(text) => reply(&text).await,
            Effect::Reload { run } => {
                info!(
                    "push: reload{}",
                    run.as_deref()
                        .map(|id| alloc::format!(", run {id}"))
                        .unwrap_or_default()
                );
                apps::request(true, run.map(apps::Action::Run));
            }
            Effect::Run(id) => apps::request(false, Some(apps::Action::Run(id))),
            Effect::Stop => apps::request(false, Some(apps::Action::Stop)),
            Effect::SkinsChanged => crate::companion::skins::device::request_reload(),
            Effect::Brain(action) => {
                let verb = action.verb();
                let text = match crate::companion::brain::control::execute(action).await {
                    Ok(status) => alloc::format!("@ok {verb} {status}"),
                    Err(e) => alloc::format!(
                        "@err {verb}: {}",
                        crate::companion::brain::store::error_line(&e)
                    ),
                };
                reply(text.trim_end()).await;
            }

            Effect::Reboot => {
                if crate::services::power::button_task::shutdown_requested() {
                    reply("@err reboot shutdown pending").await;
                    continue;
                }
                warn!("push: reboot requested");
                // Let the reply and the log line go out first.
                Timer::after_millis(300).await;
                if !crate::services::power::button_task::shutdown_requested() {
                    esp_hal::system::software_reset();
                }
            }
        }
    }
}
