//! Remote Jev completion classifier. A spoken transcript is sent to TypeSafe
//! only after it is segmented, never to the local coding-agent route.
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

const URL: &str = "https://api.typesafe.ai/v1/systemone";
const MODEL: &str = "jev-latest";
const INSTRUCTIONS: &str = "Has the speaker finished a stand-alone Korean conversational turn (statement, report, complaint, feedback, question, or request), rather than stopped mid-sentence? Do not require an explicit command or question.";
const TRUE_CRITERION: &str = "A grammatically complete utterance that an assistant could acknowledge or answer now, including a complete declarative report or complaint without an explicit request.";
const FALSE_CRITERION: &str = "A dangling clause, condition, connective, incomplete object, preamble or hesitation that clearly needs more words.";

/// A conservative deterministic veto for clauses the small model may mistake
/// for complete commands because Whisper inserts punctuation automatically.
pub(crate) fn safe_verdict(text: &str, model_says_complete: bool) -> bool {
    if !model_says_complete {
        return false;
    }
    let bare = text
        .trim()
        .trim_end_matches(|c: char| c.is_whitespace() || ".?!。？！,，".contains(c));
    if bare.chars().count() < 3 {
        return false;
    }
    let incomplete = [
        "하면",
        "가면",
        "라면",
        "면",
        "는데",
        "니까",
        "지만",
        "거든",
        "그리고",
        "그런데",
        "일단",
        "제가",
        "내가",
        "할 때",
        "것 같고",
        "한 가지는",
        "하나를",
    ];
    !incomplete.iter().any(|ending| bare.ends_with(ending))
}

pub(crate) fn parse_jev_verdict(body: &[u8]) -> Option<bool> {
    let response: serde_json::Value = serde_json::from_slice(body).ok()?;
    let answer = &response["answers"]["complete"];
    if answer["type"].as_str()? != "noul" {
        return None;
    }
    let p = answer["noul"].as_f64()?;
    if !(0.0..=1.0).contains(&p) {
        return None;
    }
    Some(p > 0.5)
}

/// Timeouts, missing credentials, and unclear results do not submit speech.
/// The user explicitly selected TypeSafe Jev; this sends the transcript there.
pub(crate) fn judge(text: &str) -> Result<bool, String> {
    if text.trim().is_empty() || text.len() > 8_192 {
        return Ok(false);
    }
    let key_path = crate::config::config_dir().join("jev.key");
    let metadata =
        std::fs::metadata(&key_path).map_err(|e| format!("Jev credential unavailable: {e}"))?;
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err("Jev credential must be readable only by its owner".into());
    }
    let key = std::fs::read_to_string(&key_path)
        .map_err(|e| format!("Jev credential unreadable: {e}"))?;
    let key = key.trim();
    if key.is_empty() || key.contains(['\r', '\n']) {
        return Err("Jev credential is invalid".into());
    }
    let request = serde_json::json!({
        "model": MODEL,
        "state": {"utterance": text},
        "questions": {"complete": {
            "type": "noul", "instructions": INSTRUCTIONS,
            "criteria": {"true": TRUE_CRITERION, "false": FALSE_CRITERION}
        }}
    });
    let response = ureq::post(URL)
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {key}"))
        .timeout(Duration::from_secs(4))
        .send_string(&request.to_string())
        .map_err(|e| format!("Jev turn judge unavailable: {e}"))?;
    let mut body = Vec::new();
    response
        .into_reader()
        .take(4_096)
        .read_to_end(&mut body)
        .map_err(|e| format!("Jev turn judge read failed: {e}"))?;
    let verdict = parse_jev_verdict(&body).ok_or("invalid Jev turn judge response")?;
    Ok(safe_verdict(text, verdict))
}

#[cfg(test)]
mod tests {
    #[test]
    fn jev_probability_decides_complete_turn_but_rejects_uncertainty() {
        let yes = br#"{"model":"jev-1.13.0","answers":{"complete":{"type":"noul","noul":0.79}},"usage":{"input_tokens":10,"output_tokens":2}}"#;
        let no = br#"{"answers":{"complete":{"type":"noul","noul":0.32}}}"#;
        let uncertain = br#"{"answers":{"complete":{"type":"noul","noul":0.5}}}"#;
        assert_eq!(super::parse_jev_verdict(yes), Some(true));
        assert_eq!(super::parse_jev_verdict(no), Some(false));
        assert_eq!(super::parse_jev_verdict(uncertain), Some(false));
        assert_eq!(super::parse_jev_verdict(b"invalid"), None);
    }

    #[test]
    #[ignore = "requires user-provided Jev API credit; run explicitly with --ignored"]
    fn jev_accepts_complete_declarative_bug_report() {
        assert!(
            super::judge("또 말 다했는데 이어서 말하라고 한다").unwrap(),
            "a finished declarative report must not wait for an explicit command"
        );
        assert!(!super::judge("내가 말을 하다가").unwrap());
    }

    #[test]
    #[ignore = "requires user-provided Jev API credit; run explicitly with --ignored"]
    fn jev_classifies_an_unfinished_and_a_complete_turn() {
        assert!(!super::judge("다른 페인으로 넘어가면?").unwrap());
        assert!(
            super::judge("지금 테스트했는데 계속 이어서 말씀해 주세요라고 떠. 왜 그래?").unwrap()
        );
    }
}
