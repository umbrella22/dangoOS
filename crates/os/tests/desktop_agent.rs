use os::services::voice::desktop::{Command, Lease, LEASE_MS};
use voice_session::agent::Action;
#[test]
fn expired_attachments_cannot_be_revived_or_claim_another_hosts_capabilities() {
    let mut lease = Lease::new(7, 3, 100).unwrap();
    assert!(!lease.valid(8, 101));
    assert!(lease.renew(8, 101).is_err());
    assert_eq!(lease.capabilities, 3);
    assert!(lease.valid(7, 99 + LEASE_MS));
    assert!(!lease.valid(7, 100 + LEASE_MS));
    assert!(lease.renew(7, 100 + LEASE_MS).is_err());
    assert!(Lease::new(0, 7, 0).is_err());
    assert!(Lease::new(1, 8, 0).is_err());
}
#[test]
fn control_boundaries_preserve_percentages_and_transport_representations() {
    assert_eq!(
        Command::parse("agent-tool", &["5", "6", "1", "volume", "100"]).unwrap(),
        Command::Tool(5, 6, 1, Action::Volume(100))
    );
    for args in [
        ["5", "6", "1", "brightness", "9"],
        ["5", "6", "1", "volume", "101"],
        ["5", "6", "1", "brightness", "-1"],
    ] {
        assert!(Command::parse("agent-tool", &args).is_err());
    }
    let hex = os::formats::hex::encode("中文🦊".as_bytes());
    assert_eq!(
        Command::parse("agent-feed", &["5", "6", "0", &hex]).unwrap(),
        Command::Feed(5, 6, 0, "中文🦊".as_bytes().to_vec())
    );
    assert!(Command::parse("agent-feed", &["5", "6", "0", "f"]).is_err());
}
