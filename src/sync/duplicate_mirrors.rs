/** sync/duplicate_mirrors.rs — finding and removing mirror albums nothing tracks. See docs/sync-loops.md. */
use crate::config::{bot_prefix, iso_seconds_ago};
use crate::immich::access::read_caller_albums;
use crate::immich::client::{Auth, Client};
use crate::settings::Settings;
use crate::state::State;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const MINIMUM_AGE_SECONDS: u64 = 600;
const AUTOMATIC_INTERVAL: Duration = Duration::from_secs(600);
pub const AUTOMATIC_BATCH: usize = 5;
pub const MANUAL_BATCH: usize = 50;

#[derive(Clone, Debug, PartialEq)]
pub struct StandInAlbum {
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub owner_name: String,
    pub created_at: String,
}

pub struct Removal {
    pub removed: usize,
    pub failed: usize,
}

pub fn owned_stand_in_albums(raw: &[Value], owner_id: &str) -> Vec<StandInAlbum> {
    raw.iter()
        .filter_map(|album| {
            let id = album.get("id")?.as_str()?;
            let owner = album
                .get("albumUsers")?
                .as_array()?
                .iter()
                .find(|entry| entry.get("role").and_then(|role| role.as_str()) == Some("owner"))?
                .get("user")?;
            if owner.get("id")?.as_str()? != owner_id {
                return None;
            }
            Some(StandInAlbum {
                id: id.to_string(),
                name: album
                    .get("albumName")
                    .and_then(|name| name.as_str())
                    .unwrap_or_default()
                    .to_string(),
                owner_id: owner_id.to_string(),
                owner_name: owner
                    .get("name")
                    .and_then(|name| name.as_str())
                    .unwrap_or_default()
                    .to_string(),
                created_at: album
                    .get("createdAt")
                    .and_then(|at| at.as_str())
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect()
}

pub fn find_duplicates(
    albums: &[StandInAlbum],
    tracked: &HashSet<String>,
    cutoff: &str,
) -> Vec<StandInAlbum> {
    albums
        .iter()
        .filter(|album| {
            !tracked.contains(&album.id)
                && !album.created_at.is_empty()
                && album.created_at.as_str() < cutoff
                && albums.iter().any(|twin| {
                    tracked.contains(&twin.id)
                        && twin.owner_id == album.owner_id
                        && twin.name == album.name
                })
        })
        .cloned()
        .collect()
}

pub fn take_batch(mut duplicates: Vec<StandInAlbum>, limit: usize) -> (Vec<StandInAlbum>, usize) {
    let held_back = duplicates.len().saturating_sub(limit);
    duplicates.truncate(limit);
    (duplicates, held_back)
}

fn tracked_album_ids(state: &State) -> HashSet<String> {
    state
        .collections()
        .mappings
        .iter()
        .map(|mapping| mapping.album_id.clone())
        .collect()
}

pub async fn scan(state: &State, client: &Client) -> Vec<StandInAlbum> {
    let stand_ins: Vec<(String, String)> = state
        .collections()
        .contributors
        .iter()
        .filter(|(slug, contributor)| {
            slug.starts_with(bot_prefix::PERSON)
                && !contributor.user_id.is_empty()
                && !contributor.api_key.is_empty()
        })
        .map(|(_, contributor)| (contributor.user_id.clone(), contributor.api_key.clone()))
        .collect();
    let mut owned = Vec::new();
    for (user_id, api_key) in stand_ins {
        let creds = crate::sync::invites::key_creds(state, &api_key);
        match read_caller_albums(client, &creds).await {
            Some(raw) => owned.extend(owned_stand_in_albums(&raw, &user_id)),
            None => {
                crate::log!("duplicate cleanup: could not read the albums of a stand-in account")
            }
        }
    }
    find_duplicates(
        &owned,
        &tracked_album_ids(state),
        &iso_seconds_ago(MINIMUM_AGE_SECONDS),
    )
}

fn stand_in_key(state: &State, owner_id: &str) -> Option<String> {
    state
        .collections()
        .contributors
        .values()
        .find(|contributor| contributor.user_id == owner_id && !contributor.api_key.is_empty())
        .map(|contributor| contributor.api_key.clone())
}

pub async fn remove(state: &State, client: &Client, duplicates: &[StandInAlbum]) -> Removal {
    let mut outcome = Removal {
        removed: 0,
        failed: 0,
    };
    for duplicate in duplicates {
        if tracked_album_ids(state).contains(&duplicate.id) {
            continue;
        }
        let Some(api_key) = stand_in_key(state, &duplicate.owner_id) else {
            outcome.failed += 1;
            continue;
        };
        match client
            .json(
                reqwest::Method::DELETE,
                &format!("/albums/{}", duplicate.id),
                &Auth::Key(&api_key),
                None,
            )
            .await
        {
            Ok(_) => {
                outcome.removed += 1;
                crate::log!(
                    "removed the duplicate mirror \"{}\" ({})",
                    duplicate.name,
                    duplicate.id
                );
            }
            Err(error) if error.reports_missing() => outcome.removed += 1,
            Err(error) => {
                outcome.failed += 1;
                crate::log!(
                    "could not remove the duplicate mirror \"{}\" ({}): {error}",
                    duplicate.name,
                    duplicate.id
                );
            }
        }
    }
    outcome
}

pub async fn sweep_if_enabled(state: &State, client: &Client) {
    if !Settings::read(&state.store).remove_duplicate_mirrors_automatically {
        return;
    }
    static LAST_SWEEP: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    {
        let mut last_sweep = LAST_SWEEP.get_or_init(Default::default).lock().unwrap();
        if last_sweep.is_some_and(|at| at.elapsed() < AUTOMATIC_INTERVAL) {
            return;
        }
        *last_sweep = Some(Instant::now());
    }
    let (batch, held_back) = take_batch(scan(state, client).await, AUTOMATIC_BATCH);
    if batch.is_empty() {
        return;
    }
    let outcome = remove(state, client, &batch).await;
    crate::log!(
        "duplicate cleanup: removed {}, failed {}, {held_back} left for the next pass",
        outcome.removed,
        outcome.failed
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;

    const CUTOFF: &str = "2026-11-10T00:00:00.000Z";
    const OLD: &str = "2026-11-04T10:58:28.000Z";
    const YOUNG: &str = "2026-11-12T10:58:28.000Z";

    fn album(id: &str, owner: &str, name: &str, created_at: &str) -> StandInAlbum {
        StandInAlbum {
            id: id.into(),
            name: name.into(),
            owner_id: owner.into(),
            owner_name: "josef".into(),
            created_at: created_at.into(),
        }
    }

    fn mapped(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    fn ids(duplicates: &[StandInAlbum]) -> Vec<&str> {
        duplicates.iter().map(|d| d.id.as_str()).collect()
    }

    #[test]
    fn an_untracked_album_beside_a_tracked_copy_of_the_same_share_is_a_duplicate() {
        let albums = [
            album("old", "u1", "Wandern", OLD),
            album("live", "u1", "Wandern", YOUNG),
        ];
        let found = find_duplicates(&albums, &mapped(&["live"]), CUTOFF);
        assert_eq!(ids(&found), vec!["old"]);
    }

    #[test]
    fn an_untracked_album_with_no_tracked_twin_is_kept() {
        let albums = [album("lonely", "u1", "Wandern", OLD)];
        assert!(find_duplicates(&albums, &mapped(&[]), CUTOFF).is_empty());
    }

    #[test]
    fn nothing_is_a_duplicate_when_no_record_survives() {
        let albums = [
            album("a", "u1", "Wandern", OLD),
            album("b", "u1", "Wandern", OLD),
        ];
        assert!(find_duplicates(&albums, &mapped(&[]), CUTOFF).is_empty());
    }

    #[test]
    fn a_tracked_album_is_never_a_duplicate() {
        let albums = [
            album("a", "u1", "Wandern", OLD),
            album("b", "u1", "Wandern", OLD),
        ];
        assert!(find_duplicates(&albums, &mapped(&["a", "b"]), CUTOFF).is_empty());
    }

    #[test]
    fn an_album_younger_than_the_cutoff_is_kept() {
        let albums = [
            album("new", "u1", "Wandern", YOUNG),
            album("live", "u1", "Wandern", OLD),
        ];
        assert!(find_duplicates(&albums, &mapped(&["live"]), CUTOFF).is_empty());
    }

    #[test]
    fn an_album_with_no_creation_time_is_kept_because_its_age_is_unknown() {
        let albums = [
            album("old", "u1", "Wandern", ""),
            album("live", "u1", "Wandern", OLD),
        ];
        assert!(find_duplicates(&albums, &mapped(&["live"]), CUTOFF).is_empty());
    }

    #[test]
    fn the_twin_must_belong_to_the_same_stand_in() {
        let albums = [
            album("old", "u1", "Wandern", OLD),
            album("live", "u2", "Wandern", OLD),
        ];
        assert!(find_duplicates(&albums, &mapped(&["live"]), CUTOFF).is_empty());
    }

    #[test]
    fn the_twin_must_carry_exactly_the_same_name() {
        let albums = [
            album("old", "u1", "Wandern", OLD),
            album("live", "u1", "wandern", OLD),
        ];
        assert!(find_duplicates(&albums, &mapped(&["live"]), CUTOFF).is_empty());
    }

    #[test]
    fn every_untracked_copy_of_one_share_is_reported() {
        let albums = [
            album("x", "u1", "Wandern", OLD),
            album("y", "u1", "Wandern", OLD),
            album("live", "u1", "Wandern", OLD),
        ];
        let found = find_duplicates(&albums, &mapped(&["live"]), CUTOFF);
        assert_eq!(ids(&found), vec!["x", "y"]);
    }

    #[test]
    fn a_batch_is_capped_and_says_how_many_it_held_back() {
        let many: Vec<StandInAlbum> = (0..7)
            .map(|n| album(&format!("d{n}"), "u1", "A", OLD))
            .collect();
        let (batch, held_back) = take_batch(many, 5);
        assert_eq!(batch.len(), 5);
        assert_eq!(held_back, 2);
    }

    #[test]
    fn only_albums_the_stand_in_owns_are_read_from_its_album_list() {
        let raw = vec![
            json!({"id": "mine", "albumName": "A", "createdAt": OLD,
                "albumUsers": [{"role": "owner", "user": {"id": "u1", "name": "josef"}}]}),
            json!({"id": "shared", "albumName": "B", "createdAt": OLD,
                "albumUsers": [{"role": "owner", "user": {"id": "someone-else", "name": "x"}},
                               {"role": "editor", "user": {"id": "u1", "name": "josef"}}]}),
            json!({"albumName": "no id", "createdAt": OLD,
                "albumUsers": [{"role": "owner", "user": {"id": "u1", "name": "josef"}}]}),
        ];
        let owned = owned_stand_in_albums(&raw, "u1");
        assert_eq!(ids(&owned), vec!["mine"]);
        assert_eq!(owned[0].created_at, OLD);
        assert_eq!(owned[0].owner_name, "josef");
    }
}
