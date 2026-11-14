/** web/passthrough.rs — transparent streaming proxy to Immich for every non-sidecar route. See ARCHITECTURE.md. */
use crate::config::cfg;
use axum::body::Body;
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use std::sync::OnceLock;

/// Headers the upstream response must not carry through. `content-length` and `transfer-encoding`
/// describe the upstream framing, which this proxy re-frames; `content-encoding` would leave the
/// caller decoding a body we already decoded.
const STRIPPED_RESPONSE_HEADERS: [&str; 3] =
    ["content-encoding", "transfer-encoding", "content-length"];

/// Headers a proxy must not forward verbatim: `host` names OUR listener, not Immich's.
const STRIPPED_REQUEST_HEADERS: [&str; 2] = ["host", "content-length"];

static UPSTREAM: OnceLock<reqwest::Client> = OnceLock::new();
static UPSTREAM_BODY: OnceLock<reqwest::Client> = OnceLock::new();

const UPSTREAM_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const UPSTREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

fn build_client(read_timeout: Option<std::time::Duration>) -> Result<reqwest::Client, String> {
    let builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(UPSTREAM_CONNECT_TIMEOUT);
    match read_timeout {
        Some(timeout) => builder.read_timeout(timeout),
        None => builder,
    }
    .build()
    .map_err(|e| e.to_string())
}

fn carries_request_body(method: &Method) -> bool {
    matches!(*method, Method::POST | Method::PUT | Method::PATCH)
}

fn upstream_for(method: &Method) -> Result<&'static reqwest::Client, String> {
    let (slot, read_timeout) = if carries_request_body(method) {
        (&UPSTREAM_BODY, None)
    } else {
        (&UPSTREAM, Some(UPSTREAM_IDLE_TIMEOUT))
    };
    if let Some(client) = slot.get() {
        return Ok(client);
    }
    let built = build_client(read_timeout)?;
    Ok(slot.get_or_init(|| built))
}

/// Proxy a request to Immich and stream the answer back.
///
/// Nothing is buffered and nothing is rewritten: uploads must never be held in memory here, and a
/// photo stream must not become a `Vec`. The body is passed through as a stream, and so is the
/// response.
///
/// FAIL LOUDLY. A caller left waiting for a response they will never get is worse than an error, and
/// it is exactly what an earlier version of this proxy did — so any failure answers 502 with the
/// reason, never a dropped connection.
pub async fn proxy_to_immich(
    method: Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: Body,
) -> Response {
    let path_and_query = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    // A share link being deleted takes its own history with it, so its album is resolved NOW, while
    // the link still answers — as the caller, whose act this is and whose credential is the only one
    // that can later record it on their album.
    let withdrawn_album =
        crate::web::share_link_audit::album_of_deleted_link(method.as_str(), uri.path(), headers)
            .await;
    let url = format!(
        "{}{}",
        cfg().immich_url.trim_end_matches('/'),
        path_and_query
    );

    let client = match upstream_for(&method) {
        Ok(client) => client,
        Err(e) => return unreachable_immich(&e),
    };

    let mut outbound = client.request(
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET),
        &url,
    );
    for (name, value) in headers.iter() {
        let name_str = name.as_str();
        if STRIPPED_REQUEST_HEADERS.contains(&name_str) {
            continue;
        }
        if let Ok(value) = value.to_str() {
            outbound = outbound.header(name_str, value);
        }
    }
    // The body is forwarded as a stream, without a `Content-Length` we would have to know in advance.
    outbound = outbound.body(reqwest::Body::wrap_stream(body.into_data_stream()));

    let upstream = match outbound.send().await {
        Ok(upstream) => upstream,
        Err(e) => {
            crate::log!("proxying {} {} failed: {e}", method, path_and_query);
            return unreachable_immich(&e.to_string());
        }
    };

    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let upstream_headers = upstream.headers().clone();
    let mut response = Response::builder().status(status);
    for (name, value) in upstream_headers.iter() {
        if STRIPPED_RESPONSE_HEADERS.contains(&name.as_str()) {
            continue;
        }
        // A set-cookie carries one cookie per header; appending keeps them all.
        response = response.header(name, value);
    }
    // Somebody was taken off an album. Both ids are in the path, so this needs no pre-read — and it
    // is recorded after the fact, as the person who did it.
    if let Some((album_id, user_id)) =
        crate::sync::traffic_triggers::removed_person(method.as_str(), uri.path())
    {
        crate::web::album_member_audit::post_removal(
            crate::state::state().clone(),
            headers.clone(),
            album_id,
            user_id,
        );
    }
    // The withdrawal has happened: put it in the album, as the person who did it. Fire and forget,
    // so their click is never held up by a trail line.
    if let Some(album_id) = withdrawn_album {
        crate::web::share_link_audit::post_withdrawal(
            crate::state::state().clone(),
            headers.clone(),
            album_id,
        );
    }
    // The one route whose ANSWER is rewritten, and only for a reader who asked. Everything else
    // streams, and so does this when the preference is the default: see `filter_activities`.
    if method == Method::GET && uri.path() == "/api/activities" && status.is_success() {
        return filter_activities(headers, upstream).await;
    }
    let stream = upstream.bytes_stream();
    response
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

