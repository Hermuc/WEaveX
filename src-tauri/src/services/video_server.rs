//! Local HTTP range server that streams a workshop video on demand.
//!
//! An external player (mpv) is pointed at `http://127.0.0.1:<port>/video`. It
//! sees a normal file of `total_size` bytes. Each `Range` request is answered by
//! fetching only the chunks that overlap the requested window — so playback
//! starts before the file is downloaded and seeking is cheap.
//!
//! Bound to loopback only; no auth, no TLS. The port is chosen by the OS so
//! multiple streams can coexist.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;

use super::video_stream::ResolvedVideo;

/// A running range server for one video.
pub struct VideoServer {
    pub port: u16,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl VideoServer {
    /// URL to hand to the player.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/video", self.port)
    }

    /// Stop the server.
    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for VideoServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start serving `video` on a random loopback port.
pub async fn serve(video: ResolvedVideo) -> anyhow::Result<VideoServer> {
    let total = video.info.total_size;
    let filename = video.info.filename.clone();
    let state = Arc::new(ServerState {
        video: Arc::new(video),
        total,
        content_type: content_type_for(&filename),
    });

    let app = Router::new()
        .route("/video", get(handle_video).head(handle_head))
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        let server = axum::serve(listener, app).with_graceful_shutdown(async {
            let _ = rx.await;
        });
        if let Err(e) = server.await {
            log::error!("[video-server] serve error: {e}");
        }
    });

    Ok(VideoServer {
        port,
        shutdown: Some(tx),
    })
}

struct ServerState {
    video: Arc<ResolvedVideo>,
    total: u64,
    content_type: &'static str,
}

fn content_type_for(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".webm") {
        "video/webm"
    } else if lower.ends_with(".mkv") {
        "video/x-matroska"
    } else if lower.ends_with(".avi") {
        "video/x-msvideo"
    } else {
        "video/mp4"
    }
}

async fn handle_head(State(st): State<Arc<ServerState>>) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(st.content_type),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Ok(v) = HeaderValue::from_str(&st.total.to_string()) {
        headers.insert(header::CONTENT_LENGTH, v);
    }
    (StatusCode::OK, headers).into_response()
}

/// Parse a single-range `Range: bytes=...` header into [start, end_inclusive].
/// Returns None for absent/unsupported (caller then serves the whole file).
fn parse_range(header: Option<&HeaderValue>, total: u64) -> Option<(u64, u64)> {
    let value = header?.to_str().ok()?;
    let spec = value.strip_prefix("bytes=")?;
    // Only the first range of a multi-range request is honoured; players do not
    // send multi-range for progressive playback.
    let first = spec.split(',').next()?.trim();
    let (start_s, end_s) = first.split_once('-')?;
    let (start, end) = if start_s.is_empty() {
        // suffix form: bytes=-N
        let n: u64 = end_s.parse::<u64>().ok()?;
        if n == 0 || total == 0 {
            return None;
        }
        (total.saturating_sub(n), total - 1)
    } else {
        let start: u64 = start_s.parse::<u64>().ok()?;
        let end: u64 = if end_s.is_empty() {
            total.saturating_sub(1)
        } else {
            end_s.parse::<u64>().ok()?.min(total.saturating_sub(1))
        };
        (start, end)
    };
    if start > end || start >= total {
        return None;
    }
    Some((start, end))
}

async fn handle_video(State(st): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    let range = parse_range(headers.get(header::RANGE), st.total);

    let (start, end) = match range {
        Some((s, e)) => (s, e),
        None => {
            // No Range: serve from the start. Streaming the entire body is fine
            // for a player that just probes; it will cancel after the header.
            (0, st.total.saturating_sub(1))
        }
    };
    let len = end - start + 1;

    // Fetch the requested window (chunk-aligned internally).
    let data = match st.video.read_range(start, len).await {
        Ok(d) => d,
        Err(e) => {
            log::error!("[video-server] read_range {start}+{len} failed: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "stream error").into_response();
        }
    };

    let partial = headers.contains_key(header::RANGE);
    let status = if partial {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(st.content_type),
    );
    resp_headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Ok(v) = HeaderValue::from_str(&len.to_string()) {
        resp_headers.insert(header::CONTENT_LENGTH, v);
    }
    if partial {
        let cr = format!("bytes {}-{}/{}", start, end, st.total);
        if let Ok(v) = HeaderValue::from_str(&cr) {
            resp_headers.insert(header::CONTENT_RANGE, v);
        }
    }

    (status, resp_headers, Body::from(data)).into_response()
}
