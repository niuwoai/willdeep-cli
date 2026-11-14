use super::init;

#[test]
fn init_is_private_and_never_overwrites() {
    let root = std::env::temp_dir().join(format!("willdeep-config-{}", uuid::Uuid::new_v4()));
    let path = root.join("nested/config.toml");
    init(&path).unwrap();
    assert!(path.exists());
    assert!(init(&path).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
