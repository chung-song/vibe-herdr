//! Smallest check that breaks if the daemon control protocol breaks:
//! spawn the real binary with an isolated state dir, query, verify, stop.

use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_herdr-handsfree")
}

fn run(state_dir: &std::path::Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(bin())
        .args(args)
        .env("HERDR_PLUGIN_STATE_DIR", state_dir)
        .env("HOME", state_dir)
        .output()
        .expect("spawn");
    (
        out.status.success(),
        format!(
            "stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[test]
fn status_roundtrip_without_microphone() {
    let dir = std::env::temp_dir().join(format!("handsfree-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let (ok, out) = run(&dir, &["ensure-daemon"]);
    assert!(ok, "ensure-daemon failed: {out}");

    let (ok, out) = run(&dir, &["status"]);
    assert!(
        ok && out.contains("dictation: off") && out.contains("gaze: off"),
        "unexpected: {out}"
    );

    let (ok, _) = run(&dir, &["stop"]);
    assert!(ok, "stop failed");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mode_switch_is_saved_without_opening_microphone() {
    let dir = std::env::temp_dir().join(format!("handsfree-mode-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("dictation-mode"), "space").unwrap();
    let (ok, out) = run(&dir, &["ensure-daemon"]);
    assert!(ok, "{out}");
    let (ok, out) = run(&dir, &["status"]);
    assert!(ok && out.contains("mode: space"), "{out}");
    let (ok, out) = run(&dir, &["cycle-dictation-mode"]);
    assert!(ok && out.contains("mode=auto"), "{out}");
    assert_eq!(
        std::fs::read_to_string(dir.join("dictation-mode")).unwrap(),
        "auto"
    );
    let (ok, out) = run(&dir, &["stop"]);
    assert!(ok, "{out}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn explicit_correction_is_saved_once_with_private_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir =
        std::env::temp_dir().join(format!("handsfree-correction-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for _ in 0..2 {
        let (ok, out) = run(&dir, &["add-correction", "노션TV", "노션 DB"]);
        assert!(ok, "{out}");
    }
    let path = dir.join(".config/herdr-handsfree/corrections.tsv");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "노션TV\t노션 DB\n");
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
        0
    );
    let (ok, _) = run(&dir, &["add-correction", "노션TV", "다른 내용"]);
    assert!(!ok);
    let _ = std::fs::remove_dir_all(dir);
}
