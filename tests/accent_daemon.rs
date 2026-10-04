//! Exercise daemon startup, STOP cleanup, and crash recovery with an isolated HOME.
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, Command},
    thread,
    time::Duration,
};

const MARKER: &str = "accent = \"#f7768e\" # handsfree-phase-accent\n";
fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_herdr-handsfree")
}

fn command(home: &Path) -> Command {
    let mut cmd = Command::new(bin());
    cmd.env("HOME", home)
        .env("HERDR_PLUGIN_STATE_DIR", home.join("state"))
        .env("HERDR_PLUGIN_CONFIG_DIR", home.join("plugin-config"))
        .env("HERDR_BIN_PATH", "/usr/bin/true");
    cmd
}

fn start(home: &Path) -> Child {
    let child = command(home).arg("daemon").spawn().unwrap();
    for _ in 0..100 {
        if command(home)
            .arg("status")
            .output()
            .unwrap()
            .status
            .success()
        {
            return child;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("isolated daemon did not start");
}

fn stop(home: &Path, mut child: Child) {
    assert!(command(home).arg("stop").output().unwrap().status.success());
    assert!(child.wait().unwrap().success());
}

fn wait_text(path: &Path, expected: &str) {
    for _ in 0..100 {
        if fs::read_to_string(path).ok().as_deref() == Some(expected) {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fs::read_to_string(path).unwrap(), expected);
}

#[test]
fn stop_and_crash_restart_restore_only_owned_accent() {
    let home = std::env::temp_dir().join(format!("accent-daemon-{}", std::process::id()));
    let config = home.join(".config/herdr/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::create_dir_all(home.join("plugin-config")).unwrap();
    fs::write(
        home.join("plugin-config/config.toml"),
        "phase_accent = true\n",
    )
    .unwrap();
    let original = "[theme]\nname = \"x\"\n[ui]\ntab_bar_right = []\n";
    fs::write(
        &config,
        format!("[theme]\nname = \"x\"\n[ui]\n{MARKER}tab_bar_right = []\n"),
    )
    .unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    let child = start(&home);
    wait_text(&config, original);
    stop(&home, child);
    wait_text(&config, original);
    assert_eq!(
        fs::metadata(&config).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let mut child = start(&home);
    fs::write(
        &config,
        format!("[theme]\nname = \"x\"\n[ui]\n{MARKER}tab_bar_right = []\n"),
    )
    .unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    let child = start(&home);
    wait_text(&config, original);
    stop(&home, child);

    let user_accent = "[ui]\naccent = \"#123456\"\n";
    fs::write(&config, user_accent).unwrap();
    stop(&home, start(&home));
    assert_eq!(fs::read_to_string(&config).unwrap(), user_accent);
    let edited = "[ui]\naccent = \"changed\" # handsfree-phase-accent\n";
    fs::write(&config, edited).unwrap();
    stop(&home, start(&home));
    assert_eq!(fs::read_to_string(&config).unwrap(), edited);
    let duplicate = "[ui]\n[ui]\n";
    fs::write(&config, duplicate).unwrap();
    stop(&home, start(&home));
    assert_eq!(fs::read_to_string(&config).unwrap(), duplicate);
    fs::remove_dir_all(&home).unwrap();
}
