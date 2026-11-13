//! One-boot CO5300 ownership handoff, carried in LP_SYS STORE12..STORE15.
//!
//! The bootloader clears the commit word on every entry, initializes and paints
//! the panel, then commits these words immediately before loading the selected
//! image. An application consumes the commit word once. This is volatile display
//! state, independent of the persistent boot journal and image authorization.

pub const FIRST_STORE: usize = 12;
pub const WORDS: usize = 4;
pub const MAGIC: u32 = 0x3144_4744; // "DGD1", CO5300 RGB565 / awake / 35% brightness.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handoff {
    /// Time from bootloader display setup entry to the visible static frame.
    pub first_frame_us: u32,
    /// Same clock origin, after image selection and before the image loader.
    pub selected_us: u32,
}

impl Handoff {
    pub fn decode(words: [u32; WORDS]) -> Option<Self> {
        let [magic, first_frame_us, selected_us, crc] = words;
        let mut body = [0; 12];
        for (target, value) in body.chunks_exact_mut(4).zip(words) {
            target.copy_from_slice(&value.to_le_bytes());
        }
        (magic == MAGIC
            && first_frame_us != 0
            && selected_us >= first_frame_us
            && crate::crc32(&body) == crc)
            .then_some(Self {
                first_frame_us,
                selected_us,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> [u32; WORDS] {
        let mut words = [MAGIC, 1_400_000, 2_000_000, 0];
        let body: [u8; 12] = core::array::from_fn(|i| words[i / 4].to_le_bytes()[i % 4]);
        words[3] = crate::crc32(&body);
        words
    }

    #[test]
    fn accepts_complete_record_and_rejects_each_corrupted_word() {
        let valid = record();
        assert_eq!(
            Handoff::decode(valid),
            Some(Handoff {
                first_frame_us: 1_400_000,
                selected_us: 2_000_000,
            })
        );
        assert_eq!(Handoff::decode([0; WORDS]), None);
        for index in 0..WORDS {
            let mut broken = valid;
            broken[index] ^= 1;
            assert_eq!(Handoff::decode(broken), None);
        }
        let mut consumed = valid;
        consumed[0] = 0;
        assert_eq!(Handoff::decode(consumed), None);
    }
}
