/** sync/mirror_edits.rs — EXPERIMENTAL: an edit made on a mirror (name, description, cover), sent back to the album's origin. See ARCHITECTURE.md. */
use crate::sync::album_details::AlbumDetails;
use serde_json::Value;

#[derive(Debug, PartialEq)]
pub enum Verdict {
    Allowed,
    NotAccepted,
    ViewOnly,
}

use crate::config::cfg;
use crate::immich::access::MappingAuth;
use crate::immich::client::{Auth, Client};
use crate::p2p::frame::RequestHeader;
use crate::settings::Settings;
use crate::state::State;
use crate::store::{Mapping, Peer, Role};
use serde_json::{json, Map};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

const LONGEST_ALBUM_NAME: usize = 255;
const LONGEST_DESCRIPTION: usize = 5_000;
const REPORT_TIMEOUT: Duration = Duration::from_millis(5_000);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Edit {
    pub name: Option<String>,
    pub description: Option<String>,
    pub cover_asset: Option<String>,
}

impl Edit {
    fn is_empty(&self) -> bool {
        self.name.is_none() && self.description.is_none() && self.cover_asset.is_none()
    }

    fn from_body(body: &[u8]) -> Edit {
        let value = serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
        let text = |field: &str| value.get(field).and_then(Value::as_str).map(str::to_string);
        Edit {
            name: text("name"),
            description: text("description"),
            cover_asset: text("coverAsset"),
        }
    }

    fn to_body(&self) -> Value {
        let mut body = Map::new();
        if let Some(name) = &self.name {
            body.insert("name".into(), json!(name));
        }
        if let Some(description) = &self.description {
            body.insert("description".into(), json!(description));
        }
        if let Some(cover) = &self.cover_asset {
            body.insert("coverAsset".into(), json!(cover));
        }
        Value::Object(body)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reported {
    pub name: Option<String>,
    pub description: Option<String>,
    pub cover: Option<String>,
}

pub struct Seen<'a> {
    pub stored_name: &'a str,
    pub live_name: &'a str,
    pub live_description: &'a str,
    pub live_cover_origin: Option<&'a str>,
    pub origin_renamed: bool,
    pub offered: &'a AlbumDetails,
    pub description_aligned: bool,
    pub cover_aligned: bool,
}

static REPORTED: Mutex<Option<HashMap<String, Reported>>> = Mutex::new(None);

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

pub fn edit_to_send(seen: &Seen, reported: &Reported) -> Edit {
    let renamed = !seen.origin_renamed
        && crate::sync::album_rename::name_change(seen.stored_name, seen.live_name).is_some()
        && reported.name.as_deref() != Some(seen.live_name);
    let described = seen.description_aligned
        && seen.live_description != seen.offered.description
        && reported.description.as_deref() != Some(seen.live_description);
    let covered = seen
        .live_cover_origin
        .filter(|cover| {
            seen.cover_aligned
                && Some(*cover) != seen.offered.cover_asset.as_deref()
                && reported.cover.as_deref() != Some(*cover)
        })
        .map(str::to_string);
    Edit {
        name: renamed.then(|| seen.live_name.to_string()),
        description: described.then(|| seen.live_description.to_string()),
        cover_asset: covered,
    }
}

pub fn permitted(accepting: bool, permissions: &str) -> Verdict {
    if !accepting {
        Verdict::NotAccepted
    } else if permissions != "contribute" {
        Verdict::ViewOnly
    } else {
        Verdict::Allowed
    }
}

pub fn patch_for(current_name: &str, current: &AlbumDetails, edit: &Edit) -> Map<String, Value> {
    let mut patch = Map::new();
    if let Some(name) = edit.name.as_deref().filter(|name| {
        !name.trim().is_empty()
            && name.chars().count() <= LONGEST_ALBUM_NAME
            && *name != current_name
    }) {
        patch.insert("albumName".into(), json!(name));
    }
    if let Some(description) = edit.description.as_deref().filter(|description| {
        description.chars().count() <= LONGEST_DESCRIPTION && *description != current.description
    }) {
        patch.insert("description".into(), json!(description));
    }
    if let Some(cover) = edit
        .cover_asset
        .as_deref()
        .filter(|cover| !cover.is_empty() && Some(*cover) != current.cover_asset.as_deref())
    {
        patch.insert("albumThumbnailAssetId".into(), json!(cover));
    }
    patch
}

fn reported_for(mirror_id: &str) -> Reported {
    REPORTED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|reported| reported.get(mirror_id))
        .cloned()
        .unwrap_or_default()
}

fn remember_reported(mirror_id: &str, edit: &Edit) {
    let mut reported = REPORTED.lock().unwrap_or_else(|e| e.into_inner());
    let entry = reported
        .get_or_insert_with(HashMap::new)
        .entry(mirror_id.to_string())
        .or_default();
    if let Some(name) = &edit.name {
        entry.name = Some(name.clone());
    }
    if let Some(description) = &edit.description {
        entry.description = Some(description.clone());
    }
    if let Some(cover) = &edit.cover_asset {
        entry.cover = Some(cover.clone());
    }
}

