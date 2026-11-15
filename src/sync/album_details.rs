/** sync/album_details.rs — an album's description and cover, carried from the origin to its mirror. See ARCHITECTURE.md. */
use serde_json::{json, Value};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlbumDetails {
    pub description: String,
    pub cover_asset: Option<String>,
}

use crate::immich::access::MappingAuth;
use crate::immich::client::Client;
use crate::state::State;
use crate::store::Mapping;
use std::collections::HashMap;
use std::sync::Mutex;

type OfferKey = (String, String);

#[derive(Default)]
struct Applied {
    description: Option<String>,
    cover: Option<String>,
}

static OFFERED: Mutex<Option<HashMap<OfferKey, AlbumDetails>>> = Mutex::new(None);
static APPLIED: Mutex<Option<HashMap<String, Applied>>> = Mutex::new(None);

fn text_of(album: &Value, field: &str) -> Option<String> {
    album
        .get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

pub fn details_of_album(album: &Value) -> AlbumDetails {
    AlbumDetails {
        description: text_of(album, "description").unwrap_or_default(),
        cover_asset: text_of(album, "albumThumbnailAssetId"),
    }
}

pub fn details_of_offer(invitation: &Value) -> AlbumDetails {
    let album = invitation.get("album").unwrap_or(&Value::Null);
    AlbumDetails {
        description: text_of(album, "description").unwrap_or_default(),
        cover_asset: text_of(album, "coverAsset"),
    }
}

pub fn put_on_offer(offer: &mut Value, details: &AlbumDetails) {
    if !details.description.is_empty() {
        offer["description"] = json!(details.description);
    }
    if let Some(cover) = &details.cover_asset {
        offer["coverAsset"] = json!(cover);
    }
}

fn description_pending(applied: Option<&str>, wanted: &AlbumDetails) -> bool {
    match applied {
        Some(applied) => applied != wanted.description,
        None => !wanted.description.is_empty(),
    }
}

fn cover_pending(applied: Option<&str>, wanted: &AlbumDetails) -> bool {
    wanted
        .cover_asset
        .as_deref()
        .is_some_and(|cover| applied != Some(cover))
}

pub fn record_offer(peer_pub: &str, album_id: &str, details: AlbumDetails) -> bool {
    let mut offered = OFFERED.lock().unwrap_or_else(|e| e.into_inner());
    let previous = offered.get_or_insert_with(HashMap::new).insert(
        (peer_pub.to_string(), album_id.to_string()),
        details.clone(),
    );
    previous.is_some_and(|previous| previous != details)
}

pub fn offered(peer_pub: &str, album_id: &str) -> AlbumDetails {
    OFFERED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|offered| offered.get(&(peer_pub.to_string(), album_id.to_string())))
        .cloned()
        .unwrap_or_default()
}

fn applied_state(mirror_id: &str) -> (Option<String>, Option<String>) {
    APPLIED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|applied| applied.get(mirror_id))
        .map(|applied| (applied.description.clone(), applied.cover.clone()))
        .unwrap_or_default()
}

fn mark_applied(mirror_id: &str, description: Option<&str>, cover: Option<&str>) {
    let mut applied = APPLIED.lock().unwrap_or_else(|e| e.into_inner());
    let entry = applied
        .get_or_insert_with(HashMap::new)
        .entry(mirror_id.to_string())
        .or_default();
    if let Some(description) = description {
        entry.description = Some(description.to_string());
    }
    if let Some(cover) = cover {
        entry.cover = Some(cover.to_string());
    }
}

async fn patch_album(state: &State, client: &Client, mirror: &Mapping, body: Value) -> bool {
    let Ok(creds) = MappingAuth::for_mapping(state, mirror) else {
        return false;
    };
    match client
        .json(
            reqwest::Method::PATCH,
            &format!("/albums/{}", mirror.album_id),
            &creds.auth(),
            Some(&body),
        )
        .await
    {
        Ok(_) => true,
        Err(error) => {
            crate::log!(
                "could not update the mirror \"{}\": {error}",
                mirror.album_name
            );
            false
        }
    }
}

