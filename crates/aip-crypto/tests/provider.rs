#![allow(clippy::unwrap_used)]
use aip_crypto::{
    cell::{Context, DataKey, Error},
    provider::{KekProvider, LocalProvider, WrapContext},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};

fn context() -> WrapContext<'static> {
    WrapContext { schema: "aip_fixture", scope_kind: "ref:Project", scope_id: "7", dek_id: "dek-fixture", version: 1 }
}
fn spec() -> String {
    format!("k1:{},k2:{}", STANDARD.encode([1; 32]), STANDARD.encode([2; 32]))
}

#[tokio::test]
async fn kek_rewrap_keeps_existing_cell_ciphertext_readable_and_authenticates_metadata() {
    let old = LocalProvider::from_spec(&spec(), "k1").unwrap();
    let new = LocalProvider::from_spec(&spec(), "k2").unwrap();
    let material = [9; 32];
    let ctx = Context { schema: "aip_fixture", resource: "Secret", field: "value", row_id: "42", scope_kind: "ref:Project", scope_id: "7" };
    let frame = DataKey::new("dek-fixture", 1, material).unwrap().seal(&ctx, b"fixture plaintext").unwrap();
    let wrapped = old.wrap(&context(), &material).await.unwrap();
    assert_eq!(wrapped.kek_id, "k1");
    assert_ne!(wrapped.sealed, old.wrap(&context(), &material).await.unwrap().sealed);
    let plain = new.unwrap(&context(), &wrapped).await.unwrap();
    let rewrapped = new.wrap(&context(), &plain).await.unwrap();
    assert_eq!(rewrapped.kek_id, "k2");
    let opened = new.unwrap(&context(), &rewrapped).await.unwrap();
    assert_eq!(DataKey::new("dek-fixture", 1, *opened).unwrap().open(&ctx, &frame).unwrap().as_slice(), b"fixture plaintext");
    for changed in [
        WrapContext { schema: "aip_other", ..context() },
        WrapContext { scope_kind: "ref:Other", ..context() },
        WrapContext { scope_id: "8", ..context() },
        WrapContext { dek_id: "other-dek", ..context() },
        WrapContext { version: 2, ..context() },
    ] {
        assert_eq!(new.unwrap(&changed, &rewrapped).await, Err(Error::Corrupted));
    }
    let mut damaged = rewrapped.clone();
    damaged.sealed[20] ^= 1;
    assert_eq!(new.unwrap(&context(), &damaged).await, Err(Error::Corrupted));
    let mut renamed = rewrapped.clone();
    renamed.kek_id = "k1".into();
    assert_eq!(new.unwrap(&context(), &renamed).await, Err(Error::Corrupted));
}

#[test]
fn malformed_key_material_and_missing_or_duplicate_identities_are_rejected_without_echo() {
    for (spec, active) in [
        (String::new(), "k1"),
        (format!("k1:{0},k1:{0}", STANDARD.encode([1; 32])), "k1"),
        (format!("bad:id:{}", STANDARD.encode([1; 32])), "bad:id"),
        ("k1:not-base64".into(), "k1"),
        (format!("k1:{}", STANDARD.encode([1; 31])), "k1"),
        (spec(), "missing"),
    ] {
        assert!(matches!(LocalProvider::from_spec(&spec, active), Err(Error::InvalidInput)));
    }
}

#[cfg(unix)]
#[test]
fn key_file_requires_owned_private_regular_file_and_no_symlink_at_any_component() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    // macOS's /var is a system symlink; pass the actual owned path to the deliberately strict loader.
    let root = dir.path().canonicalize().unwrap();
    let folder = root.join("private-folder");
    std::fs::create_dir(&folder).unwrap();
    let path = folder.join("fixture-key-ring");
    std::fs::write(&path, spec()).unwrap();
    for mode in [0o600, 0o400] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        let provider = LocalProvider::from_file(&path, "k1").unwrap();
        assert!(!format!("{provider:?}").contains(&STANDARD.encode([1; 32])));
    }
    for mode in [0o644, 0o640, 0o660, 0o700] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        assert!(
            LocalProvider::from_file(&path, "k1").is_err(),
            "requested {mode:o}, observed {:o}",
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777
        );
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let alias = root.join("leaf-alias");
    symlink(&path, &alias).unwrap();
    assert!(LocalProvider::from_file(&alias, "k1").is_err());
    let folder_alias = root.join("folder-alias");
    symlink(&folder, &folder_alias).unwrap();
    assert!(LocalProvider::from_file(&folder_alias.join("fixture-key-ring"), "k1").is_err());
    assert!(LocalProvider::from_file(&folder, "k1").is_err());
    std::fs::write(&path, "x".repeat(8193)).unwrap();
    assert!(LocalProvider::from_file(&path, "k1").is_err());
}
