//! Console text encoding and bounded reply reconstruction.
use os::formats::hex::{decode, encode};
use os::services::voice::{reply_record, Command, TRANSCRIPT_BYTES};

#[test]
fn console_prompt_preserves_valid_text_and_rejects_invalid_representations() {
    let text = "  你好🦊 \"hello\"\\\n第二行  ";
    assert_eq!(
        Command::ask(&encode(text.as_bytes())).unwrap(),
        Command::Ask(text.into())
    );
    let limit = "a".repeat(TRANSCRIPT_BYTES);
    assert_eq!(
        Command::ask(&encode(limit.as_bytes())).unwrap(),
        Command::Ask(limit)
    );
    for bytes in [
        b"".as_slice(),
        b" \n\t",
        b"a\0b",
        b"\xff",
        &vec![b'a'; TRANSCRIPT_BYTES + 1],
    ] {
        assert!(Command::ask(&encode(bytes)).is_err());
    }
    assert!(Command::ask("not hex!").is_err());
}

#[test]
fn console_reply_chunks_reassemble_unicode_and_fit_cdc_log_lines() {
    let line = reply_record(
        u32::MAX,
        u32::MAX as usize,
        u32::MAX as usize,
        &encode(&[0xff; 40]),
    );
    assert!(line.len() <= 158);
    let (_, payload) = line.split_once("hex=").unwrap();
    assert_eq!(decode(payload, 40).unwrap(), vec![0xff; 40]);
}
