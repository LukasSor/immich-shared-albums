use crate::immich::client::{Auth, Client};
use serde_json::Value;
use std::future::Future;
use std::time::Duration;

const WAIT_BUDGET: Duration = Duration::from_secs(90);
const POLL_INTERVAL: Duration = Duration::from_millis(1500);

fn has_thumbnail(asset: &Value) -> bool {
    asset.get("thumbhash").is_some_and(|hash| !hash.is_null())
}

async fn poll_until<Probe, Pending>(budget: Duration, interval: Duration, mut probe: Probe) -> bool
where
    Probe: FnMut() -> Pending,
    Pending: Future<Output = bool>,
{
    tokio::time::timeout(budget, async {
        while !probe().await {
            tokio::time::sleep(interval).await;
        }
    })
    .await
    .is_ok()
}

pub async fn wait_until_thumbnailed(client: &Client, asset_id: &str, key: &str) -> bool {
    poll_until(WAIT_BUDGET, POLL_INTERVAL, || async {
        matches!(
            client.get(&format!("/assets/{asset_id}"), &Auth::Key(key)).await,
            Ok(Some(asset)) if has_thumbnail(&asset)
        )
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn an_asset_has_a_thumbnail_once_immich_reports_a_thumbhash() {
        assert!(has_thumbnail(
            &json!({ "thumbhash": "3OcRJYB4d3h/iIeHeEh3eIhw+j2w" })
        ));
    }

    #[test]
    fn a_null_or_missing_thumbhash_means_no_thumbnail_yet() {
        assert!(!has_thumbnail(&json!({ "thumbhash": null })));
        assert!(!has_thumbnail(&json!({ "id": "a1" })));
    }

    #[tokio::test]
    async fn polling_stops_at_the_first_success() {
        let calls = AtomicUsize::new(0);
        let satisfied = poll_until(Duration::from_secs(5), Duration::from_millis(5), || {
            let attempt = calls.fetch_add(1, Ordering::SeqCst) + 1;
            async move { attempt >= 3 }
        })
        .await;
        assert!(satisfied);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn polling_gives_up_when_the_budget_is_spent() {
        let started = Instant::now();
        let satisfied = poll_until(
            Duration::from_millis(60),
            Duration::from_millis(5),
            || async { false },
        )
        .await;
        assert!(!satisfied);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn a_probe_that_never_answers_is_cancelled_with_the_budget() {
        let started = Instant::now();
        let satisfied = poll_until(
            Duration::from_millis(60),
            Duration::from_millis(5),
            std::future::pending::<bool>,
        )
        .await;
        assert!(!satisfied);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
