//! Bind optional RTC metadata to the panic text written by this firmware.
//! An older firmware only overwrites the original RTC record prefix, so its
//! panic must not inherit a build ID left in the extended record's tail.
use core::fmt::{self, Write};

const PREFIX: &str = "[escope:";
const SUFFIX: &str = "] ";

pub(crate) fn write_marker(output: &mut impl Write, build: &str) -> fmt::Result {
    output.write_str(PREFIX)?;
    output.write_str(build)?;
    output.write_str(SUFFIX)
}

pub(crate) fn matches_build(text: &[u8], build: &[u8]) -> bool {
    if build.is_empty() || build.len() > 64 || core::str::from_utf8(build).is_err() {
        return false;
    }
    text.strip_prefix(PREFIX.as_bytes())
        .and_then(|rest| rest.strip_prefix(build))
        .is_some_and(|rest| rest.starts_with(SUFFIX.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{format, string::String};

    #[test]
    fn marker_preserves_the_recorded_build_including_maximum_utf8_bytes() {
        let maximum = format!("{}x", "\u{754c}".repeat(21));
        assert_eq!(maximum.len(), 64);
        for build in ["dango-0.4.0+escope.old", maximum.as_str()] {
            let mut text = String::new();
            write_marker(&mut text, build).unwrap();
            text.push_str("Stack overflow detected @ mod.rs:533");
            assert!(matches_build(text.as_bytes(), build.as_bytes()));
            assert!(!matches_build(
                text.as_bytes(),
                b"the-current-running-build"
            ));
        }
    }

    #[test]
    fn official_panics_cannot_inherit_a_stale_build_tail() {
        let stale = b"dango-0.4.0+escope.old";
        assert!(!matches_build(
            b"Stack overflow detected @ mod.rs:533",
            stale
        ));
        assert!(!matches_build(
            b"[escope:dango-0.4.0+escope.other] panic",
            stale
        ));
        assert!(!matches_build(
            b"prefix [escope:dango-0.4.0+escope.old] panic",
            stale
        ));
    }

    #[test]
    fn incomplete_or_invalid_build_metadata_stays_unknown() {
        for text in [
            b"[escope:build".as_slice(),
            b"[escope:build]",
            b"[escope:build-more] panic",
        ] {
            assert!(!matches_build(text, b"build"));
        }
        assert!(!matches_build(b"[escope:] panic", b""));
        assert!(!matches_build(b"[escope:\xff] panic", b"\xff"));
        let oversized = "x".repeat(65);
        assert!(!matches_build(
            format!("[escope:{oversized}] panic").as_bytes(),
            oversized.as_bytes()
        ));
    }
}
