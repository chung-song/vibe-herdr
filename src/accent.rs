//! Tint Herdr's accent color (focused pane border, highlights) by dictation
//! phase while dictation is on. Only our marked line is changed.

use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

const MARKER: &str = "# handsfree-phase-accent";

/// Accent for a dictation phase; `None` keeps the theme's own color.
pub(crate) fn color_for(phase: &str) -> Option<&'static str> {
    match phase {
        "speech" => Some("#f7768e"),
        "transcribing" | "checking" => Some("#ff9e64"),
        "waiting" => Some("#e0af68"),
        _ => None,
    }
}

fn config_path() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME not set")).join(".config/herdr/config.toml")
}

/// Drop any marked line, then insert the new one right after `[ui]`.
/// Returns `None` when the file has no `[ui]` table to put it in.
fn with_accent(text: &str, color: Option<&str>) -> Option<String> {
    // Do not repair or rewrite a malformed config. A duplicate table can also
    // make an apparently simple line edit ambiguous.
    toml::from_str::<toml::Value>(text).ok()?;
    if text.lines().filter(|line| line.trim() == "[ui]").count() != 1 {
        return None;
    }
    let mut out = String::with_capacity(text.len() + 64);
    let mut has_ui = false;
    let mut user_accent = false;
    let mut in_ui = false;
    let mut marked = 0;
    for line in text.split_inclusive('\n') {
        if line.trim_end().ends_with(MARKER) {
            marked += 1;
            let owned = ["#f7768e", "#ff9e64", "#e0af68"]
                .iter()
                .any(|c| line.trim_end() == format!("accent = \"{c}\" {MARKER}"));
            if !in_ui || !owned || marked > 1 {
                return None;
            }
            continue;
        }
        if line.trim_start().starts_with('[') {
            in_ui = line.trim() == "[ui]";
        } else if in_ui && line.trim_start().starts_with("accent") && line.contains('=') {
            user_accent = true;
        }
        out.push_str(line);
        if in_ui && line.trim_end() == "[ui]" {
            has_ui = true;
            if let Some(c) = color {
                out.push_str(&format!("accent = \"{c}\" {MARKER}\n"));
            }
        }
    }
    (has_ui && !user_accent && toml::from_str::<toml::Value>(&out).is_ok()).then_some(out)
}

/// Apply `color` and reload Herdr. Failures are logged, never fatal.
pub(crate) fn apply(color: Option<&str>) {
    // Resolve a dotfiles symlink so the rename replaces the real file.
    let Ok(path) = std::fs::canonicalize(config_path()) else {
        return;
    };
    if !write_accent(&path, color) {
        return;
    }
    match std::process::Command::new(crate::deliver::herdr_bin())
        .args(["server", "reload-config"])
        .output()
    {
        Ok(o) if o.status.success() => {}
        Ok(o) => eprintln!("accent: reload-config failed ({})", o.status),
        Err(e) => eprintln!("accent: reload-config: {e}"),
    }
}

fn write_accent(path: &std::path::Path, color: Option<&str>) -> bool {
    write_accent_with_hook(path, color, || {})
}

fn same_file_state(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
        && a.mode() == b.mode()
}

