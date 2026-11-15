/** sync/mirror_rename.rs — EXPERIMENTAL: a rename made on a mirror, sent back to the album's origin. See ARCHITECTURE.md. */
use serde_json::{json, Value};

#[derive(Debug, PartialEq)]
pub enum Verdict {
    Apply,
    NotAccepted,
    ViewOnly,
    Nothing,
}

use crate::config::cfg;
use crate::immich::access::MappingAuth;
use crate::immich::client::{Auth, Client};
use crate::p2p::frame::RequestHeader;
use crate::settings::Settings;
use crate::state::State;
use crate::store::{Mapping, Peer, Role};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

const LONGEST_ALBUM_NAME: usize = 255;
const REPORT_TIMEOUT: Duration = Duration::from_millis(5_000);

static REPORTED: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

pub fn origin_name_of(template: &str, peer_name: &str, live: &str) -> String {
    let Some((before, after)) = template.split_once("{name}") else {
        return live.to_string();
    };
    let prefix = before.replace("{peer}", peer_name);
    let suffix = after.replace("{peer}", peer_name);
    live.strip_prefix(prefix.as_str())
        .and_then(|rest| rest.strip_suffix(suffix.as_str()))
        .filter(|name| !name.is_empty())
        .unwrap_or(live)
        .to_string()
}

fn should_report(
    stored: &str,
    live: &str,
    origin_renamed: bool,
    last_reported: Option<&str>,
) -> bool {
    !origin_renamed
        && crate::sync::album_rename::name_change(stored, live).is_some()
        && last_reported != Some(live)
}

pub fn verdict(accepting: bool, permissions: &str, current: &str, wanted: &str) -> Verdict {
    if !accepting {
        return Verdict::NotAccepted;
    }
    if permissions != "contribute" {
        return Verdict::ViewOnly;
    }
    if wanted.trim().is_empty() || wanted.chars().count() > LONGEST_ALBUM_NAME || wanted == current
    {
        return Verdict::Nothing;
    }
    Verdict::Apply
}

fn last_reported(mirror_id: &str) -> Option<String> {
    REPORTED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|reported| reported.get(mirror_id))
        .cloned()
}

fn remember_reported(mirror_id: &str, name: &str) {
    REPORTED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(mirror_id.to_string(), name.to_string());
}

pub async fn report_local_rename(
    state: &State,
    client: &Client,
    mirror: &Mapping,
    peer: &Peer,
    origin_renamed: bool,
) {
    let Ok(creds) = MappingAuth::for_mapping(state, mirror) else {
        return;
    };
    let Ok(Some(album)) = client
        .json(
            reqwest::Method::GET,
            &format!("/albums/{}", mirror.album_id),
            &creds.auth(),
            None,
        )
        .await
    else {
        return;
    };
    let live = album
        .get("albumName")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !should_report(
        &mirror.album_name,
        live,
        origin_renamed,
        last_reported(&mirror.id).as_deref(),
    ) {
        return;
    }
    let remote_id = crate::sync::peer_mapping_id::peer_album_mapping_id(mirror);
    let Some(transport) = crate::p2p::transport::transport() else {
        return;
    };
    let header = RequestHeader {
        path: format!("/albums/{remote_id}/rename"),
        ..Default::default()
    };
    let body = json!({ "name": origin_name_of(&cfg().mirror_album_template, &peer.name, live) })
        .to_string();
    let Ok(Ok((head, _))) = tokio::time::timeout(
        REPORT_TIMEOUT,
        transport.round_trip(peer, &header, Some(body.as_bytes())),
    )
    .await
    else {
        return;
    };
    remember_reported(&mirror.id, live);
    if head.status < 400 {
        crate::log!(
            "sent the rename of \"{}\" to \"{}\"",
            mirror.album_name,
            peer.name
        );
    } else {
        crate::log!(
            "\"{}\" did not take the rename of \"{}\" ({})",
            peer.name,
            mirror.album_name,
            head.status
        );
    }
}

fn editor_keys(state: &State, peer: &Peer, mapping: &Mapping) -> Vec<String> {
    let people = mapping.for_peer_user_ids.clone().unwrap_or_default();
    state
        .collections()
        .contributors
        .values()
        .filter(|contributor| {
            contributor.via_peer.as_deref() == Some(peer.pub_key.as_str())
                && contributor
                    .peer_user_id
                    .as_ref()
                    .is_some_and(|id| people.contains(id))
                && !contributor.api_key.is_empty()
        })
        .map(|contributor| contributor.api_key.clone())
        .collect()
}