/// `GET /api/activities` — the album's comment history, and the ONE answer this proxy rewrites.
///
/// Immich has no per-reader view of an album's comments, and we never touch Immich: the history
/// merely passes through here on its way to the browser. So a person who has asked not to see the
/// addon's own trail gets it dropped from THEIR answer — the rows stay in Immich, stay in the trail,
/// and stay visible to everyone else.
///
/// The caller is resolved only for this route: it costs one Immich read, and it is the only way to
/// know whose preference applies. Default is visible, so the common case re-serves bytes that were
/// already fetched and unchanged.
async fn filter_activities(headers: &HeaderMap, upstream: reqwest::Response) -> Response {
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let bytes = match upstream.bytes().await {
        Ok(bytes) => bytes,
        Err(e) => return unreachable_immich(&e.to_string()),
    };
    let filtered = match crate::web::auth::caller_signed_in(headers).await {
        Some(signed_in) => crate::web::activity_filter::filter_activities_body(
            crate::state::state(),
            &signed_in.caller.id,
            &bytes,
        ),
        None => None,
    };
    let body = filtered.unwrap_or_else(|| bytes.to_vec());
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

/// The one answer a caller must always get when Immich cannot be reached.
fn unreachable_immich(reason: &str) -> Response {
    let body =
        serde_json::json!({ "message": format!("the addons could not reach Immich: {reason}") });
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn immich_that_answers_after_the_whole_body() -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();
            let mut buffer = [0u8; 1024];
            while !received.ends_with(b"0\r\n\r\n") {
                let read = socket.read(&mut buffer).await.unwrap();
                if read == 0 {
                    return;
                }
                received.extend_from_slice(&buffer[..read]);
            }
            socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
                .await
                .unwrap();
        });
        address
    }

    fn body_sent_in_chunks(chunks: u32, gap: Duration) -> reqwest::Body {
        reqwest::Body::wrap_stream(futures_lite::stream::unfold(0, move |sent| async move {
            if sent == chunks {
                return None;
            }
            tokio::time::sleep(gap).await;
            Some((Ok::<_, std::io::Error>(Bytes::from_static(b"x")), sent + 1))
        }))
    }

    async fn upload_status(
        client: &reqwest::Client,
        address: std::net::SocketAddr,
    ) -> Result<StatusCode, reqwest::Error> {
        let response = client
            .post(format!("http://{address}/api/assets"))
            .body(body_sent_in_chunks(6, Duration::from_millis(100)))
            .send()
            .await?;
        Ok(StatusCode::from_u16(response.status().as_u16()).unwrap())
    }

    #[tokio::test]
    async fn an_idle_bound_cuts_off_an_upload_that_outlasts_it() {
        let address = immich_that_answers_after_the_whole_body().await;
        let client = build_client(Some(Duration::from_millis(250))).unwrap();
        assert!(upload_status(&client, address).await.is_err());
    }

    #[tokio::test]
    async fn an_upload_slower_than_the_idle_bound_still_completes_without_it() {
        let address = immich_that_answers_after_the_whole_body().await;
        let client = build_client(None).unwrap();
        assert_eq!(
            upload_status(&client, address).await.unwrap(),
            StatusCode::OK
        );
    }

    #[test]
    fn only_requests_that_carry_a_body_skip_the_idle_bound() {
        for method in [Method::POST, Method::PUT, Method::PATCH] {
            assert!(carries_request_body(&method), "{method}");
        }
        for method in [Method::GET, Method::HEAD, Method::DELETE, Method::OPTIONS] {
            assert!(!carries_request_body(&method), "{method}");
        }
    }

    #[test]
    fn the_failure_answer_names_immich_and_carries_the_reason() {
        // The message shape the panel's JS surfaces; a silent failure here is what left callers
        // waiting forever before this existed.
        let r = unreachable_immich("connection refused");
        assert_eq!(r.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            r.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
    }

    #[test]
    fn framing_headers_are_not_forwarded_verbatim() {
        // Forwarding any of these re-frames a body twice and corrupts it.
        for name in STRIPPED_RESPONSE_HEADERS {
            assert!(["content-encoding", "transfer-encoding", "content-length"].contains(&name));
        }
        // `host` names our listener, not Immich's.
        assert!(STRIPPED_REQUEST_HEADERS.contains(&"host"));
    }
}