fn write_accent_with_hook(
    path: &std::path::Path,
    color: Option<&str>,
    before_check: impl FnOnce(),
) -> bool {
    let Ok(mut source) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(original_meta) = source.metadata() else {
        return false;
    };
    let mut text = String::new();
    if source.read_to_string(&mut text).is_err() {
        return false;
    }
    let Some(next) = with_accent(&text, color) else {
        return false;
    };
    if next == text {
        return false;
    }
    let tmp = path.with_extension(format!("toml.handsfree-{}.tmp", std::process::id()));
    let result = std::fs::write(&tmp, &next)
        .and_then(|()| std::fs::set_permissions(&tmp, original_meta.permissions()));
    if let Err(e) = result {
        eprintln!("accent: cannot write herdr config: {e}");
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    before_check();
    // Best-effort final check, immediately before rename; this is not an atomic CAS.
    let unchanged = std::fs::metadata(path)
        .ok()
        .is_some_and(|meta| same_file_state(&original_meta, &meta))
        && std::fs::read_to_string(path).ok().as_deref() == Some(text.as_str())
        && std::fs::metadata(path)
            .ok()
            .is_some_and(|meta| same_file_state(&original_meta, &meta));
    if !unchanged {
        eprintln!("accent: config changed externally; leaving it untouched");
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        eprintln!("accent: cannot replace herdr config: {e}");
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{MARKER, with_accent, write_accent, write_accent_with_hook};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn only_the_marked_line_changes_and_removal_restores_the_original() {
        let original = "[theme]\nname = \"x\"\n\n[ui]\ntab_bar_right = []\n";
        let red = with_accent(original, Some("#f7768e")).unwrap();
        assert_eq!(
            red,
            format!(
                "[theme]\nname = \"x\"\n\n[ui]\naccent = \"#f7768e\" {MARKER}\ntab_bar_right = []\n"
            )
        );
        let orange = with_accent(&red, Some("#ff9e64")).unwrap();
        assert_eq!(orange.matches(MARKER).count(), 1);
        assert_eq!(with_accent(&orange, None).unwrap(), original);
        assert!(with_accent("[theme]\n", Some("red")).is_none());
        assert!(with_accent("[ui]\n[ui]\n", Some("#f7768e")).is_none());
        assert!(with_accent("[ui]\n[ui]\n", None).is_none());
        assert!(with_accent("[ui]\ninvalid = [\n", Some("#f7768e")).is_none());
        assert!(with_accent("[ui]\ninvalid = [\n", None).is_none());
        assert!(with_accent("[ui]\naccent = \"a\"\naccent = \"b\"\n", Some("#f7768e")).is_none());
        assert!(with_accent("[ui]\naccent = \"#123456\"\n", Some("red")).is_none());
        assert!(
            with_accent(
                "[ui]\naccent = \"changed\" # handsfree-phase-accent\n",
                None
            )
            .is_none()
        );
        assert!(
            with_accent(
                "[ui]\naccent = \"#123456\" # handsfree-phase-accent\n",
                None
            )
            .is_none()
        );
        assert!(with_accent("[ui]\naccent = \"#f7768e\" # handsfree-phase-accent\naccent = \"#ff9e64\" # handsfree-phase-accent\n", None).is_none());
        let original_on = "[ui]\naccent = \"#123456\"\n";
        assert!(with_accent(original_on, Some("#f7768e")).is_none());
        assert!(with_accent(original_on, None).is_none());
    }

    #[test]
    fn restart_recovery_preserves_contents_permissions_and_conflicting_edits() {
        let path = std::env::temp_dir().join(format!("accent-test-{}", std::process::id()));
        let original = "[theme]\nname = \"x\"\n[ui]\ntab_bar_right = []\n";
        std::fs::write(&path, original).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(write_accent(&path, Some("#f7768e")));
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // A new daemon removes the marked line left by a crashed daemon.
        assert!(write_accent(&path, None));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::write(&path, "[ui]\naccent = \"#123456\"\n").unwrap();
        assert!(!write_accent(&path, Some("#f7768e")));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[ui]\naccent = \"#123456\"\n"
        );
        std::fs::write(
            &path,
            "[ui]\naccent = \"edited\" # handsfree-phase-accent\n",
        )
        .unwrap();
        assert!(!write_accent(&path, None));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[ui]\naccent = \"edited\" # handsfree-phase-accent\n"
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn external_edit_after_temp_creation_aborts_and_cleans_temp() {
        let path = std::env::temp_dir().join(format!("accent-conflict-{}", std::process::id()));
        let original = "[ui]\ntab_bar_right = []\n";
        std::fs::write(&path, original).unwrap();
        let edited = "[ui]\naccent = \"#123456\"\n";
        assert!(!write_accent_with_hook(&path, Some("#f7768e"), || {
            std::fs::write(&path, edited).unwrap();
        }));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
        assert!(
            !path
                .with_extension(format!("toml.handsfree-{}.tmp", std::process::id()))
                .exists()
        );
        // An external editor can replace the inode without changing bytes.
        std::fs::write(&path, original).unwrap();
        assert!(!write_accent_with_hook(&path, Some("#f7768e"), || {
            let replacement = path.with_extension("external-replacement");
            std::fs::write(&replacement, original).unwrap();
            std::fs::rename(&replacement, &path).unwrap();
        }));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert!(
            !path
                .with_extension(format!("toml.handsfree-{}.tmp", std::process::id()))
                .exists()
        );
        std::fs::remove_file(&path).unwrap();
    }
}
