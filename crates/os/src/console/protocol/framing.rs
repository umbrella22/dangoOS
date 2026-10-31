//! Command-line framing, binary-body boundaries and deferred-effect cutoffs.
use super::{Effect, Session, LINE_MAX, PREFIX};
use ::fs::Vfs;

impl Session {
    /// Feed bytes from the host; effects come out in order.
    pub fn feed(&mut self, vfs: &mut Vfs, mut data: &[u8], out: &mut dyn FnMut(Effect)) {
        while !data.is_empty() {
            let consumed = self.feed_until_deferred(vfs, data, out);
            data = &data[consumed..];
        }
    }

    /// Consume whole payload chunks, but stop exactly after a deferred
    /// management command. Execute it before feeding the unconsumed tail.
    pub fn feed_until_deferred(
        &mut self,
        vfs: &mut Vfs,
        mut data: &[u8],
        out: &mut dyn FnMut(Effect),
    ) -> usize {
        let original_len = data.len();
        while !data.is_empty() {
            if self.ota_poisoned {
                return original_len;
            }
            if let Some(body) = self.ota_body.as_mut() {
                let n = (body.len - body.bytes.len()).min(data.len());
                body.bytes.extend_from_slice(&data[..n]);
                data = &data[n..];
                if body.bytes.len() == body.len {
                    let body = self.ota_body.take().unwrap();
                    if body.invalid {
                        out(Effect::Reply(
                            "@err ota-chunk invalid header; body discarded".into(),
                        ));
                    } else {
                        out(Effect::Ota(crate::services::update::ota::Command::Chunk {
                            offset: body.offset,
                            bytes: body.bytes,
                            crc: body.crc,
                        }));
                    }
                    return original_len - data.len();
                }
                continue;
            }
            if self.upload.is_some() {
                data = self.feed_upload(vfs, data, out);
                continue;
            }
            match data.iter().position(|&b| b == b'\n') {
                Some(n) => {
                    self.push_line(&data[..n]);
                    data = &data[n + 1..];
                    let line = core::mem::take(&mut self.line);
                    let overflow = core::mem::replace(&mut self.overflow, false);
                    if overflow || core::str::from_utf8(&line).is_err() {
                        // The discarded prefix may have been a binary-body header.
                        // Its length is unknowable; only reconnect can restore framing.
                        self.ota_active = false;
                        self.ota_poisoned = true;
                        out(Effect::OtaCancel);
                        out(Effect::Reply(if overflow {
                            "@err line too long; reconnect required".into()
                        } else {
                            "@err invalid UTF-8 control line; reconnect required".into()
                        }));
                        return original_len - data.len();
                    } else if let Ok(text) = core::str::from_utf8(&line) {
                        let text = text.trim_end_matches('\r');
                        if let Some(cmd) = text.strip_prefix(PREFIX) {
                            let mut deferred = false;
                            self.command(vfs, cmd.trim(), &mut |effect| {
                                deferred |= matches!(
                                    effect,
                                    Effect::Manage(_)
                                        | Effect::Brain(_)
                                        | Effect::Ota(_)
                                        | Effect::OtaCancel
                                        | Effect::NandInfo
                                        | Effect::NandStatus
                                        | Effect::NandFormat
                                        | Effect::Audio(_)
                                        | Effect::Radio(_)
                                        | Effect::Update(_)
                                        | Effect::Camera(_)
                                        | Effect::Power(_)
                                );
                                out(effect);
                            });
                            if deferred {
                                return original_len - data.len();
                            }
                            if self.upload.as_ref().is_some_and(|u| u.left == 0) {
                                // An empty file: no payload follows.
                                self.feed_upload(vfs, &[], out);
                            }
                        }
                    }
                }
                None => {
                    self.push_line(data);
                    data = &[];
                }
            }
        }
        original_len
    }

    fn push_line(&mut self, bytes: &[u8]) {
        if self.line.len() + bytes.len() > LINE_MAX {
            self.overflow = true;
        } else if !self.overflow {
            self.line.extend_from_slice(bytes);
        }
    }
}
