use super::*;
use serde_json::json;

#[test]
fn addresses_split_into_host_and_port() {
    assert_eq!(split_address("play.example.com"), Ok(("play.example.com".into(), 25565)));
    assert_eq!(split_address(" play.example.com:25570 "), Ok(("play.example.com".into(), 25570)));
    assert_eq!(split_address("127.0.0.1:1"), Ok(("127.0.0.1".into(), 1)));
    assert_eq!(split_address("[::1]:25566"), Ok(("::1".into(), 25566)));
    assert_eq!(split_address("[::1]"), Ok(("::1".into(), 25565)));
}

#[test]
fn bad_addresses_are_refused() {
    for bad in [
        "",
        "   ",
        ":25565",
        "host:0",
        "host:70000",
        "host:abc",
        "[::1",
        "[::1]x",
        "host name",
        "host\nbind-address: 0.0.0.0:1",
        "host\"",
    ] {
        assert!(split_address(bad).is_err(), "{bad:?} should be refused");
    }
}

#[test]
fn versions_keep_to_a_versions_characters() {
    for good in ["1.21.4", "26.2", "1.8.9", "b1.7.3", "auto", " 1.20.1 "] {
        assert!(check_version(good).is_ok(), "{good:?}");
    }
    assert_eq!(check_version(" 26.2 "), Ok("26.2".into()));
    for bad in ["", "  ", "1.21\nauth-method: none", "1.21 4", "\"1.21\""] {
        assert!(check_version(bad).is_err(), "{bad:?} should be refused");
    }
    assert!(check_version(&"1".repeat(33)).is_err());
    assert!(check_version(&"1".repeat(32)).is_ok());
}

#[test]
fn the_config_listens_on_this_computer_only() {
    let yaml = config_yaml(40000, "play.example.com", 25565, "26.2", false);
    assert!(yaml.contains("bind-address: \"127.0.0.1:40000\"\n"));
    assert!(yaml.contains("target-address: \"play.example.com:25565\"\n"));
    assert!(yaml.contains("target-version: \"26.2\"\n"));
    assert!(yaml.contains("auth-method: none\n"));
    assert!(!yaml.contains("0.0.0.0"));
}

#[test]
fn the_config_uses_the_account_when_asked() {
    let yaml = config_yaml(40001, "::1", 25566, "1.21.4", true);
    assert!(yaml.contains("target-address: \"[::1]:25566\"\n"));
    assert!(yaml.contains("auth-method: account\n"));
}

#[test]
fn the_jar_is_the_plain_one_from_viaproxys_repository() {
    let base = "https://github.com/ViaVersion/ViaProxy/releases/download/v3.4.14";
    let release = json!({
        "tag_name": "v3.4.14",
        "assets": [
            {"name": "ViaProxy-3.4.14+java8.jar", "browser_download_url": format!("{base}/a.jar")},
            {"name": "ViaProxy-3.4.14-sources.jar", "browser_download_url": format!("{base}/b.jar")},
            {"name": "notes.txt", "browser_download_url": format!("{base}/c.txt")},
            {"name": "ViaProxy-3.4.14.jar", "browser_download_url": format!("{base}/d.jar"),
             "digest": "sha256:abc123"},
        ]
    });
    assert_eq!(
        pick_asset(&release),
        Some((format!("{base}/d.jar"), Some("abc123".to_string())))
    );
}

#[test]
fn a_jar_from_elsewhere_is_not_taken() {
    let release = json!({"assets": [
        {"name": "ViaProxy.jar", "browser_download_url": "https://example.com/ViaProxy.jar"}
    ]});
    assert_eq!(pick_asset(&release), None);
    assert_eq!(pick_asset(&json!({})), None);
}

#[test]
fn a_release_without_a_checksum_still_gives_its_jar() {
    let url = "https://github.com/ViaVersion/ViaProxy/releases/download/v1/ViaProxy.jar";
    let release = json!({"assets": [{"name": "ViaProxy.jar", "browser_download_url": url}]});
    assert_eq!(pick_asset(&release), Some((url.to_string(), None)));
}

#[test]
fn a_free_port_is_found() {
    assert!(free_port().unwrap() > 0);
}

fn launcher_account() -> Option<(String, Value)> {
    Some(("Notch".into(), json!({ "minecraftToken": {} })))
}

#[test]
fn the_launchers_account_is_used_first() {
    let auth = pick_auth(None, true, launcher_account);
    assert!(matches!(auth, Auth::Launcher { ref name, .. } if name == "Notch"));
    assert!(matches!(pick_auth(Some("other"), false, launcher_account), Auth::Launcher { .. }));
}

#[test]
fn without_the_launchers_account_the_saved_one_or_none() {
    assert_eq!(pick_auth(None, true, || None), Auth::Saved);
    assert_eq!(pick_auth(None, false, || None), Auth::None);
}

#[test]
fn the_override_decides_and_only_asks_the_launcher_when_it_must() {
    let never = || -> Option<(String, Value)> { panic!("the launcher's account was asked for") };
    assert_eq!(pick_auth(Some("none"), true, never), Auth::None);
    assert_eq!(pick_auth(Some("account"), false, never), Auth::Saved);
    assert!(matches!(pick_auth(Some("launcher"), true, launcher_account), Auth::Launcher { .. }));
    // Forced to the launcher's account with none to give: not ViaProxy's own.
    assert_eq!(pick_auth(Some("launcher"), true, || None), Auth::None);
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("horizon-via-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_run_folder_holds_the_account_for_this_user_only() {
    let base = scratch("saves");
    let run = base.join("run-40000");
    let entry = json!({ "minecraftProfile": { "name": "Notch" } });
    write_private_saves(&run, &entry).unwrap();
    let saves: Value = serde_json::from_slice(&fs::read(run.join("saves.json")).unwrap()).unwrap();
    assert_eq!(saves["accountsV4"], json!([entry]));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&run), 0o700);
        assert_eq!(mode(&run.join("saves.json")), 0o600);
    }
    // A leftover of the same name is replaced, not appended to.
    write_private_saves(&run, &json!({ "again": true })).unwrap();
    let saves: Value = serde_json::from_slice(&fs::read(run.join("saves.json")).unwrap()).unwrap();
    assert_eq!(saves["accountsV4"], json!([{ "again": true }]));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn a_run_still_starting_is_not_swept() {
    let base = scratch("sweep");
    // Nothing listens on port 1, but the folder is new: its ViaProxy may still be starting.
    let fresh = base.join("run-1");
    let other = base.join("keep-me");
    fs::create_dir_all(&fresh).unwrap();
    fs::create_dir_all(&other).unwrap();
    sweep_stale(&base);
    assert!(fresh.is_dir() && other.is_dir());
    let _ = fs::remove_dir_all(base);
}

