//! Deliver transcribed text to the focused herdr pane.

use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) fn herdr_bin() -> String {
    std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_string())
}

pub(crate) struct FocusedPane {
    pub(crate) pane_id: String,
}

/// Sticky revision: a transition A→B→A must not authorize A's old draft.
pub(crate) struct FocusTracker {
    last: Mutex<Option<String>>,
    revision: AtomicU64,
}

impl FocusTracker {
    pub(crate) fn new(initial: Option<String>) -> Self {
        Self {
            last: Mutex::new(initial),
            revision: AtomicU64::new(0),
        }
    }

    pub(crate) fn observe(&self, current: Option<String>) {
        let mut previous = self.last.lock().unwrap();
        if *previous != current {
            *previous = current;
            self.revision.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.revision.load(Ordering::SeqCst)
    }

    /// Take a consistent observed pane/epoch for audio queued behind STT.
    pub(crate) fn snapshot(&self) -> (u64, Option<String>) {
        let pane = self.last.lock().unwrap();
        (self.epoch(), pane.clone())
    }
}

pub(crate) fn watch_focus(stop: Arc<AtomicBool>) -> Arc<FocusTracker> {
    let initial = focused_pane().ok().map(|p| p.pane_id);
    let tracker = Arc::new(FocusTracker::new(initial));
    let watcher = tracker.clone();
    std::thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            let current = focused_pane().ok().map(|p| p.pane_id);
            watcher.observe(current);
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    tracker
}

fn parse_focused_pane(json: &serde_json::Value) -> Result<FocusedPane, String> {
    let panes = json["result"]["panes"]
        .as_array()
        .ok_or("pane list: missing panes array")?;
    let pane = panes
        .iter()
        .find(|p| p["focused"].as_bool() == Some(true))
        .ok_or("no focused pane")?;
    if pane["agent"].as_str().is_none() {
        return Err("focused pane is not an agent; transcript not sent".into());
    }
    Ok(FocusedPane {
        pane_id: pane["pane_id"]
            .as_str()
            .ok_or("focused pane has no pane_id")?
            .to_string(),
    })
}

/// Focused pane at delivery time, from the live pane list (the daemon's own
/// HERDR_PANE_ID env is stale — it names whichever pane started the daemon).
pub(crate) fn focused_pane() -> Result<FocusedPane, String> {
    let out = Command::new(herdr_bin())
        .args(["pane", "list"])
        .output()
        .map_err(|e| format!("herdr pane list: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "herdr pane list failed (exit code {})",
            out.status
                .code()
                .map_or_else(|| "signal".to_string(), |code| code.to_string())
        ));
    }
    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("pane list json: {e}"))?;
    parse_focused_pane(&json)
}

fn prompt_args<'a>(pane: &'a FocusedPane, text: &'a str) -> [&'a str; 4] {
    ["agent", "prompt", &pane.pane_id, text]
}

fn require_same_pane(start: &FocusedPane, current: &FocusedPane) -> Result<(), String> {
    if start.pane_id == current.pane_id {
        Ok(())
    } else {
        Err("focus changed while speaking; unsent speech discarded".into())
    }
}

pub(crate) fn same_as_current(pane: &FocusedPane) -> bool {
    focused_pane()
        .map(|current| require_same_pane(pane, &current).is_ok())
        .unwrap_or(false)
}

/// Deliver only when the original agent pane still has focus.
/// Never type into an ordinary shell.
pub(crate) fn deliver_to(pane: &FocusedPane, text: &str) -> Result<(), String> {
    require_same_pane(pane, &focused_pane()?)?;
    send_prompt(pane, text)
}

/// Deliver to the pane the speech started in even though focus has moved on.
/// Only for text already transcribed there; `agent prompt` still refuses a
/// pane that no longer hosts an agent.
pub(crate) fn deliver_to_original(pane: &FocusedPane, text: &str) -> Result<(), String> {
    send_prompt(pane, text)
}

fn send_prompt(pane: &FocusedPane, text: &str) -> Result<(), String> {
    let args = prompt_args(pane, text);
    let out = Command::new(herdr_bin())
        .args(args)
        .output()
        .map_err(|e| format!("herdr {}: {e}", args[..2].join(" ")))?;
    if !out.status.success() {
        let blocked = [&out.stdout, &out.stderr]
            .iter()
            .any(|b| String::from_utf8_lossy(b).contains("agent_blocked"));
        if blocked && awaits_clarify_answer(&pane.pane_id) {
            return answer_clarify(pane, text);
        }
        return Err(format!(
            "herdr {} failed (exit code {}){}",
            args[..2].join(" "),
            out.status
                .code()
                .map_or_else(|| "signal".to_string(), |code| code.to_string()),
            if blocked {
                "; agent is blocked on a prompt that is not a clarify question"
            } else {
                ""
            }
        ));
    }
    Ok(())
}

