use super::*;

#[test]
fn keyring_snapshot_prefers_plaintext_without_migrating_it() {
    // Given a newer plaintext identity beside an older keyring value.
    let scratch = tempfile::tempdir().unwrap();
    let dir = scratch.path();
    std::fs::write(dir.join("identity"), b"plaintext identity").unwrap();
    let request = SnapshotRequest {
        store: Store::Keyring,
        profile_dir: dir,
        kind: Kind::Identity,
        max_bytes: 16 * 1024,
    };

    // When routing takes its read-only snapshot.
    let bytes = snapshot_with(request, |_| {
        panic!("a readable plaintext value has runtime precedence")
    });

    // Then it sees the value the runtime will adopt, without adopting it now.
    assert_eq!(bytes.as_deref(), Some(b"plaintext identity".as_slice()));
    assert!(dir.join("identity").is_file());
}

#[test]
fn keyring_snapshot_reads_service_when_plaintext_is_absent() {
    // Given a profile whose only saved identity is in the service.
    let scratch = tempfile::tempdir().unwrap();
    let dir = scratch.path();
    let request = SnapshotRequest {
        store: Store::Keyring,
        profile_dir: dir,
        kind: Kind::Identity,
        max_bytes: 16 * 1024,
    };

    // When routing reads through the selected backend.
    let bytes = snapshot_with(request, |_| Ok(Some("keyring identity".to_string())));

    // Then the service value is the snapshot.
    assert_eq!(bytes.as_deref(), Some(b"keyring identity".as_slice()));
}

#[test]
fn unavailable_keyring_and_none_store_fail_closed() {
    // Given a plaintext value that keyring-only mode must ignore.
    let scratch = tempfile::tempdir().unwrap();
    let dir = scratch.path();
    std::fs::write(dir.join("identity"), b"ignored identity").unwrap();
    let none = SnapshotRequest {
        store: Store::None,
        profile_dir: dir,
        kind: Kind::Identity,
        max_bytes: 16 * 1024,
    };

    // When routing scans a backend that selection marked unavailable.
    let ignored = snapshot_with(none, |_| panic!("Store::None must not touch the keyring"));

    // Then no other backend is consulted.
    assert!(ignored.is_none());

    // Given no plaintext fallback and a locked or failed service.
    std::fs::remove_file(dir.join("identity")).unwrap();
    let keyring = SnapshotRequest {
        store: Store::Keyring,
        profile_dir: dir,
        kind: Kind::Identity,
        max_bytes: 16 * 1024,
    };

    // When the bounded service read fails, then routing cannot establish a match.
    assert!(snapshot_with(keyring, |_| Err("locked".to_string())).is_none());
}

#[test]
fn snapshots_refuse_values_over_the_requested_bound() {
    // Given both backends holding values one byte over the caller's bound.
    let scratch = tempfile::tempdir().unwrap();
    let dir = scratch.path();
    std::fs::write(dir.join("cookies"), b"12345").unwrap();
    let file = SnapshotRequest {
        store: Store::File,
        profile_dir: dir,
        kind: Kind::Cookies,
        max_bytes: 4,
    };

    // When each backend is read, then neither oversized value becomes routing evidence.
    assert!(snapshot_with(file, |_| unreachable!()).is_none());
    std::fs::remove_file(dir.join("cookies")).unwrap();
    let keyring = SnapshotRequest {
        store: Store::Keyring,
        ..file
    };
    assert!(snapshot_with(keyring, |_| Ok(Some("12345".to_string()))).is_none());
}
