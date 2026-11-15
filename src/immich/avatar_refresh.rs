/** immich/avatar_refresh.rs — when a stand-in's picture is worth fetching again, and whether it changed. See ARCHITECTURE.md. */
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const RECHECK_AFTER: Duration = Duration::from_secs(3600);

struct Seen {
    checked: Instant,
    digest: String,
}

static SEEN: Mutex<Option<HashMap<String, Seen>>> = Mutex::new(None);

pub fn digest_of(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn due(checked: Option<Instant>, now: Instant) -> bool {
    checked.is_none_or(|checked| now.saturating_duration_since(checked) >= RECHECK_AFTER)
}

fn differs(known: Option<&str>, digest: &str) -> bool {
    known != Some(digest)
}

pub fn recheck_due(user_id: &str) -> bool {
    let seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let checked = seen
        .as_ref()
        .and_then(|seen| seen.get(user_id))
        .map(|seen| seen.checked);
    due(checked, Instant::now())
}

pub fn picture_changed(user_id: &str, digest: &str) -> bool {
    let seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    differs(
        seen.as_ref()
            .and_then(|seen| seen.get(user_id))
            .map(|seen| seen.digest.as_str()),
        digest,
    )
}

pub fn remember(user_id: &str, digest: &str) {
    SEEN.lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(
            user_id.to_string(),
            Seen {
                checked: Instant::now(),
                digest: digest.to_string(),
            },
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_never_checked_is_due() {
        assert!(due(None, Instant::now()));
    }

    #[test]
    fn a_picture_checked_a_moment_ago_is_not_due_and_an_old_check_is() {
        let now = Instant::now();
        assert!(!due(Some(now), now));
        assert!(due(Some(now), now + RECHECK_AFTER));
    }

    #[test]
    fn the_same_bytes_are_not_a_change_and_other_bytes_are() {
        let digest = digest_of(b"one face");
        assert!(!differs(Some(&digest), &digest_of(b"one face")));
        assert!(differs(Some(&digest), &digest_of(b"another face")));
        assert!(differs(None, &digest));
    }

    #[test]
    fn what_was_remembered_is_known_and_not_due() {
        let user = "avatar-refresh-test-user";
        assert!(recheck_due(user));
        assert!(picture_changed(user, "abc"));
        remember(user, "abc");
        assert!(!recheck_due(user));
        assert!(!picture_changed(user, "abc"));
        assert!(picture_changed(user, "def"));
    }
}
