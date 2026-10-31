//! OTA command parsing and bounded binary-body metadata.
use super::{Effect, OtaBody, Session};
use alloc::{format, vec::Vec};

impl Session {
    pub(super) fn ota_command(&mut self, verb: &str, args: &[&str], out: &mut dyn FnMut(Effect)) {
        use crate::services::update::ota::Command;
        let result: core::result::Result<Option<Command>, &str> = match (verb, args) {
            ("ota-status", []) => Ok(Some(Command::Status)),
            ("ota-abort", []) => {
                self.ota_active = false;
                Ok(Some(Command::Abort))
            }
            ("ota-end", []) => Ok(Some(Command::End)),
            ("ota-begin", [size, sha]) => {
                if self.management_busy || self.staging.is_some() || self.upload.is_some() {
                    Err("other upload/management busy")
                } else {
                    match (
                        size.parse::<u32>(),
                        crate::services::update::ota::parse_sha(sha),
                    ) {
                        (Ok(size), Ok(sha)) => Ok(Some(Command::Begin { size, sha })),
                        _ => Err("invalid size/SHA256"),
                    }
                }
            }
            ("ota-chunk", _) => {
                let len = args.get(1).and_then(|s| s.parse::<usize>().ok());
                match len {
                    Some(len) if (1..=crate::services::update::ota::SECTOR).contains(&len) => {
                        let offset = args.first().and_then(|s| s.parse::<u32>().ok());
                        let crc = args
                            .get(2)
                            .filter(|s| s.len() == 8)
                            .and_then(|s| u32::from_str_radix(s, 16).ok());
                        self.ota_body = Some(OtaBody {
                            offset: offset.unwrap_or(0),
                            len,
                            crc: crc.unwrap_or(0),
                            bytes: Vec::with_capacity(len),
                            invalid: args.len() != 3 || offset.is_none() || crc.is_none(),
                        });
                        if args.len() != 3 || offset.is_none() || crc.is_none() {
                            self.ota_active = false;
                            out(Effect::OtaCancel);
                        }
                        Ok(None)
                    }
                    _ => {
                        self.ota_active = false;
                        self.ota_poisoned = true;
                        out(Effect::OtaCancel);
                        Err("invalid body length; reconnect required")
                    }
                }
            }
            _ => Err("invalid OTA command"),
        };
        match result {
            Ok(Some(cmd)) => out(Effect::Ota(cmd)),
            Ok(None) => {}
            Err(e) => out(Effect::Reply(format!("@err {verb}: {e}"))),
        }
    }
}
