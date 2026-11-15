/** immich/stub_refresh.rs — what an origin's later edits change on a stub that already exists. See ARCHITECTURE.md. */
use crate::config::unix_millis_from_iso;
use crate::immich::client::composed_description;
use crate::immich::refs::AssetRef;
use serde_json::{json, Map, Value};

const COORDINATE_TOLERANCE: f64 = 1e-5;

fn current_number(current: &Value, field: &str) -> Option<f64> {
    current.pointer(&format!("/exifInfo/{field}"))?.as_f64()
}

fn same_instant(wanted: &str, current: Option<&str>) -> bool {
    match (
        unix_millis_from_iso(wanted),
        current.and_then(unix_millis_from_iso),
    ) {
        (Some(wanted), Some(current)) => wanted == current,
        _ => false,
    }
}

pub fn stub_metadata_update(reference: &AssetRef, current: &Value) -> Map<String, Value> {
    let mut update = Map::new();
    let wanted_description = composed_description(reference);
    let current_description = current
        .pointer("/exifInfo/description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if wanted_description != current_description {
        update.insert("description".into(), json!(wanted_description));
    }
    if let Some(exif) = &reference.exif {
        if let (Some(latitude), Some(longitude)) = (exif.latitude, exif.longitude) {
            let at_place = |field: &str, wanted: f64| {
                current_number(current, field)
                    .is_some_and(|now| (now - wanted).abs() <= COORDINATE_TOLERANCE)
            };
            if !at_place("latitude", latitude) || !at_place("longitude", longitude) {
                update.insert("latitude".into(), json!(latitude));
                update.insert("longitude".into(), json!(longitude));
            }
        }
        if let Some(rating) = exif.rating.filter(|rating| *rating != 0) {
            if current_number(current, "rating") != Some(rating as f64) {
                update.insert("rating".into(), json!(rating));
            }
        }
    }
    if let Some(taken_at) = &reference.taken_at {
        let current_taken = current
            .pointer("/exifInfo/dateTimeOriginal")
            .and_then(Value::as_str);
        if unix_millis_from_iso(taken_at).is_some() && !same_instant(taken_at, current_taken) {
            update.insert("dateTimeOriginal".into(), json!(taken_at));
        }
    }
    update
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::immich::refs::{Contributor, RefExif};

    fn reference(exif: RefExif, taken_at: Option<&str>) -> AssetRef {
        AssetRef {
            origin_asset: "a1".into(),
            checksum: "sum".into(),
            checksum_alg: None,
            contributor: Contributor {
                display_name: "Anna".into(),
                origin_user_id: None,
            },
            kind: "image".into(),
            taken_at: taken_at.map(str::to_string),
            exif: Some(exif),
        }
    }

    fn settled() -> (AssetRef, Value) {
        let exif = RefExif {
            latitude: Some(50.5),
            longitude: Some(8.25),
            description: Some("Lake".into()),
            rating: Some(4),
            ..Default::default()
        };
        let current = json!({
            "exifInfo": {
                "latitude": 50.5,
                "longitude": 8.25,
                "description": "Lake\n\nShared by Anna",
                "rating": 4,
                "dateTimeOriginal": "2024-05-01T12:30:00.000+00:00"
            }
        });
        (reference(exif, Some("2024-05-01T12:30:00.000Z")), current)
    }

    #[test]
    fn a_stub_that_already_matches_needs_nothing() {
        let (reference, current) = settled();
        assert!(stub_metadata_update(&reference, &current).is_empty());
    }

    #[test]
    fn a_new_caption_is_written() {
        let (mut reference, current) = settled();
        reference.exif.as_mut().unwrap().description = Some("Mountain".into());
        let update = stub_metadata_update(&reference, &current);
        assert_eq!(update["description"], "Mountain\n\nShared by Anna");
        assert_eq!(update.len(), 1);
    }

    #[test]
    fn a_moved_place_is_written_as_a_pair() {
        let (mut reference, current) = settled();
        reference.exif.as_mut().unwrap().latitude = Some(48.1);
        let update = stub_metadata_update(&reference, &current);
        assert_eq!(update["latitude"], 48.1);
        assert_eq!(update["longitude"], 8.25);
    }

    #[test]
    fn a_place_the_stub_lacks_is_written() {
        let (reference, mut current) = settled();
        current["exifInfo"]["latitude"] = Value::Null;
        current["exifInfo"]["longitude"] = Value::Null;
        let update = stub_metadata_update(&reference, &current);
        assert_eq!(update["latitude"], 50.5);
        assert_eq!(update["longitude"], 8.25);
    }

    #[test]
    fn a_changed_rating_is_written_and_a_zero_one_is_not() {
        let (mut reference, current) = settled();
        reference.exif.as_mut().unwrap().rating = Some(5);
        assert_eq!(stub_metadata_update(&reference, &current)["rating"], 5);
        reference.exif.as_mut().unwrap().rating = Some(0);
        assert!(!stub_metadata_update(&reference, &current).contains_key("rating"));
    }

    #[test]
    fn a_changed_taken_date_is_written() {
        let (mut reference, current) = settled();
        reference.taken_at = Some("2024-06-02T08:00:00.000Z".into());
        let update = stub_metadata_update(&reference, &current);
        assert_eq!(update["dateTimeOriginal"], "2024-06-02T08:00:00.000Z");
    }

    #[test]
    fn the_same_instant_in_another_notation_is_not_a_change() {
        let (mut reference, current) = settled();
        reference.taken_at = Some("2024-05-01T14:30:00+02:00".into());
        assert!(stub_metadata_update(&reference, &current).is_empty());
    }

    #[test]
    fn a_ref_that_says_nothing_about_a_field_leaves_it_alone() {
        let (mut reference, current) = settled();
        reference.exif = Some(RefExif {
            description: Some("Lake".into()),
            ..Default::default()
        });
        reference.taken_at = None;
        assert!(stub_metadata_update(&reference, &current).is_empty());
    }
}
