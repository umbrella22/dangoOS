use boot_contract::{self as boot, Flash, State, Target};
use recovery::install::{Error, Installer, Manifest, Preflight};
use sha2::{Digest, Sha256};

struct Nor {
    bytes: Vec<u8>,
    fail_program: bool,
}
impl Nor {
    fn new() -> Self {
        Self {
            bytes: vec![255; 0x1000000],
            fail_program: false,
        }
    }
}
impl Flash for Nor {
    type Error = ();
    fn read(&mut self, a: u32, b: &mut [u8]) -> Result<(), ()> {
        b.copy_from_slice(&self.bytes[a as usize..a as usize + b.len()]);
        Ok(())
    }
    fn erase(&mut self, a: u32) -> Result<(), ()> {
        assert!(
            boot::JOURNAL.contains(&a)
                || (boot::MAIN_BASE..boot::MAIN_BASE + boot::MAIN_BYTES).contains(&a)
        );
        assert_eq!(a % 4096, 0);
        self.bytes[a as usize..a as usize + 4096].fill(255);
        Ok(())
    }
    fn program(&mut self, a: u32, b: &[u8]) -> Result<(), ()> {
        if self.fail_program {
            return Err(());
        }
        for (i, &v) in b.iter().enumerate() {
            self.bytes[a as usize + i] &= v;
        }
        Ok(())
    }
}
fn image() -> Vec<u8> {
    // Structurally valid S31 image; these bytes are never sent to a device.
    let mut header = vec![0; 24];
    header[..4].copy_from_slice(&[0xe9, 1, 2, 0x40]);
    header[23] = 1;
    header[4..8].copy_from_slice(&0x40000020u32.to_le_bytes());
    header[12..14].copy_from_slice(&32u16.to_le_bytes());
    let mut segment = vec![0; 12000];
    segment[..4].copy_from_slice(&0xabcd5432u32.to_le_bytes());
    segment[16..21].copy_from_slice(b"0.4.4");
    segment[48..59].copy_from_slice(b"mosaico-s31");
    let marker = boot::image::MAIN_MARKER;
    segment[512..512 + marker.len()].copy_from_slice(marker);
    header.extend_from_slice(&0x40000020u32.to_le_bytes());
    header.extend_from_slice(&(segment.len() as u32).to_le_bytes());
    let checksum = segment.iter().fold(0xef, |s, b| s ^ b);
    header.extend_from_slice(&segment);
    let checksum_at = (header.len() / 16 + 1) * 16 - 1;
    header.resize(checksum_at, 0);
    header.push(checksum);
    let sha = Sha256::digest(&header);
    header.extend_from_slice(&sha);
    header
}
fn manifest(b: &[u8]) -> Manifest {
    Manifest {
        layout: *boot::LAYOUT,
        bytes: b.len() as u32,
        sha256: Sha256::digest(b).into(),
    }
}
#[test]
fn partial_bad_hash_and_corrupt_readback_remain_in_recovery() {
    let image = image();
    for fault in 0..3 {
        let mut nor = Nor::new();
        let mut m = manifest(&image);
        if fault == 1 {
            m.sha256 = [3; 32];
        }
        let mut installer = Installer::begin(&mut nor, m).unwrap();
        let length = if fault == 0 { 4096 } else { image.len() };
        for (offset, chunk) in image[..length].chunks(4096).enumerate() {
            installer
                .push(&mut nor, offset as u32 * 4096, chunk)
                .unwrap();
        }
        if fault == 2 {
            nor.bytes[boot::MAIN_BASE as usize + 35] ^= 1;
        }
        assert!(installer.finish(&mut nor).is_err());
        let (_, record) = boot::read(&mut nor).unwrap().unwrap();
        assert_eq!(record.state, State::Installing);
        assert_eq!(boot::target(Some(record), false, true), Target::Recovery);
    }
}
#[test]
fn valid_complete_install_is_one_trial_and_never_touches_recovery_or_system() {
    let image = image();
    let mut nor = Nor::new();
    nor.bytes[..boot::MAIN_BASE as usize].fill(0xa5);
    // The initial journal is blank; firmware and configuration bytes are not.
    nor.bytes[0x9000..0xb000].fill(255);
    let before = nor.bytes[..boot::MAIN_BASE as usize].to_vec();
    let mut installer = Installer::begin(&mut nor, manifest(&image)).unwrap();
    assert_eq!(
        installer.push(&mut nor, 4, &image[..4096]),
        Err(Error::Offset)
    );
    for (offset, chunk) in image.chunks(4096).enumerate() {
        let offset = offset as u32 * 4096;
        installer.push(&mut nor, offset, chunk).unwrap();
        installer.push(&mut nor, offset, chunk).unwrap();
    }
    let trial = installer.finish(&mut nor).unwrap();
    assert_eq!(trial.state, State::Trial);
    assert_eq!(boot::target(Some(trial), false, true), Target::ConsumeTrial);
    assert_eq!(
        &nor.bytes[boot::MAIN_BASE as usize..boot::MAIN_BASE as usize + image.len()],
        &image
    );
    assert_eq!(&nor.bytes[..0x9000], &before[..0x9000]);
    assert_eq!(
        &nor.bytes[0xb000..boot::MAIN_BASE as usize],
        &before[0xb000..]
    );
    assert!(nor.bytes[boot::SYSTEM_BASE as usize..]
        .iter()
        .all(|&b| b == 255));
}
#[test]
fn bad_target_or_capacity_is_rejected_before_any_erase() {
    let mut nor = Nor::new();
    let before = nor.bytes.clone();
    let mut m = manifest(&image());
    m.layout = *b"OLDAB002";
    assert!(matches!(
        Installer::begin(&mut nor, m),
        Err(Error::Manifest)
    ));
    m.layout = *boot::LAYOUT;
    m.bytes = boot::MAIN_BYTES + 1;
    assert!(matches!(
        Installer::begin(&mut nor, m),
        Err(Error::Manifest)
    ));
    assert_eq!(nor.bytes, before);
}

