//! Protocol command parsing and passive filesystem responses.
use super::{Effect, Session, UploadKind, DATA_APPS, VERSION};
use crate::apps::registry::id_is_valid;
use ::fs::{Errno, Kind, Vfs};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};

impl Session {
    pub(super) fn command(&mut self, vfs: &mut Vfs, cmd: &str, out: &mut dyn FnMut(Effect)) {
        let mut words = cmd.split_ascii_whitespace();
        let verb = words.next().unwrap_or("");
        let args: Vec<&str> = words.collect();
        if self.update_active
            && !matches!(
                verb,
                "hello"
                    | "power-status"
                    | "wifi-status"
                    | "wifi-saved"
                    | "bt-status"
                    | "update-status"
                    | "boot-status"
                    | "update-cancel"
            )
        {
            out(Effect::Reply("@err update busy".into()));
            return;
        }
        if self.management_busy
            && matches!(
                verb,
                "run"
                    | "begin"
                    | "file"
                    | "commit"
                    | "rm"
                    | "rmdata"
                    | "photo-begin"
                    | "photo-commit"
            )
        {
            out(Effect::Reply("@err app management busy".into()));
            return;
        }
        let result = match (verb, args.as_slice()) {
            ("boot-status", []) => {
                out(Effect::BootStatus);
                return;
            }
            ("recovery", []) => {
                out(Effect::Recovery);
                return;
            }
            ("hello", []) => Ok(format!("hello mosaico-push {VERSION}")),
            (verb, args) if verb.starts_with("agent-") => {
                match crate::services::voice::desktop::Command::parse(verb, args) {
                    Ok(command) => out(Effect::Voice(crate::services::voice::Command::Desktop(
                        command,
                    ))),
                    Err(error) => out(Effect::Reply(format!("@err {error}"))),
                }
                return;
            }
            ("voice-ask", [text]) => {
                if self.staging.is_some() || self.management_busy {
                    out(Effect::Reply(
                        "@err voice transfer or management busy".into(),
                    ));
                } else {
                    match crate::services::voice::Command::ask(text) {
                        Ok(command) => out(Effect::Voice(command)),
                        Err(error) => out(Effect::Reply(format!("@err {error}"))),
                    }
                }
                return;
            }
            (verb @ ("voice-read" | "voice-page"), [offset]) => {
                match offset.parse() {
                    Ok(offset) => out(Effect::Voice(if verb == "voice-read" {
                        crate::services::voice::Command::Read(offset)
                    } else {
                        crate::services::voice::Command::Page(offset)
                    })),
                    Err(_) => out(Effect::Reply("@err invalid voice reply offset".into())),
                }
                return;
            }
            (verb @ ("voice-toggle" | "voice-cancel" | "voice-status" | "voice-reload"), []) => {
                if verb == "voice-toggle" && (self.staging.is_some() || self.management_busy) {
                    out(Effect::Reply(
                        "@err voice transfer or management busy".into(),
                    ));
                } else {
                    out(Effect::Voice(
                        crate::services::voice::Command::parse(verb).unwrap(),
                    ));
                }
                return;
            }
            ("voice-config-begin", []) => self
                .begin(vfs, "voice", UploadKind::VoiceConfig)
                .map(|_| "voice-config-begin".into()),
            ("voice-config-commit", [])
                if self.kind == UploadKind::VoiceConfig
                    && self.completed_file.as_deref() == Some("config.json") =>
            {
                crate::services::voice::config::commit(vfs)
                    .map(|()| {
                        self.staging = None;
                        self.completed_file = None;
                        let _ = vfs.remove_tree(self.staging_root());
                        "voice-config-commit next-turn=1".into()
                    })
                    .map_err(String::from)
            }
            ("voice-font-begin", []) => self
                .begin(vfs, "font", UploadKind::VoiceFont)
                .map(|_| "voice-font-begin".into()),
            ("voice-font-commit", [])
                if self.kind == UploadKind::VoiceFont
                    && self.completed_file.as_deref() == Some("font.bin") =>
            {
                crate::services::resources::voice_font::commit(vfs)
                    .map(|()| {
                        self.staging = None;
                        self.completed_file = None;
                        let _ = vfs.remove_tree(self.staging_root());
                        "voice-font-commit restart-if-loaded=1".into()
                    })
                    .map_err(String::from)
            }

            (
                verb @ ("update-status" | "update-check" | "update-install" | "update-cancel"),
                [],
            ) => {
                let command = match verb {
                    "update-status" => crate::services::update::Command::Status,
                    "update-check" => crate::services::update::Command::Check,
                    "update-install" => crate::services::update::Command::Install,
                    _ => crate::services::update::Command::Cancel,
                };
                if command != crate::services::update::Command::Status
                    && (self.staging.is_some() || self.management_busy)
                {
                    out(Effect::Reply(
                        "@err update transfer or management busy".into(),
                    ));
                } else {
                    out(Effect::Update(command));
                }
                return;
            }
            ("update-url", [url]) => {
                if self.staging.is_some() || self.management_busy {
                    out(Effect::Reply(
                        "@err update transfer or management busy".into(),
                    ));
                } else {
                    match crate::services::update::Url::parse(url) {
                        Ok(url) => out(Effect::Update(
                            crate::services::update::Command::InstallUrl(url),
                        )),
                        Err(error) => out(Effect::Reply(format!("@err update-url {error}"))),
                    }
                }
                return;
            }
            ("update-proxy", [address]) => {
                if self.staging.is_some() || self.management_busy {
                    out(Effect::Reply(
                        "@err update transfer or management busy".into(),
                    ));
                } else if *address == "off" {
                    out(Effect::Update(crate::services::update::Command::SetProxy(
                        None,
                    )));
                } else {
                    match crate::services::update::HttpProxy::parse(address) {
                        Ok(proxy) => out(Effect::Update(
                            crate::services::update::Command::SetProxy(Some(proxy)),
                        )),
                        Err(error) => out(Effect::Reply(format!("@err update-proxy {error}"))),
                    }
                }
                return;
            }
            (verb @ ("camera-status" | "camera-start" | "camera-stop" | "camera-capture"), _) => {
                match crate::services::camera::Command::parse(verb, &args) {
                    Ok(command)
                        if command != crate::services::camera::Command::Status
                            && (self.staging.is_some() || self.management_busy) =>
                    {
                        out(Effect::Reply(
                            "@err camera transfer or management busy".into(),
                        ))
                    }
                    Ok(command) => out(Effect::Camera(command)),
                    Err(error) => out(Effect::Reply(format!("@err {error}"))),
                }
                return;
            }

            (verb @ ("screen-off" | "screen-on" | "power-status"), _) => {
                match crate::services::power::standby::Command::parse(verb, &args) {
                    Ok(crate::services::power::standby::Command::ScreenOff(_))
                        if self.staging.is_some() || self.management_busy =>
                    {
                        out(Effect::Reply(
                            "@err screen-off transfer or management busy".into(),
                        ))
                    }
                    Ok(command) => out(Effect::Power(command)),
                    Err(error) => out(Effect::Reply(format!("@err {error}"))),
                }
                return;
            }
            (
                "audio-status" | "audio-stop" | "audio-play" | "audio-record" | "audio-volume"
                | "audio-mute",
                _,
            ) => {
                match crate::services::audio::Command::parse(verb, &args) {
                    Ok(command)
                        if command != crate::services::audio::Command::Status
                            && (self.staging.is_some() || self.management_busy) =>
                    {
                        out(Effect::Reply(
                            "@err audio transfer or management busy".into(),
                        ))
                    }
                    Ok(command) => out(Effect::Audio(command)),
                    Err(error) => out(Effect::Reply(format!("@err audio {error}"))),
                }
                return;
            }
            (
                "wifi-status" | "wifi-on" | "wifi-scan" | "wifi-connect" | "wifi-disconnect"
                | "wifi-off" | "wifi-saved" | "wifi-saved-connect" | "wifi-forget" | "wifi-auto"
                | "bt-status" | "bt-on" | "bt-off" | "bt-restart",
                _,
            ) => {
                let result = crate::services::radio::Command::parse(verb, &args);
                match result {
                    Ok(command)
                        if !command.status()
                            && (self.staging.is_some() || self.management_busy) =>
                    {
                        out(Effect::Reply(
                            "@err wireless transfer or management busy".into(),
                        ))
                    }
                    Ok(command) => out(Effect::Radio(command)),
                    Err(error) => out(Effect::Reply(format!("@err wireless {error}"))),
                }
                return;
            }
            ("nand-status", []) => {
                out(Effect::NandStatus);
                return;
            }
            ("nand-format", []) => {
                if self.staging.is_some() || self.management_busy {
                    out(Effect::Reply(
                        "@err nand-format transfer or management busy".into(),
                    ));
                } else {
                    out(Effect::NandFormat);
                }
                return;
            }
            ("nand-info", []) => {
                if self.staging.is_some() || self.management_busy {
                    out(Effect::Reply(
                        "@err nand-info transfer or management busy".into(),
                    ));
                } else {
                    out(Effect::NandInfo);
                }
                return;
            }
            ("begin", [id]) => self.begin(vfs, id, UploadKind::App),
            ("skin-begin", [id]) => self.begin(vfs, id, UploadKind::Skin),
            ("fonts-begin", []) => self.begin(vfs, "fonts", UploadKind::Fonts),
            ("fonts-commit", [])
                if self.kind == UploadKind::Fonts
                    && self.staging.as_deref() == Some("fonts")
                    && self.completed_file.as_deref() == Some("fonts.bin") =>
            {
                crate::services::resources::fonts::commit(vfs).map(|()| {
                    self.staging = None;
                    self.completed_file = None;
                    "fonts-commit restart-required=1".into()
                })
            }
            ("fonts-status", []) => {
                use crate::services::resources::fonts;
                Ok(format!(
                    "fonts-status expected={} bytes={} present={} contract={}",
                    fonts::ID,
                    fonts::BYTES,
                    u8::from(
                        vfs.stat(fonts::DATA_PATH).is_ok() || vfs.stat(fonts::SYSTEM_PATH).is_ok()
                    ),
                    fonts::STAMP
                ))
            }
            ("photo-begin", []) => self
                .begin(vfs, "photo", UploadKind::Photo)
                .map(|_| "photo-begin".into()),
            ("photo-commit", [rel]) => self.commit_photo(vfs, rel),
            ("brain-begin", []) => self
                .begin(vfs, "resident", UploadKind::Brain)
                .map(|_| "brain-begin".into()),
            ("brain-commit", [])
                if self.kind == UploadKind::Brain
                    && self.staging.as_deref() == Some("resident") =>
            {
                match crate::companion::brain::store::read_source(
                    vfs,
                    crate::companion::brain::store::STAGED_SOURCE,
                ) {
                    Ok(source) => {
                        self.staging = None;
                        let _ = vfs.remove_tree(self.staging_root());
                        out(Effect::Brain(
                            crate::companion::brain::store::Action::Install(source),
                        ));
                        return;
                    }
                    Err(e) => Err(e),
                }
            }
            (verb @ ("brain-reload" | "brain-default" | "brain-status"), []) => {
                use crate::companion::brain::store::Action;
                let action = match verb {
                    "brain-reload" => Action::Reload,
                    "brain-default" => Action::Default,
                    _ => Action::Status,
                };
                out(Effect::Brain(action));
                return;
            }
            ("skin-commit", [id])
                if self.kind == UploadKind::Skin && self.staging.as_deref() == Some(id) =>
            {
                match crate::companion::skins::commit(vfs, id) {
                    Ok(()) => {
                        self.staging = None;
                        out(Effect::SkinsChanged);
                        Ok(format!("skin-commit {id}"))
                    }
                    Err(e) => Err(e),
                }
            }
            ("skin-select", [id]) => crate::companion::skins::select(vfs, id).map(|()| {
                out(Effect::SkinsChanged);
                format!("skin-select {id}")
            }),
            ("skin-rm", [id]) => {
                let result = crate::companion::skins::remove(vfs, id);
                out(Effect::SkinsChanged);
                result.map(|()| format!("skin-rm {id}"))
            }

            ("skin-ls", []) => crate::companion::skins::list(vfs),
            ("file", [rel, size, crc]) => match self.file(vfs, rel, size, crc) {
                // The host sends the payload only after this; the final
                // reply follows it (see `feed_upload`).
                Ok(()) => Ok(format!("ready {rel}")),
                Err(text) => Err(text),
            },
            ("commit", [id]) => self.commit(vfs, id, false, out),
            ("commit", [id, "run"]) => self.commit(vfs, id, true, out),
            ("abort", []) => {
                self.completed_file = None;
                if self.staging.take().is_some() {
                    let _ = vfs.remove_tree(self.staging_root());
                }
                Ok("abort".into())
            }
            ("run", [id]) if id_is_valid(id) => {
                out(Effect::Run(id.to_string()));
                Ok(format!("run {id}"))
            }
            ("stop", []) => {
                out(Effect::Stop);
                Ok("stop".into())
            }
            ("reload", []) => {
                out(Effect::Reload { run: None });
                Ok("reload".into())
            }
            (verb @ ("rm" | "rmdata"), [id]) if id_is_valid(id) => {
                match crate::apps::lifecycle::uninstall::resolve(vfs, id) {
                    Ok(identity) => {
                        // Invalidate in-memory staging as well as the on-disk
                        // incoming tree, preventing a stale file/commit resurrection.
                        if (self.kind == UploadKind::App && self.staging.as_deref() == Some(id))
                            || (self.kind == UploadKind::Photo && *id == "photo")
                        {
                            self.staging = None;
                            self.completed_file = None;
                        }
                        let op = if verb == "rm" {
                            crate::apps::lifecycle::uninstall::Operation::Remove(identity)
                        } else {
                            crate::apps::lifecycle::uninstall::Operation::Clear(identity)
                        };
                        out(Effect::Manage(op));
                        return; // completion reply belongs to the executor
                    }
                    Err(e) => Err(e),
                }
            }
            ("ls", []) => list(vfs),
            ("status", []) => Ok(status(vfs)),
            ("reboot", []) => {
                out(Effect::Reboot);
                Ok("reboot".into())
            }
            _ => Err(format!("bad command: {cmd}")),
        };
        out(Effect::Reply(match result {
            Ok(text) => format!("@ok {text}"),
            Err(text) => format!("@err {text}"),
        }));
    }
}

fn status(vfs: &mut Vfs) -> String {
    let names: Vec<String> = vfs.mount_names().map(String::from).collect();
    let mut text = String::from("status");
    if names.is_empty() {
        text.push_str(" no-mounts");
    }
    for name in names {
        match vfs.space(&format!("/{name}")) {
            Ok(sp) => text.push_str(&format!(
                " {name}:{}/{}x{}",
                sp.free, sp.blocks, sp.block_size
            )),
            Err(e) => text.push_str(&format!(" {name}:{e}")),
        }
    }
    text
}

fn list(vfs: &mut Vfs) -> Result<String, String> {
    let mut ids: Vec<String> = match vfs.read_dir(DATA_APPS) {
        Ok(entries) => entries
            .into_iter()
            .filter(|e| e.meta.kind == Kind::Dir && !e.name.starts_with('.'))
            .map(|e| e.name)
            .collect(),
        Err(Errno::NOENT) => Vec::new(),
        Err(e) => return Err(format!("ls: {e}")),
    };
    ids.sort();
    let mut text = String::from("ls");
    for id in ids {
        text.push(' ');
        text.push_str(&id);
    }
    Ok(text)
}