pub async fn apply_to_mirror(
    state: &State,
    client: &Client,
    mirror: &Mapping,
    wanted: &AlbumDetails,
) {
    let (applied_description, applied_cover) = applied_state(&mirror.id);
    if description_pending(applied_description.as_deref(), wanted)
        && patch_album(
            state,
            client,
            mirror,
            json!({ "description": wanted.description }),
        )
        .await
    {
        mark_applied(&mirror.id, Some(&wanted.description), None);
    }
    if !cover_pending(applied_cover.as_deref(), wanted) {
        return;
    }
    let Some(origin_cover) = wanted.cover_asset.as_deref() else {
        return;
    };
    let local_cover = state
        .store
        .seen_for_mapping(&mirror.id)
        .unwrap_or_default()
        .into_iter()
        .find(|entry| entry.origin_asset.as_deref() == Some(origin_cover))
        .map(|entry| entry.local_asset)
        .filter(|local| !local.is_empty());
    let Some(local_cover) = local_cover else {
        return;
    };
    if patch_album(
        state,
        client,
        mirror,
        json!({ "albumThumbnailAssetId": local_cover }),
    )
    .await
    {
        mark_applied(&mirror.id, None, Some(origin_cover));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_immich_album_gives_its_description_and_cover() {
        let album = json!({
            "id": "a1",
            "albumName": "Trip",
            "description": "Two weeks in Norway",
            "albumThumbnailAssetId": "cover-1"
        });
        assert_eq!(
            details_of_album(&album),
            AlbumDetails {
                description: "Two weeks in Norway".into(),
                cover_asset: Some("cover-1".into())
            }
        );
    }

    #[test]
    fn an_album_without_either_gives_empty_details() {
        let album = json!({ "id": "a1", "description": null, "albumThumbnailAssetId": null });
        assert_eq!(details_of_album(&album), AlbumDetails::default());
    }

    #[test]
    fn details_survive_the_wire() {
        let details = AlbumDetails {
            description: "Two weeks".into(),
            cover_asset: Some("cover-1".into()),
        };
        let mut offer = json!({ "id": "a1", "name": "Trip" });
        put_on_offer(&mut offer, &details);
        let invitation = json!({ "album": offer });
        assert_eq!(details_of_offer(&invitation), details);
    }

    #[test]
    fn an_offer_from_an_older_peer_has_no_details() {
        let invitation = json!({ "album": { "id": "a1", "name": "Trip" } });
        assert_eq!(details_of_offer(&invitation), AlbumDetails::default());
    }

    #[test]
    fn empty_details_add_nothing_to_the_offer() {
        let mut offer = json!({ "id": "a1" });
        put_on_offer(&mut offer, &AlbumDetails::default());
        assert_eq!(offer, json!({ "id": "a1" }));
    }

    #[test]
    fn a_description_is_pending_when_it_differs_from_what_was_applied() {
        let wanted = AlbumDetails {
            description: "New".into(),
            cover_asset: None,
        };
        assert!(description_pending(None, &wanted));
        assert!(description_pending(Some("Old"), &wanted));
        assert!(!description_pending(Some("New"), &wanted));
    }

    #[test]
    fn an_empty_description_is_not_pending_on_a_mirror_never_touched() {
        assert!(!description_pending(None, &AlbumDetails::default()));
    }

    #[test]
    fn a_cover_is_pending_only_when_the_origin_has_one_that_was_not_applied() {
        let with_cover = AlbumDetails {
            description: String::new(),
            cover_asset: Some("c1".into()),
        };
        assert!(cover_pending(None, &with_cover));
        assert!(cover_pending(Some("c0"), &with_cover));
        assert!(!cover_pending(Some("c1"), &with_cover));
        assert!(!cover_pending(None, &AlbumDetails::default()));
    }
}
