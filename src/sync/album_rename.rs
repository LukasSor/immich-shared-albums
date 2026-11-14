use crate::immich::access::MappingAuth;
use crate::immich::client::Client;
use crate::state::State;
use crate::store::Mapping;

pub fn name_change<'a>(stored: &str, live: &'a str) -> Option<&'a str> {
    (!live.is_empty() && live != stored).then_some(live)
}

pub async fn rename_mirror(state: &State, client: &Client, mapping: &Mapping, wanted: &str) {
    let Ok(creds) = MappingAuth::for_mapping(state, mapping) else {
        return;
    };
    let body = serde_json::json!({ "albumName": wanted });
    if let Err(error) = client
        .json(
            reqwest::Method::PATCH,
            &format!("/albums/{}", mapping.album_id),
            &creds.auth(),
            Some(&body),
        )
        .await
    {
        crate::log!(
            "could not rename the mirror \"{}\": {error}",
            mapping.album_name
        );
        return;
    }
    if let Some(live) = state
        .collections()
        .mappings
        .iter_mut()
        .find(|m| m.id == mapping.id)
    {
        live.album_name = wanted.to_string();
    }
    let _ = state.save();
    crate::log!(
        "renamed the mirror \"{}\" to \"{wanted}\"",
        mapping.album_name
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_different_name_is_a_rename() {
        assert_eq!(
            name_change("Holidays", "Holidays 2026"),
            Some("Holidays 2026")
        );
    }

    #[test]
    fn the_same_name_is_not_a_rename() {
        assert_eq!(name_change("Holidays", "Holidays"), None);
    }

    #[test]
    fn an_empty_name_is_never_applied() {
        assert_eq!(name_change("Holidays", ""), None);
    }

    #[test]
    fn only_an_exact_match_counts_as_unchanged() {
        assert_eq!(name_change("Holidays", "holidays"), Some("holidays"));
        assert_eq!(name_change("Holidays", "Holidays "), Some("Holidays "));
    }
}
