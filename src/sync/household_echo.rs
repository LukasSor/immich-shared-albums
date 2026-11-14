use crate::immich::client::{Auth, Client};
use crate::immich::refs::AssetRef;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const PEOPLE_CACHE_LIFETIME: Duration = Duration::from_secs(60);

type PeopleCache = Mutex<Option<(Instant, HashSet<String>)>>;

pub fn authored_by_household(reference: &AssetRef, people: &HashSet<String>) -> bool {
    reference
        .contributor
        .origin_user_id
        .as_deref()
        .is_some_and(|id| people.contains(id))
}

async fn household_people(client: &Client) -> Option<HashSet<String>> {
    static CACHE: OnceLock<PeopleCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Some((fetched, people)) = cache.lock().unwrap().as_ref() {
        if fetched.elapsed() < PEOPLE_CACHE_LIFETIME {
            return Some(people.clone());
        }
    }
    let users = client
        .get("/admin/users", &Auth::Admin)
        .await
        .ok()
        .flatten()?;
    let people: HashSet<String> = users
        .as_array()?
        .iter()
        .filter(|user| !crate::config::is_utility_email(user.get("email").and_then(|e| e.as_str())))
        .filter_map(|user| {
            user.get("id")
                .and_then(|id| id.as_str())
                .map(str::to_string)
        })
        .collect();
    *cache.lock().unwrap() = Some((Instant::now(), people.clone()));
    Some(people)
}

pub async fn is_household_authored(client: &Client, reference: &AssetRef) -> bool {
    match household_people(client).await {
        Some(people) => authored_by_household(reference, &people),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::immich::refs::{AssetRef, Contributor};

    fn reference(origin_user_id: Option<&str>) -> AssetRef {
        AssetRef {
            origin_asset: "origin-1".into(),
            checksum: "summer.jpg".into(),
            checksum_alg: None,
            contributor: Contributor {
                display_name: "Lukas".into(),
                origin_user_id: origin_user_id.map(str::to_string),
            },
            kind: "image".into(),
            taken_at: None,
            exif: None,
        }
    }

    fn household(ids: &[&str]) -> std::collections::HashSet<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn a_photo_is_household_authored_when_its_author_is_one_of_our_people() {
        assert!(authored_by_household(
            &reference(Some("user-1")),
            &household(&["user-1", "user-2"])
        ));
    }

    #[test]
    fn a_photo_from_anyone_else_is_not() {
        assert!(!authored_by_household(
            &reference(Some("user-9")),
            &household(&["user-1"])
        ));
    }

    #[test]
    fn a_photo_that_names_no_author_is_not() {
        assert!(!authored_by_household(
            &reference(None),
            &household(&["user-1"])
        ));
    }

    #[test]
    fn nobody_is_household_authored_when_we_know_of_no_people() {
        assert!(!authored_by_household(
            &reference(Some("user-1")),
            &household(&[])
        ));
    }
}