fn origin_asset_of(state: &State, mirror: &Mapping, local_asset: &str) -> Option<String> {
    state
        .store
        .seen_for_mapping(&mirror.id)
        .unwrap_or_default()
        .into_iter()
        .find(|entry| entry.local_asset == local_asset)
        .and_then(|entry| entry.origin_asset)
}

pub async fn report_local_edits(
    state: &State,
    client: &Client,
    mirror: &Mapping,
    peer: &Peer,
    origin_renamed: bool,
    offered: &AlbumDetails,
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
    let live = crate::sync::album_details::details_of_album(&album);
    let live_name = album
        .get("albumName")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (description_aligned, cover_aligned, applied_cover_local) =
        crate::sync::album_details::alignment(&mirror.id, offered);
    let live_cover_origin = match (&live.cover_asset, cover_aligned) {
        (Some(local), true) if applied_cover_local.as_deref() != Some(local.as_str()) => {
            origin_asset_of(state, mirror, local)
        }
        (Some(_), true) => offered.cover_asset.clone(),
        _ => None,
    };
    let edit = edit_to_send(
        &Seen {
            stored_name: &mirror.album_name,
            live_name,
            live_description: &live.description,
            live_cover_origin: live_cover_origin.as_deref(),
            origin_renamed,
            offered,
            description_aligned,
            cover_aligned,
        },
        &reported_for(&mirror.id),
    );
    if edit.is_empty() {
        return;
    }
    let remote_id = crate::sync::peer_mapping_id::peer_album_mapping_id(mirror);
    let Some(transport) = crate::p2p::transport::transport() else {
        return;
    };
    let header = RequestHeader {
        path: format!("/albums/{remote_id}/edit"),
        ..Default::default()
    };
    let sent = Edit {
        name: edit
            .name
            .as_deref()
            .map(|name| origin_name_of(&cfg().mirror_album_template, &peer.name, name)),
        ..edit.clone()
    };
    let body = sent.to_body().to_string();
    let Ok(Ok((head, _))) = tokio::time::timeout(
        REPORT_TIMEOUT,
        transport.round_trip(peer, &header, Some(body.as_bytes())),
    )
    .await
    else {
        return;
    };
    remember_reported(&mirror.id, &edit);
    if head.status < 400 {
        crate::log!(
            "sent an edit of \"{}\" to \"{}\"",
            mirror.album_name,
            peer.name
        );
    } else {
        crate::log!(
            "\"{}\" did not take the edit of \"{}\" ({})",
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

pub async fn handle_edit(
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
    let accepting = Settings::read(&state.store).accept_mirror_edits;
    match permitted(accepting, &mapping.permissions) {
        Verdict::NotAccepted => {
            return (
                403,
                json!({ "error": "this server does not accept album edits", "code": "edits_not_accepted" }),
            )
        }
        Verdict::ViewOnly => {
            return (
                403,
                json!({ "error": "this share is view only", "code": "view_only" }),
            )
        }
        Verdict::Allowed => {}
    }
    let edit = Edit::from_body(body);
    let current = crate::sync::album_details::offered(&peer.pub_key, &mapping.album_id);
    let patch = patch_for(&mapping.album_name, &current, &edit);
    if patch.is_empty() {
        return (200, json!({ "ok": true, "changed": false }));
    }
    for key in editor_keys(state, &peer, &mapping) {
        let applied = client
            .json(
                reqwest::Method::PATCH,
                &format!("/albums/{}", mapping.album_id),
                &Auth::Key(&key),
                Some(&Value::Object(patch.clone())),
            )
            .await
            .is_ok();
        if !applied {
            continue;
        }
        if let Some(name) = patch.get("albumName").and_then(Value::as_str) {
            if let Some(live) = state
                .collections()
                .mappings
                .iter_mut()
                .find(|m| m.id == mapping.id)
            {
                live.album_name = name.to_string();
            }
            let _ = state.save();
        }
        crate::sync::album_details::record_offer(
            &peer.pub_key,
            &mapping.album_id,
            AlbumDetails {
                description: patch
                    .get("description")
                    .and_then(Value::as_str)
                    .map_or(current.description.clone(), str::to_string),
                cover_asset: patch
                    .get("albumThumbnailAssetId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or(current.cover_asset.clone()),
            },
        );
        crate::log!(
            "\"{}\" edited \"{}\" from its mirror ({})",
            peer.name,
            mapping.album_name,
            patch.keys().cloned().collect::<Vec<_>>().join(", ")
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

    fn details(description: &str, cover: Option<&str>) -> AlbumDetails {
        AlbumDetails {
            description: description.into(),
            cover_asset: cover.map(str::to_string),
        }
    }

    fn aligned_view<'a>(offered: &'a AlbumDetails) -> Seen<'a> {
        Seen {
            stored_name: "Wandern",
            live_name: "Wandern",
            live_description: &offered.description,
            live_cover_origin: offered.cover_asset.as_deref(),
            origin_renamed: false,
            offered,
            description_aligned: true,
            cover_aligned: true,
        }
    }

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
    fn a_mirror_that_matches_the_origin_has_nothing_to_send() {
        let offered = details("Two weeks", Some("c1"));
        assert!(edit_to_send(&aligned_view(&offered), &Reported::default()).is_empty());
    }

    #[test]
    fn a_local_rename_is_sent_once_per_new_name() {
        let offered = details("", None);
        let mut seen = aligned_view(&offered);
        seen.live_name = "Berge";
        let edit = edit_to_send(&seen, &Reported::default());
        assert_eq!(edit.name.as_deref(), Some("Berge"));
        let reported = Reported {
            name: Some("Berge".into()),
            ..Default::default()
        };
        assert_eq!(edit_to_send(&seen, &reported).name, None);
        seen.live_name = "Alpen";
        assert_eq!(
            edit_to_send(&seen, &reported).name.as_deref(),
            Some("Alpen")
        );
    }

    #[test]
    fn an_empty_local_name_is_not_a_rename() {
        let offered = details("", None);
        let mut seen = aligned_view(&offered);
        seen.live_name = "";
        assert_eq!(edit_to_send(&seen, &Reported::default()).name, None);
    }

    #[test]
    fn when_the_origin_renamed_too_the_origin_wins_and_no_name_is_sent() {
        let offered = details("", None);
        let mut seen = aligned_view(&offered);
        seen.live_name = "Berge";
        seen.origin_renamed = true;
        assert_eq!(edit_to_send(&seen, &Reported::default()).name, None);
    }

    #[test]
    fn a_local_description_is_sent_when_the_mirror_had_caught_up() {
        let offered = details("Two weeks", None);
        let mut seen = aligned_view(&offered);
        seen.live_description = "Three weeks";
        let edit = edit_to_send(&seen, &Reported::default());
        assert_eq!(edit.description.as_deref(), Some("Three weeks"));
        seen.live_description = "";
        assert_eq!(
            edit_to_send(&seen, &Reported::default())
                .description
                .as_deref(),
            Some("")
        );
    }

    #[test]
    fn a_description_difference_is_not_an_edit_before_the_mirror_caught_up() {
        let offered = details("Two weeks", None);
        let mut seen = aligned_view(&offered);
        seen.live_description = "";
        seen.description_aligned = false;
        assert_eq!(edit_to_send(&seen, &Reported::default()).description, None);
    }

    #[test]
    fn a_local_cover_is_sent_when_it_is_one_of_the_origins_photos() {
        let offered = details("", Some("c1"));
        let mut seen = aligned_view(&offered);
        seen.live_cover_origin = Some("c2");
        let edit = edit_to_send(&seen, &Reported::default());
        assert_eq!(edit.cover_asset.as_deref(), Some("c2"));
    }

    #[test]
    fn a_cover_that_is_not_an_origin_photo_or_not_yet_applied_is_not_sent() {
        let offered = details("", Some("c1"));
        let mut seen = aligned_view(&offered);
        seen.live_cover_origin = None;
        assert_eq!(edit_to_send(&seen, &Reported::default()).cover_asset, None);
        seen.live_cover_origin = Some("c2");
        seen.cover_aligned = false;
        assert_eq!(edit_to_send(&seen, &Reported::default()).cover_asset, None);
    }

    #[test]
    fn a_server_that_does_not_accept_edits_refuses_them() {
        assert_eq!(permitted(false, "contribute"), Verdict::NotAccepted);
    }

    #[test]
    fn a_view_only_share_may_not_edit() {
        assert_eq!(permitted(true, "view"), Verdict::ViewOnly);
    }

    #[test]
    fn an_editable_share_may_edit_when_accepted() {
        assert_eq!(permitted(true, "contribute"), Verdict::Allowed);
    }

    #[test]
    fn only_what_differs_from_the_album_is_patched() {
        let current = details("Two weeks", Some("c1"));
        let edit = Edit {
            name: Some("Berge".into()),
            description: Some("Two weeks".into()),
            cover_asset: Some("c2".into()),
        };
        let patch = patch_for("Wandern", &current, &edit);
        assert_eq!(patch["albumName"], "Berge");
        assert_eq!(patch["albumThumbnailAssetId"], "c2");
        assert!(!patch.contains_key("description"));
    }

    #[test]
    fn a_cleared_description_is_patched() {
        let current = details("Two weeks", None);
        let edit = Edit {
            description: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(patch_for("Wandern", &current, &edit)["description"], "");
    }

    #[test]
    fn blank_overlong_or_unchanged_values_patch_nothing() {
        let current = details("Two weeks", Some("c1"));
        let edit = Edit {
            name: Some("  ".into()),
            description: Some("x".repeat(LONGEST_DESCRIPTION + 1)),
            cover_asset: Some("c1".into()),
        };
        assert!(patch_for("Wandern", &current, &edit).is_empty());
        let long_name = Edit {
            name: Some("x".repeat(LONGEST_ALBUM_NAME + 1)),
            ..Default::default()
        };
        assert!(patch_for("Wandern", &current, &long_name).is_empty());
        let same_name = Edit {
            name: Some("Wandern".into()),
            ..Default::default()
        };
        assert!(patch_for("Wandern", &current, &same_name).is_empty());
    }
}