#[test]
fn a_foreign_chip_or_bad_image_never_becomes_trial_even_with_matching_manifest_hash() {
    for offset in [12, 32, 80, 12047] {
        let mut data = image();
        data[offset] ^= 1;
        let mut nor = Nor::new();
        let mut installer = Installer::begin(&mut nor, manifest(&data)).unwrap();
        let mut rejected = false;
        for (i, chunk) in data.chunks(4096).enumerate() {
            if installer.push(&mut nor, (i * 4096) as u32, chunk).is_err() {
                rejected = true;
                break;
            }
        }
        assert!(rejected || installer.finish(&mut nor).is_err());
        assert_eq!(
            boot::read(&mut nor).unwrap().unwrap().1.state,
            State::Installing
        );
    }
}

#[test]
fn staged_preflight_validates_the_complete_package_before_main_can_be_erased() {
    let original = image();
    let mut check = Preflight::new::<()>(manifest(&original)).unwrap();
    for bytes in original.chunks(4096) {
        check.feed::<()>(bytes).unwrap();
    }
    assert_eq!(check.finish::<()>(), Ok(manifest(&original)));
    let mut short = Preflight::new::<()>(manifest(&original)).unwrap();
    short.feed::<()>(&original[..4096]).unwrap();
    assert_eq!(short.finish::<()>(), Err(Error::Length));
    // A transport checksum for malformed contents cannot make them bootable.
    let mut corrupt = original.clone();
    corrupt[350] ^= 1;
    let mut check = Preflight::new::<()>(manifest(&corrupt)).unwrap();
    let mut rejected = false;
    for bytes in corrupt.chunks(4096) {
        if check.feed::<()>(bytes).is_err() {
            rejected = true;
            break;
        }
    }
    assert!(rejected || check.finish::<()>().is_err());
}

#[test]
fn cancel_before_final_readback_cannot_commit_a_trial() {
    let bytes = image();
    let mut nor = Nor::new();
    let mut writer = Installer::begin(&mut nor, manifest(&bytes)).unwrap();
    for (n, chunk) in bytes.chunks(4096).enumerate() {
        writer.push(&mut nor, (n * 4096) as u32, chunk).unwrap();
    }
    let mut check = writer.start_verify::<()>().unwrap();
    assert!(check.step(&mut nor).unwrap().is_none());
    assert_eq!(check.checked(), 4096);
    drop(check); // The actual firmware cancels between these bounded steps.
    let (_, record) = boot::read(&mut nor).unwrap().unwrap();
    assert_eq!(record.state, State::Installing);
    assert_eq!(boot::target(Some(record), false, true), Target::Recovery);
}