pub async fn handle_rename(
    state: &State,
    client: &Client,
    caller_pub: &str,
    album_mapping_id: &str,
    body: &[u8],
) -> (u16, Value) {
    let Some(peer) = state
        .collections()
        .peers
        .iter()
        .find(|p| p.pub_key == caller_pub)
        .cloned()
    else {
        return (
            403,
            json!({ "error": "unknown peer", "code": "unknown_peer" }),
        );
    };
    let Some(mapping) =
        crate::p2p::protocol::mapping_for(state, &peer.pub_key, album_mapping_id, None)
    else {
        return crate::p2p::protocol::gone_or_404(state, &peer.pub_key, album_mapping_id);
    };
    if mapping.dead {
        return crate::p2p::protocol::gone_or_404(state, &peer.pub_key, album_mapping_id);
    }
    if mapping.role != Role::Owner || mapping.via != "invite" {
        return (404, json!({ "error": "not an album shared by invitation" }));
    }
    let wanted = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    let accepting = Settings::read(&state.store).accept_mirror_renames;
    match verdict(
        accepting,
        &mapping.permissions,
        &mapping.album_name,
        &wanted,
    ) {
        Verdict::NotAccepted => {
            return (
                403,
                json!({ "error": "this server does not accept renames", "code": "renames_not_accepted" }),
            )
        }
        Verdict::ViewOnly => {
            return (
                403,
                json!({ "error": "this share is view only", "code": "view_only" }),
            )
        }
        Verdict::Nothing => return (200, json!({ "ok": true, "changed": false })),
        Verdict::Apply => {}
    }
    for key in editor_keys(state, &peer, &mapping) {
        let renamed = client
            .json(
                reqwest::Method::PATCH,
                &format!("/albums/{}", mapping.album_id),
                &Auth::Key(&key),
                Some(&json!({ "albumName": wanted })),
            )
            .await
            .is_ok();
        if !renamed {
            continue;
        }
        if let Some(live) = state
            .collections()
            .mappings
            .iter_mut()
            .find(|m| m.id == mapping.id)
        {
            live.album_name = wanted.clone();
        }
        let _ = state.save();
        crate::log!(
            "\"{}\" renamed \"{}\" to \"{wanted}\" from its mirror",
            peer.name,
            mapping.album_name
        );
        return (200, json!({ "ok": true, "changed": true }));
    }
    (
        502,
        json!({ "error": "no account of that server may edit the album", "code": "not_editable" }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_in_the_plain_template_is_the_origin_name_as_typed() {
        assert_eq!(origin_name_of("{name}", "Holzlab", "Wandern"), "Wandern");
    }

    #[test]
    fn the_templates_own_decoration_is_taken_off() {
        assert_eq!(
            origin_name_of("{name} (via {peer})", "Holzlab", "Wandern (via Holzlab)"),
            "Wandern"
        );
        assert_eq!(
            origin_name_of("[{peer}] {name}", "Holzlab", "[Holzlab] Wandern 2"),
            "Wandern 2"
        );
    }

    #[test]
    fn a_name_that_dropped_the_decoration_is_used_whole() {
        assert_eq!(
            origin_name_of("{name} (via {peer})", "Holzlab", "Berge"),
            "Berge"
        );
    }

    #[test]
    fn a_local_rename_is_reported_only_once_per_new_name() {
        assert!(should_report("Wandern", "Berge", false, None));
        assert!(!should_report("Wandern", "Berge", false, Some("Berge")));
        assert!(should_report("Wandern", "Alpen", false, Some("Berge")));
    }

    #[test]
    fn an_unchanged_or_empty_local_name_is_not_a_rename() {
        assert!(!should_report("Wandern", "Wandern", false, None));
        assert!(!should_report("Wandern", "", false, None));
    }

    #[test]
    fn when_the_origin_renamed_too_the_origin_wins_and_nothing_is_sent() {
        assert!(!should_report("Wandern", "Berge", true, None));
    }

    #[test]
    fn a_server_that_does_not_accept_renames_refuses_them() {
        assert_eq!(
            verdict(false, "contribute", "Wandern", "Berge"),
            Verdict::NotAccepted
        );
    }

    #[test]
    fn a_view_only_share_may_not_rename() {
        assert_eq!(verdict(true, "view", "Wandern", "Berge"), Verdict::ViewOnly);
    }

    #[test]
    fn an_editable_share_may_rename_when_accepted() {
        assert_eq!(
            verdict(true, "contribute", "Wandern", "Berge"),
            Verdict::Apply
        );
    }

    #[test]
    fn a_blank_overlong_or_unchanged_name_changes_nothing() {
        assert_eq!(
            verdict(true, "contribute", "Wandern", "  "),
            Verdict::Nothing
        );
        assert_eq!(verdict(true, "contribute", "Wandern", ""), Verdict::Nothing);
        assert_eq!(
            verdict(true, "contribute", "Wandern", "Wandern"),
            Verdict::Nothing
        );
        assert_eq!(
            verdict(true, "contribute", "Wandern", &"x".repeat(256)),
            Verdict::Nothing
        );
    }
}