/// `agent prompt` refuses a blocked agent, but Hermes' clarify question is a
/// free-text box and safe to type into. Other blocked prompts (permission or
/// choice menus) are never typed into: speech like "1번" could approve them.
fn awaits_clarify_answer(pane_id: &str) -> bool {
    let Ok(out) = Command::new(herdr_bin())
        .args([
            "pane", "read", pane_id, "--source", "visible", "--lines", "12",
        ])
        .output()
    else {
        return false;
    };
    out.status.success()
        && String::from_utf8_lossy(&out.stdout)
            .to_lowercase()
            .contains("type your answer here and press enter")
}

fn answer_clarify(pane: &FocusedPane, text: &str) -> Result<(), String> {
    let out = Command::new(herdr_bin())
        .args(["pane", "run", &pane.pane_id, text])
        .output()
        .map_err(|e| format!("herdr pane run: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "herdr pane run failed (exit code {})",
            out.status
                .code()
                .map_or_else(|| "signal".to_string(), |code| code.to_string())
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn changed_focus_never_invokes_prompt_even_with_a_real_command_boundary() {
        let _env_guard = ENV_LOCK.lock().unwrap();
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("handsfree-pane-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("mock-herdr");
        let focus = dir.join("focus");
        let sent = dir.join("sent");
        std::fs::write(&script, format!(
            "#!/bin/sh\nif [ \"$1 $2\" = 'pane list' ]; then printf '{{\"result\":{{\"panes\":[{{\"pane_id\":\"%s\",\"focused\":true,\"agent\":\"hermes\"}}]}}}}' \"$(/bin/cat '{}')\"; elif [ \"$1 $2\" = 'agent prompt' ]; then printf '%s' \"$4\" > '{}'; else exit 2; fi\n",
            focus.display(), sent.display()
        )).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(&focus, "pane-B").unwrap();
        let previous = std::env::var_os("HERDR_BIN_PATH");
        unsafe { std::env::set_var("HERDR_BIN_PATH", &script) };
        let original = super::FocusedPane {
            pane_id: "pane-A".into(),
        };
        assert!(super::deliver_to(&original, "private draft").is_err());
        assert!(
            !sent.exists(),
            "wrong-pane speech reached the prompt command"
        );
        if let Some(previous) = previous {
            unsafe { std::env::set_var("HERDR_BIN_PATH", previous) };
        } else {
            unsafe { std::env::remove_var("HERDR_BIN_PATH") };
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn transcribed_speech_reaches_the_original_pane_after_focus_moves() {
        let _env_guard = ENV_LOCK.lock().unwrap();
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("handsfree-home-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("mock-herdr");
        let sent = dir.join("sent");
        std::fs::write(&script, format!(
            "#!/bin/sh\nif [ \"$1 $2\" = 'pane list' ]; then printf '{{\"result\":{{\"panes\":[{{\"pane_id\":\"pane-B\",\"focused\":true,\"agent\":\"hermes\"}}]}}}}'; elif [ \"$1 $2\" = 'agent prompt' ]; then printf '%s %s' \"$3\" \"$4\" > '{}'; else exit 2; fi\n",
            sent.display()
        )).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let previous = std::env::var_os("HERDR_BIN_PATH");
        unsafe { std::env::set_var("HERDR_BIN_PATH", &script) };
        let original = super::FocusedPane {
            pane_id: "pane-A".into(),
        };
        assert!(super::deliver_to_original(&original, "주황까지 간 말").is_ok());
        assert_eq!(
            std::fs::read_to_string(&sent).unwrap(),
            "pane-A 주황까지 간 말"
        );
        if let Some(previous) = previous {
            unsafe { std::env::set_var("HERDR_BIN_PATH", previous) };
        } else {
            unsafe { std::env::remove_var("HERDR_BIN_PATH") };
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_prompt_does_not_expose_child_stderr_or_transcript() {
        let _env_guard = ENV_LOCK.lock().unwrap();
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "handsfree-private-stderr-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("mock-herdr");
        std::fs::write(
            &script,
            "#!/bin/sh\nif [ \"$1 $2\" = 'pane list' ]; then printf '{\"result\":{\"panes\":[{\"pane_id\":\"pane-A\",\"focused\":true,\"agent\":\"hermes\"}]}}'; elif [ \"$1 $2\" = 'agent prompt' ]; then printf '%s' \"$4\" >&2; exit 23; fi\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let previous = std::env::var_os("HERDR_BIN_PATH");
        unsafe { std::env::set_var("HERDR_BIN_PATH", &script) };
        let pane = super::FocusedPane {
            pane_id: "pane-A".into(),
        };
        let error = super::deliver_to(&pane, "private transcript").unwrap_err();
        assert!(error.contains("23"));
        assert!(!error.contains("private transcript"));
        if let Some(previous) = previous {
            unsafe { std::env::set_var("HERDR_BIN_PATH", previous) };
        } else {
            unsafe { std::env::remove_var("HERDR_BIN_PATH") };
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn blocked_agent_is_typed_into_only_when_it_asks_a_clarify_question() {
        let _env_guard = ENV_LOCK.lock().unwrap();
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("handsfree-clarify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("mock-herdr");
        let screen = dir.join("screen");
        let sent = dir.join("sent");
        std::fs::write(&script, format!(
            "#!/bin/sh\nif [ \"$1 $2\" = 'agent prompt' ]; then echo 'error: agent_blocked' >&2; exit 1; elif [ \"$1 $2\" = 'pane read' ]; then /bin/cat '{}'; elif [ \"$1 $2\" = 'pane run' ]; then printf '%s %s' \"$3\" \"$4\" > '{}'; else exit 2; fi\n",
            screen.display(), sent.display()
        )).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let previous = std::env::var_os("HERDR_BIN_PATH");
        unsafe { std::env::set_var("HERDR_BIN_PATH", &script) };
        let pane = super::FocusedPane {
            pane_id: "pane-A".into(),
        };

        std::fs::write(&screen, "Allow this command?\n❯ 1. Yes\n  2. No\n").unwrap();
        assert!(super::deliver_to_original(&pane, "1번").is_err());
        assert!(!sent.exists(), "speech was typed into a non-clarify prompt");

        std::fs::write(
            &screen,
            "? clarify (16.6s)\n❯ type your answer here and press Enter\n",
        )
        .unwrap();
        assert!(super::deliver_to_original(&pane, "2025-06-01").is_ok());
        assert_eq!(std::fs::read_to_string(&sent).unwrap(), "pane-A 2025-06-01");

        if let Some(previous) = previous {
            unsafe { std::env::set_var("HERDR_BIN_PATH", previous) };
        } else {
            unsafe { std::env::remove_var("HERDR_BIN_PATH") };
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn focus_switch_is_sticky_even_if_user_returns_to_original_pane() {
        let tracker = super::FocusTracker::new(Some("pane-A".into()));
        let original_epoch = tracker.epoch();
        tracker.observe(Some("pane-B".into()));
        tracker.observe(Some("pane-A".into()));
        assert_ne!(tracker.epoch(), original_epoch);
    }

    #[test]
    fn moving_to_another_pane_invalidates_unsent_speech() {
        let started = super::FocusedPane {
            pane_id: "w1:p1".into(),
        };
        let moved = super::FocusedPane {
            pane_id: "w1:p2".into(),
        };
        assert!(super::require_same_pane(&started, &moved).is_err());
        assert!(super::require_same_pane(&started, &started).is_ok());
    }

    #[test]
    fn prompt_uses_captured_pane_even_after_focus_moves() {
        let start = serde_json::json!({"result": {"panes": [
            {"pane_id":"w1:p1", "focused":true, "agent":"hermes"},
            {"pane_id":"w1:p2", "focused":false, "agent":"claude"}
        ]}});
        let later = serde_json::json!({"result": {"panes": [
            {"pane_id":"w1:p1", "focused":false, "agent":"hermes"},
            {"pane_id":"w1:p2", "focused":true, "agent":"claude"}
        ]}});
        let captured = super::parse_focused_pane(&start).unwrap();
        assert_eq!(super::parse_focused_pane(&later).unwrap().pane_id, "w1:p2");
        assert_eq!(
            super::prompt_args(&captured, "test"),
            ["agent", "prompt", "w1:p1", "test"]
        );
    }

    #[test]
    fn focuses_only_agent_pane_without_falling_back_to_shell() {
        let shell = serde_json::json!({"result": {"panes": [
            {"pane_id":"w1:p1", "focused":true},
            {"pane_id":"w1:p2", "focused":false, "agent":"claude"}
        ]}});
        assert!(super::parse_focused_pane(&shell).is_err());
        let agent = serde_json::json!({"result": {"panes": [
            {"pane_id":"w1:p1", "focused":false},
            {"pane_id":"w1:p2", "focused":true, "agent":"hermes"}
        ]}});
        assert_eq!(super::parse_focused_pane(&agent).unwrap().pane_id, "w1:p2");
    }
}
