//! Exercise the actual C decoder used by the bootloader against Rust records.
use boot_contract::{Record, State, BODY, COMMIT, RECORD};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn bootloader_c_and_rust_share_committed_bytes_and_reject_torn_records() {
    let directory =
        Fixture(std::env::temp_dir().join(format!("dango-boot-contract-{}", std::process::id())));
    fs::create_dir_all(&directory.0).unwrap();
    let header = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../bootloader/bootloader_components/main/contract.h")
        .canonicalize()
        .unwrap();
    let source = directory.0.join("contract.c");
    fs::write(
        &source,
        format!(
            r#"
#include <stdio.h>
#include "{}"
int main(void) {{
    uint8_t input[DG_RECORD], output[DG_RECORD];
    struct dg_record record;
    if (fread(input, 1, DG_RECORD, stdin) != DG_RECORD) return 2;
    if (!dg_decode(input, &record)) return 3;
    dg_encode(output, &record);
    return fwrite(output, 1, DG_RECORD, stdout) == DG_RECORD ? 0 : 4;
}}
"#,
            header.display()
        ),
    )
    .unwrap();
    let binary = directory.0.join("contract");
    assert!(Command::new("cc")
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror"])
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .status()
        .unwrap()
        .success());
    let run = |bytes: &[u8]| {
        let mut child = Command::new(&binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(bytes).unwrap();
        child.wait_with_output().unwrap()
    };
    for state in [
        State::Rescue,
        State::Installing,
        State::Trial,
        State::Testing,
        State::Ready,
        State::Staged,
    ] {
        let record = Record {
            generation: 0x1234_5678_abcd_ef01,
            state,
            image_bytes: 0x123456,
            sha256: std::array::from_fn(|i| i as u8),
        };
        let mut encoded = [0; RECORD];
        encoded[..BODY].copy_from_slice(&record.body());
        encoded[BODY..].copy_from_slice(COMMIT);
        let valid = run(&encoded);
        assert!(valid.status.success());
        assert_eq!(valid.stdout, encoded);
        assert_eq!(Record::decode(&valid.stdout), Some(record));
        // Damage every byte, including commit, reserved bytes and the high
        // generation word. Neither implementation may accept any such record.
        for offset in 0..RECORD {
            let mut damaged = encoded;
            damaged[offset] ^= 1;
            assert!(Record::decode(&damaged).is_none());
            assert_eq!(run(&damaged).status.code(), Some(3));
        }
    }
}

#[test]
fn display_handoff_from_actual_c_producer_is_accepted_once() {
    use boot_contract::display::{Handoff, FIRST_STORE};
    let directory = Fixture(
        std::env::temp_dir().join(format!("dango-display-contract-{}", std::process::id())),
    );
    fs::create_dir_all(&directory.0).unwrap();
    let header = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../bootloader/bootloader_components/main/display_contract.h")
        .canonicalize()
        .unwrap();
    let source = directory.0.join("display.c");
    fs::write(
        &source,
        format!(
            r#"
#include <stdio.h>
#include "{}"
int main(void) {{
    uint32_t words[5];
    dg_display_record(words, 1400000, 2000000);
    words[4] = DG_DISPLAY_FIRST_STORE;
    return fwrite(words, sizeof(words), 1, stdout) == 1 ? 0 : 1;
}}
"#,
            header.display()
        ),
    )
    .unwrap();
    let binary = directory.0.join("display");
    assert!(Command::new("cc")
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror"])
        .arg(source)
        .arg("-o")
        .arg(&binary)
        .status()
        .unwrap()
        .success());
    let output = Command::new(binary).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout.len(), 20);
    let words: [u32; 4] = std::array::from_fn(|i| {
        u32::from_le_bytes(output.stdout[i * 4..i * 4 + 4].try_into().unwrap())
    });
    assert_eq!(
        u32::from_le_bytes(output.stdout[16..20].try_into().unwrap()) as usize,
        FIRST_STORE
    );
    assert_eq!(
        Handoff::decode(words),
        Some(Handoff {
            first_frame_us: 1_400_000,
            selected_us: 2_000_000
        })
    );
    let mut consumed = words;
    consumed[0] = 0;
    assert!(Handoff::decode(consumed).is_none());
}
