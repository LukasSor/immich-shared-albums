/** db/import.rs — copying every table from one database to another, for moving a SQLite install onto Postgres. See ARCHITECTURE.md. */
use super::{Db, Step};
use crate::store::StoreError;

const CHUNK: usize = 2_000;

pub const TABLES: [(&str, &[&str]); 11] = [
    ("kv", &["name", "value"]),
    (
        "seen",
        &[
            "id",
            "mapping",
            "checksum",
            "localAsset",
            "originAsset",
            "storedFull",
        ],
    ),
    ("seen_activity", &["tag", "mapping"]),
    (
        "trail_pending",
        &[
            "id",
            "albumId",
            "mappingId",
            "event",
            "text",
            "at",
            "attempts",
        ],
    ),
    ("cache", &["key", "size", "lastUsed"]),
    ("offered", &["id", "mapping", "asset"]),
    ("added", &["album", "\"user\""]),
    (
        "peers",
        &[
            "pub",
            "name",
            "version",
            "protocol",
            "features",
            "via",
            "firstSeenAt",
            "relayHint",
            "lastAddrs",
            "advertisedHost",
        ],
    ),
    (
        "mappings",
        &[
            "id",
            "role",
            "albumId",
            "albumName",
            "peer",
            "remoteAlbumId",
            "remoteMappingId",
            "permissions",
            "hostSlug",
            "via",
            "forPeerUserIds",
            "albumOwnerName",
            "albumOwnerId",
            "adopted",
            "reunified",
            "dead",
            "deadAt",
            "deadReason",
            "failCount",
            "localVersion",
            "remoteVersion",
            "commentCount",
            "remoteCommentCount",
        ],
    ),
    (
        "contributors",
        &[
            "slug",
            "userId",
            "apiKey",
            "password",
            "avatarDone",
            "viaPeer",
            "peerUserId",
            "homePeer",
        ],
    ),
    (
        "published_albums",
        &[
            "peer",
            "direction",
            "ownerUserId",
            "name",
            "assetCount",
            "startDate",
            "endDate",
            "ownerName",
        ],
    ),
];

const COUNTED_IDS: [&str; 3] = ["seen", "trail_pending", "offered"];

fn placeholders(width: usize) -> String {
    (1..=width)
        .map(|n| format!("?{n}"))
        .collect::<Vec<_>>()
        .join(",")
}

pub fn copy_all(from: &Db, to: &Db) -> Result<usize, StoreError> {
    let mut copied = 0;
    for (index, (table, columns)) in TABLES.iter().enumerate() {
        let rows = from.rows(&format!("SELECT {} FROM {table}", columns.join(", ")), &[])?;
        let clear = format!("DELETE FROM {table}");
        let insert = format!(
            "INSERT INTO {table} ({}) VALUES ({})",
            columns.join(", "),
            placeholders(columns.len())
        );
        let mut first = vec![Step {
            sql: &clear,
            params: vec![],
        }];
        let mut pending: Vec<Step> = Vec::new();
        for row in &rows {
            pending.push(Step {
                sql: &insert,
                params: row.0.to_vec(),
            });
            if pending.len() == CHUNK {
                first.append(&mut pending);
                to.batch(&first)?;
                first.clear();
            }
        }
        first.append(&mut pending);
        if !first.is_empty() {
            to.batch(&first)?;
        }
        copied += rows.len();
        crate::log!(
            "copied {} row(s) of {table} ({}/{})",
            rows.len(),
            index + 1,
            TABLES.len()
        );
    }
    to.reset_id_sequences(&COUNTED_IDS)?;
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params;
    use crate::store::Store;

    #[test]
    fn every_table_the_schema_creates_is_in_the_copy_list() {
        let store = Store::open_in_memory().unwrap();
        let tables = store
            .raw_db()
            .rows(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                &[],
            )
            .unwrap();
        for table in tables {
            let name = table.text(0).unwrap();
            assert!(
                TABLES.iter().any(|(listed, _)| *listed == name),
                "{name} is created by the schema but never copied"
            );
        }
    }

    #[test]
    fn every_listed_column_exists() {
        let store = Store::open_in_memory().unwrap();
        for (table, columns) in TABLES {
            store
                .raw_db()
                .rows(&format!("SELECT {} FROM {table}", columns.join(", ")), &[])
                .unwrap_or_else(|e| panic!("{table}: {e}"));
        }
    }

    #[test]
    fn a_copy_carries_rows_ids_and_nulls_across() {
        let from = Store::open_in_memory().unwrap();
        from.seen_add("m1", "c1", "a1", Some("o1"), false).unwrap();
        from.seen_add("m1", "c2", "a2", None, true).unwrap();
        from.kv_set("settings", &serde_json::json!({"a": 1}))
            .unwrap();
        let to = Store::open_in_memory().unwrap();
        to.seen_add("stale", "x", "y", None, false).unwrap();
        let copied = copy_all(from.raw_db(), to.raw_db()).unwrap();
        assert!(copied >= 3);
        assert!(!to.seen_has("stale", "x").unwrap());
        let rows = to.seen_for_mapping("m1").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].origin_asset.as_deref(), Some("o1"));
        assert_eq!(rows[1].origin_asset, None);
        assert!(rows[1].stored_full);
        assert_eq!(to.kv("settings").unwrap().unwrap()["a"], 1);
        let ids = to
            .raw_db()
            .rows("SELECT id FROM seen ORDER BY id", params![])
            .unwrap();
        assert_eq!(ids.len(), 2);
    }
}
