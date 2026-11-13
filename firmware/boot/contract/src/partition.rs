//! Fixed S31ONE03 table, checked without allocating before storage is writable.
use crate::{MAIN_BASE, MAIN_BYTES, RECOVERY_BASE, RECOVERY_BYTES, SYSTEM_BASE, SYSTEM_BYTES};
use md5::{Digest, Md5};

pub const EXPECTED: [(&str, u8, u8, u32, u32); 8] = [
    ("boot_state", 1, 0x40, 0x9000, 0x2000),
    ("phy_init", 1, 1, 0xb000, 0x1000),
    ("sysmeta", 1, 2, 0xc000, 0x14000),
    ("recovery_app", 0, 0, RECOVERY_BASE, RECOVERY_BYTES),
    ("coredump", 1, 3, 0x1e0000, 0x20000),
    ("nvs", 1, 2, 0x200000, 0x6000),
    ("main_app", 0, 0x10, MAIN_BASE, MAIN_BYTES),
    ("system", 1, 0x83, SYSTEM_BASE, SYSTEM_BYTES),
];
pub fn verify(table: &[u8]) -> bool {
    if table.len() < (EXPECTED.len() + 1) * 32 || table.len() % 32 != 0 {
        return false;
    }
    for (entry, &(name, kind, subtype, offset, bytes)) in
        table.chunks_exact(32).zip(EXPECTED.iter())
    {
        let word = |i| u32::from_le_bytes(entry[i..i + 4].try_into().unwrap());
        if entry[..4] != [0xaa, 0x50, kind, subtype]
            || word(4) != offset
            || word(8) != bytes
            || word(28) != 0
            || &entry[12..12 + name.len()] != name.as_bytes()
            || entry[12 + name.len()..28].iter().any(|&v| v != 0)
        {
            return false;
        }
    }
    let at = EXPECTED.len() * 32;
    table[at..at + 16]
        == [
            0xeb, 0xeb, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff,
        ]
        && Md5::digest(&table[..at])[..] == table[at + 16..at + 32]
        && table[at + 32..].iter().all(|&v| v == 255)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    fn table() -> std::vec::Vec<u8> {
        let mut b = std::vec![255; 3072];
        for (i, &(label, kind, subtype, offset, bytes)) in EXPECTED.iter().enumerate() {
            let e = &mut b[i * 32..i * 32 + 32];
            e.fill(0);
            e[..4].copy_from_slice(&[0xaa, 0x50, kind, subtype]);
            e[4..8].copy_from_slice(&offset.to_le_bytes());
            e[8..12].copy_from_slice(&bytes.to_le_bytes());
            e[12..12 + label.len()].copy_from_slice(label.as_bytes());
        }
        let at = EXPECTED.len() * 32;
        let digest = Md5::digest(&b[..at]);
        b[at..at + 2].copy_from_slice(&[0xeb, 0xeb]);
        b[at + 16..at + 32].copy_from_slice(&digest);
        b
    }
    #[test]
    fn only_exact_geometry_and_digest_admit_writes() {
        let bytes = table();
        assert!(verify(&bytes));
        for i in 0..bytes.len() {
            let mut broken = bytes.clone();
            broken[i] ^= 1;
            assert!(!verify(&broken), "byte {i}");
        }
        assert!(!verify(&bytes[..256]));
        // A correct checksum cannot legitimize the former two-slot geometry.
        let mut old = bytes.clone();
        old[6 * 32 + 8..6 * 32 + 12].copy_from_slice(&0x600000u32.to_le_bytes());
        let digest = Md5::digest(&old[..256]);
        old[272..288].copy_from_slice(&digest);
        assert!(!verify(&old));
    }
}
