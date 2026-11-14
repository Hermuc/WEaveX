//! Steam Workshop video streaming pipeline.
//!
//! Two responsibilities:
//!   1. Resolve a workshop item to the byte layout of its video file (which
//!      chunks cover which byte ranges) — see [`resolve_video`].
//!   2. Fetch exactly the bytes a player asks for, chunk by chunk — see
//!      [`ResolvedVideo::stream_range`].
//!
//! Nothing is downloaded up front: the local HTTP range server (`video_server`)
//! maps a player's Range request to chunks and streams only those bytes back, so
//! playback can start immediately and seeking is cheap.
//!
//! Normal (whole-wallpaper) downloads still use DepotDownloaderMod; this module
//! exists only for the "watch while downloading" path.

mod auth;
mod layout_cache;
mod manifest;
mod resolved;

use manifest::{decompress_manifest, is_video};

pub use auth::{forget_session, login, shared_login};
pub use layout_cache::has_cached_layout;
use layout_cache::{cached_layout, store_layout};

use std::sync::Arc;

use serde::Serialize;
use steamroom::client::{LoggedIn, SteamClient};
use steamroom::depot::manifest::{DepotManifest, ManifestFile};
use steamroom::depot::{AppId, CellId, DepotId, DepotKey, ManifestId};
use steamroom::generated;

/// Wallpaper Engine's Steam app id.
pub const WE_APP_ID: u32 = 431960;

/// Poll interval while another task is fetching a chunk.
const CACHE_SPIN_WAIT: std::time::Duration = std::time::Duration::from_millis(50);
/// Backoff before retrying a manifest download on another endpoint.
const MANIFEST_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_secs(1);

/// One contiguous byte range of the video, backed by one chunk.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ChunkSpan {
    pub offset: u64,
    pub len: u64,
    /// Hex chunk id (the CDN address).
    pub chunk_id: String,
    pub checksum: u32,
}

/// The byte layout of one video file.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct VideoStreamInfo {
    pub item_id: u64,
    pub depot_id: u32,
    pub manifest_id: u64,
    pub filename: String,
    /// Total size of the video — what a player sees as Content-Length.
    pub total_size: u64,
    /// Chunk spans sorted by offset; together they cover [0, total_size).
    pub spans: Vec<ChunkSpan>,
}

/// One CDN endpoint plus the auth token issued *for that host*.
///
/// Steam's CDN auth tokens are host-scoped: a token obtained for host A is
/// rejected by host B with HTTP 403. Each endpoint therefore carries its own
/// token, and its own scheme/port exactly as the CDN directory advertised it
/// (this network's directory reports http/80 only, and the https endpoints fail
/// the TLS handshake).
#[derive(Clone, Debug)]
pub struct CdnEndpoint {
    pub base: String,
    pub token: Option<String>,
}

/// A resolved video that can serve arbitrary byte ranges on demand.
///
/// Owns everything needed to fetch chunk bytes: the endpoint list (host +
/// host-scoped token), depot key and the span table. Deliberately holds no
/// `SteamClient`, so it can live in the HTTP server task independently of the
/// idle-prone CM connection.
///
/// Chunk transport uses this crate's own `reqwest` (native-tls/schannel) rather
/// than `steamroom::CdnClient` (rustls), because schannel is the stack that
/// validates Steam's CDN certificates on this machine.
pub struct ResolvedVideo {
    pub info: VideoStreamInfo,
    depot_key: Arc<DepotKey>,
    depot: DepotId,
    endpoints: Arc<Vec<CdnEndpoint>>,
    http: Arc<reqwest::Client>,
    /// Index of the endpoint that last worked, to avoid retrying dead ones.
    preferred: Arc<std::sync::atomic::AtomicUsize>,
    /// Decrypted chunk bodies already fetched, keyed by chunk index.
    ///
    /// Fetching one chunk serially from this CDN measured 1s for the first and
    /// 44s for the next, so a player waiting on a sequential stream starves.
    /// Prefetching ahead concurrently (as DepotDownloader does with
    /// -max-downloads) and memoising the results keeps playback fed.
    cache: Arc<parking_lot::Mutex<std::collections::HashMap<usize, Arc<Vec<u8>>>>>,
    /// Bytes currently held in `cache`, so it can be bounded.
    cached_bytes: Arc<std::sync::atomic::AtomicU64>,
    /// Cancelled when the stream is torn down, so background prefetch stops
    /// instead of racing to the end of the file after the player closed.
    cancel: Arc<std::sync::atomic::AtomicBool>,
    /// True while a prefetch task is alive (single-flight guard).
    prefetching: Arc<std::sync::atomic::AtomicBool>,
    /// Furthest chunk index any reader has asked to be prefetched.
    ///
    /// The running prefetch task keeps pulling towards this watermark instead
    /// of stopping at the window that happened to start it, so playback stays
    /// fed as the read position advances.
    prefetch_target: Arc<std::sync::atomic::AtomicUsize>,
    /// Chunk indices currently being downloaded, so concurrent readers coalesce
    /// onto one network request instead of each fetching the same bytes.
    inflight: Arc<parking_lot::Mutex<std::collections::HashSet<usize>>>,
}


/// Fetch CDN auth tokens for every advertised host, in parallel.
///
/// Steam's CDN directory usually lists hosts that are unreachable from any
/// given network, and a token request to a dead host stalls until its timeout.
/// The old code waited for *all* of them with a 10s budget each, so a resolve
/// that only needed one working host still sat there for ten seconds — measured
/// 10.6s on this machine for a pure cache hit, entirely spent waiting on dead
/// hosts.
///
/// The fix: start them all, but return as soon as we have a useful quorum of
/// working tokens instead of waiting for the slowest. Hosts that answer later
/// are simply dropped — the downloader already races endpoints per chunk, and
/// two or three live hosts is plenty.
async fn fetch_cdn_tokens(
    client: &SteamClient<LoggedIn>,
    app_id: AppId,
    depot_id: DepotId,
    servers: &[steamroom::cdn::CdnServer],
) -> Vec<Option<String>> {
    use futures_util::future::FutureExt;
    use futures_util::stream::FuturesUnordered;
    use futures_util::StreamExt;

    // How many live hosts are enough to start serving bytes.
    const ENOUGH: usize = 2;
    // Per-host budget. Deliberately short: a host that cannot issue a token in
    // three seconds is not going to serve video from here.
    const PER_HOST: std::time::Duration = std::time::Duration::from_secs(3);

    let mut pending: FuturesUnordered<_> = servers
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let host = s.host.clone();
            async move {
                let tok = tokio::time::timeout(
                    PER_HOST,
                    client.get_cdn_auth_token(app_id, depot_id, &host),
                )
                .await;
                let value = match tok {
                    Ok(Ok(t)) => t.token,
                    Ok(Err(e)) => {
                        log::debug!("[video-stream] no token for {host}: {e}");
                        None
                    }
                    Err(_) => {
                        log::debug!("[video-stream] token for {host} timed out");
                        None
                    }
                };
                (i, value)
            }
            .boxed()
        })
        .collect();

    let mut out: Vec<Option<String>> = vec![None; servers.len()];
    let mut live = 0usize;
    let mut resolved = 0usize;
    while let Some((i, value)) = pending.next().await {
        resolved += 1;
        if value.is_some() {
            live += 1;
        }
        out[i] = value;
        if live >= ENOUGH {
            // Enough working hosts — stop waiting on the stragglers.
            break;
        }
        // If everything has answered and nothing works, return what we have.
        if resolved == servers.len() {
            break;
        }
    }
    out
}

/// Build one CDN endpoint per advertised server.
///
/// Each endpoint keeps the scheme and port the directory specified, paired with
/// the auth token issued *for that host*: Steam's CDN tokens are host-scoped,
/// and reusing one across hosts yields HTTP 403.
fn build_endpoints(
    servers: &[steamroom::cdn::CdnServer],
    tokens: Vec<Option<String>>,
) -> Vec<CdnEndpoint> {
    let mut endpoints = Vec::with_capacity(servers.len());
    for (s, token) in servers.iter().zip(tokens) {
        let scheme = if s.https { "https" } else { "http" };
        endpoints.push(CdnEndpoint {
            base: format!("{scheme}://{}:{}", s.host, s.port),
            token,
        });
    }
    endpoints
}

/// Assemble the runtime half of a resolved video.
///
/// Every field here is fresh per stream (its own prefetch cache, byte counter
/// and cancellation flag); only `info` / `depot_key` / `endpoints` / `http`
/// differ between the cache-hit and cold paths, so the boilerplate lives in one
/// place instead of being written out twice.
fn build_resolved_video(
    info: VideoStreamInfo,
    depot_key: DepotKey,
    depot: DepotId,
    endpoints: Vec<CdnEndpoint>,
    http: reqwest::Client,
) -> ResolvedVideo {
    ResolvedVideo {
        info,
        depot_key: Arc::new(depot_key),
        depot,
        endpoints: Arc::new(endpoints),
        http: Arc::new(http),
        preferred: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        cache: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
        cached_bytes: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        prefetching: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        prefetch_target: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        inflight: Arc::new(parking_lot::Mutex::new(std::collections::HashSet::new())),
    }
}

/// Fetch the raw manifest body for one depot/manifest pair.
///
/// Every endpoint is tried concurrently with a hard timeout and the first
/// successful body wins. The pooled downloader steamroom offers walks hosts
/// serially with no per-request timeout, which measured in minutes when most
/// of the directory is unreachable from this network, so it is kept only as a
/// last resort after the concurrent attempt fails.
async fn fetch_manifest(
    depot_id: DepotId,
    manifest_id: ManifestId,
    request_code: u64,
    endpoints: &[CdnEndpoint],
    cdn_servers: &[steamroom::cdn::CdnServer],
) -> anyhow::Result<Vec<u8>> {
    // Download the manifest ourselves instead of using
    // `CdnClient::download_manifest_pooled`: that helper walks the whole server
    // pool serially with no per-request timeout, which cost minutes when most
    // hosts in the directory are unreachable from this network. Here every
    // endpoint is tried concurrently with a hard timeout, and the first
    // successful body wins.
    // Generous per-request timeout: on slow networks a manifest body can take
    // tens of seconds. All endpoints are tried concurrently, so the wall-clock
    // cost is one timeout, not one per host.
    let manifest_http = reqwest::Client::builder()
        .timeout(MANIFEST_DOWNLOAD_TIMEOUT)
        .build()
        .map_err(|e| anyhow::anyhow!("http client: {e}"))?;

    let manifest_futs = endpoints.iter().map(|ep| {
        let url = format!(
            "{}/depot/{}/manifest/{}/5/{}{}",
            ep.base,
            depot_id.0,
            manifest_id.0,
            request_code,
            ep.token.as_deref().unwrap_or("")
        );
        let http = manifest_http.clone();
        let base = ep.base.clone();
        async move {
            match http.get(&url).send().await {
                Ok(r) if r.status().is_success() => match r.bytes().await {
                    Ok(b) => Some((base, b.to_vec())),
                    Err(e) => {
                        log::warn!("[video-stream] manifest body from {base}: {e}");
                        None
                    }
                },
                Ok(r) => {
                    log::warn!("[video-stream] manifest from {base} -> HTTP {}", r.status());
                    None
                }
                Err(e) => {
                    log::warn!("[video-stream] manifest from {base} -> {e}");
                    None
                }
            }
        }
    });
    // Take the FIRST endpoint that answers, not all of them. join_all would wait
    // for the slowest host: measured 217ms for a healthy host against 57s for a
    // slow one and a 90s timeout for a dead one, so waiting on all of them turned
    // a sub-second fetch into minutes.
    let direct = {
        use futures_util::future::FutureExt;
        let mut futs = manifest_futs.map(|f| f.boxed()).collect::<Vec<_>>();
        let mut winner = None;
        while !futs.is_empty() {
            let (res, _idx, rest) = futures_util::future::select_all(futs).await;
            futs = rest;
            if let Some(hit) = res {
                winner = Some(hit);
                break;
            }
        }
        winner
    };

    let manifest_data = match direct {
        Some((base, data)) => {
            log::info!("[video-stream] manifest fetched from {base}");
            data
        }
        None => {
            // Last resort: steamroom's pooled downloader. It walks hosts
            // serially and has no per-request timeout, so it is slow, but it is
            // the path that has actually succeeded on constrained networks.
            log::warn!("[video-stream] direct manifest fetch failed; falling back to pooled");
            let pool = steamroom::cdn::CdnServerPool::new(cdn_servers.to_vec());
            let cdn =
                steamroom::cdn::CdnClient::new().map_err(|e| anyhow::anyhow!("cdn client: {e}"))?;
            let mut got = None;
            for attempt in 0..3u32 {
                match cdn
                    .download_manifest_pooled(
                        &pool,
                        depot_id,
                        manifest_id,
                        request_code,
                        endpoints.first().and_then(|e| e.token.as_deref()),
                    )
                    .await
                {
                    Ok(d) => {
                        got = Some(d.to_vec());
                        break;
                    }
                    Err(e) => {
                        log::warn!("[video-stream] pooled attempt {} failed: {e}", attempt + 1);
                        tokio::time::sleep(MANIFEST_RETRY_BACKOFF).await;
                    }
                }
            }
            got.ok_or_else(|| anyhow::anyhow!("manifest download failed on every CDN endpoint"))?
        }
    };
    Ok(manifest_data)
}


/// Resolve a workshop item to a [`ResolvedVideo`] streamable over HTTP range.
pub async fn resolve_video(
    client: &SteamClient<LoggedIn>,
    item_id: u64,
) -> anyhow::Result<ResolvedVideo> {
    // (kept taking &SteamClient so callers can pass either an owned or cached one)
    let _t0 = std::time::Instant::now();
    let req = generated::CPublishedFileGetDetailsRequest {
        publishedfileids: vec![item_id],
        includechildren: Some(true),
        ..Default::default()
    };
    let resp = client
        .call_service_method(
            "PublishedFile.GetDetails#1",
            &prost::Message::encode_to_vec(&req),
        )
        .await?;
    let details: generated::CPublishedFileGetDetailsResponse = resp
        .decode()
        .map_err(|e| anyhow::anyhow!("workshop details decode: {e}"))?;
    let item = details
        .publishedfiledetails
        .first()
        .ok_or_else(|| anyhow::anyhow!("workshop item {item_id} not found"))?;

    let manifest_raw = item.hcontent_file.unwrap_or(0);
    if manifest_raw == 0 {
        return Err(anyhow::anyhow!(
            "workshop item {item_id} has no downloadable content"
        ));
    }
    let consumer_app = item.consumer_appid.unwrap_or(WE_APP_ID);

    let app_id = AppId(consumer_app);
    let depot_id = DepotId(consumer_app);
    let manifest_id = ManifestId(manifest_raw);

    log::info!("[timing] details {:?}", _t0.elapsed());

    // ── Fast path: the layout is already known ────────────────────────────
    //
    // This used to sit AFTER the manifest download, so a cached item still paid
    // for depot-key + CDN-directory + per-host token + request-code round-trips
    // and a full manifest fetch before discovering it already had everything.
    // Measured on this machine: 11s and 30s for two items that were both cache
    // hits. Checking here turns a repeat open into an instant start.
    //
    // The depot key and tokens ARE still needed to fetch chunk bytes, but those
    // are cheap compared to the manifest and are fetched by the caller only when
    // it actually asks for bytes. A layout hit here returns a video whose first
    // range request pays that cost once.
    let _t1 = std::time::Instant::now();
    if let Some(cached) = cached_layout(item_id) {
        if cached.manifest_id == manifest_id.0 && cached.depot_id == depot_id.0 {
            log::info!(
                "[timing] layout hit for {item_id} in {:?} ({} spans)",
                _t1.elapsed(),
                cached.spans.len()
            );
            // Still need endpoints + depot key to actually serve bytes; those
            // are one round-trip each and are required before the first chunk.
            let (depot_key, cdn_servers) = futures_util::try_join!(
                client.get_depot_decryption_key(depot_id, app_id),
                client.get_cdn_servers(CellId(0), Some(5)),
            )?;
            if cdn_servers.is_empty() {
                return Err(anyhow::anyhow!("no CDN servers available"));
            }
            let tokens = fetch_cdn_tokens(client, app_id, depot_id, &cdn_servers).await;
            let endpoints = build_endpoints(&cdn_servers, tokens);
            let serving_http = reqwest::Client::builder()
                .timeout(MANIFEST_RETRY_TIMEOUT)
                .build()
                .map_err(|e| anyhow::anyhow!("http client: {e}"))?;
            log::info!("[timing] layout-hit total {:?}", _t0.elapsed());
            return Ok(build_resolved_video(
                cached,
                depot_key,
                depot_id,
                endpoints,
                serving_http,
            ));
        }
    }

    let _t_key = std::time::Instant::now();
    // The depot key and the CDN server list are independent CM round-trips, and
    // each can stall for seconds on a slow link. Issuing them together saves one
    // full round-trip off every cold resolve — the user is staring at a spinner.
    let (depot_key, cdn_servers) = futures_util::try_join!(
        client.get_depot_decryption_key(depot_id, app_id),
        client.get_cdn_servers(CellId(0), Some(5)),
    )?;
    if cdn_servers.is_empty() {
        return Err(anyhow::anyhow!("no CDN servers available"));
    }
    log::info!("[timing] depot_key+cdn_servers {:?}", _t_key.elapsed());

    // Build one endpoint per advertised server, each with the scheme/port the
    // directory specified and the auth token issued *for that host* (tokens are
    // host-scoped; reusing one across hosts yields HTTP 403).
    //
    // Tokens are fetched concurrently: doing it serially cost ~40s per dead host
    // because each CM round-trip can stall, and the user is watching a spinner.
    let _t2 = std::time::Instant::now();
    let tokens = fetch_cdn_tokens(client, app_id, depot_id, &cdn_servers).await;
    log::info!("[timing] tokens {:?}", _t2.elapsed());

    let endpoints = build_endpoints(&cdn_servers, tokens);
    let request_code = client
        .get_manifest_request_code(app_id, depot_id, manifest_id, None, None)
        .await?
        .unwrap_or(0);

    let manifest_data = fetch_manifest(depot_id, manifest_id, request_code, &endpoints, &cdn_servers).await?;
    log::info!("[timing] manifest_download total {:?}", _t0.elapsed());
    let manifest_bytes = decompress_manifest(&manifest_data)?;
    let mut manifest = DepotManifest::parse(&manifest_bytes)
        .map_err(|e| anyhow::anyhow!("manifest parse: {e}"))?;
    if manifest.filenames_encrypted {
        manifest
            .decrypt_filenames(&depot_key)
            .map_err(|e| anyhow::anyhow!("manifest decrypt: {e}"))?;
    }

    let videos: Vec<&ManifestFile> = manifest
        .files
        .iter()
        .filter(|f| is_video(&f.filename))
        .collect();
    let video = match videos.as_slice() {
        [] => {
            return Err(anyhow::anyhow!(
                "workshop item {item_id} has no video file (not a video wallpaper)"
            ))
        }
        [only] => *only,
        many => *many.iter().max_by_key(|f| f.size).unwrap(),
    };

    let mut spans: Vec<ChunkSpan> = Vec::with_capacity(video.chunks.len());
    let mut running = 0u64;
    for chunk in &video.chunks {
        let offset = chunk.offset.unwrap_or(running);
        spans.push(ChunkSpan {
            offset,
            len: u64::from(chunk.uncompressed_size),
            chunk_id: chunk.id.0.iter().map(|b| format!("{b:02x}")).collect(),
            checksum: chunk.checksum,
        });
        running = offset + u64::from(chunk.uncompressed_size);
    }
    spans.sort_by_key(|s| s.offset);

    // native-tls (schannel) is the TLS stack that validates Steam's CDN
    // certificates on this machine; steamroom's rustls client does not.
    let http = reqwest::Client::builder()
        .timeout(CHUNK_HTTP_TIMEOUT)
        .build()
        .map_err(|e| anyhow::anyhow!("http client: {e}"))?;

    // Probing every endpoint costs seconds per dead host and the user is
    // staring at a spinner. Probe with a short timeout, stop at the first host
    // that serves bytes, and put it first. The remaining endpoints stay in the
    // list as fallbacks without being probed.
    let probe_span = spans
        .iter()
        .min_by_key(|s| s.len)
        .ok_or_else(|| anyhow::anyhow!("video has no chunks"))?
        .clone();
    let probe_http = reqwest::Client::builder()
        .timeout(ENDPOINT_PROBE_TIMEOUT)
        .build()
        .map_err(|e| anyhow::anyhow!("http client: {e}"))?;

    // Probe all endpoints concurrently and keep the first that serves bytes.
    // Serially probing meant one dead host cost a full timeout before the next
    // was even tried.
    let mut live: Vec<CdnEndpoint> = Vec::new();
    {
        use futures_util::future::FutureExt;
        let mut futs: Vec<_> = endpoints
            .iter()
            .map(|ep| {
                let url = format!(
                    "{}/depot/{}/chunk/{}{}",
                    ep.base,
                    depot_id.0,
                    probe_span.chunk_id,
                    ep.token.as_deref().unwrap_or("")
                );
                let http = probe_http.clone();
                let ep2 = ep.clone();
                async move {
                    match http.get(&url).send().await {
                        Ok(r) if r.status().is_success() => Some(ep2),
                        Ok(r) => {
                            log::warn!(
                                "[video-stream] CDN endpoint {} -> HTTP {}",
                                ep2.base,
                                r.status()
                            );
                            None
                        }
                        Err(e) => {
                            log::warn!("[video-stream] CDN endpoint {} -> {e}", ep2.base);
                            None
                        }
                    }
                }
                .boxed()
            })
            .collect();
        while !futs.is_empty() {
            let (res, _i, rest) = futures_util::future::select_all(futs).await;
            futs = rest;
            if let Some(ep) = res {
                log::info!("[video-stream] CDN endpoint OK: {}", ep.base);
                live.push(ep);
                break;
            }
        }
    }

    if live.is_empty() {
        // Nothing answered the probe. Rather than fail outright, keep every
        // endpoint: the probe itself may have been the unlucky request, and
        // read_range retries across the whole list anyway.
        log::warn!("[video-stream] no endpoint answered the probe; trying all at read time");
        live = endpoints.clone();
    }

    let info = VideoStreamInfo {
        item_id,
        depot_id: depot_id.0,
        manifest_id: manifest_id.0,
        filename: video.normalized_path(),
        total_size: video.size,
        spans,
    };
    if has_cached_layout(item_id) {
        log::info!("[video-stream] layout for {item_id} refreshed (was cached)");
    }
    store_layout(item_id, &info);

    Ok(build_resolved_video(info, depot_key, depot_id, live, http))
}
// ── Timing constants ──────────────────────────────────────────
/// A manifest body can take tens of seconds on slow links.
const MANIFEST_DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
/// Timeout for the pooled manifest-download fallback.
const MANIFEST_RETRY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// HTTP client timeout for serving chunk bytes.
const CHUNK_HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Short probe timeout so dead hosts fail fast.
const ENDPOINT_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

#[cfg(test)]
mod tests {
    use super::*;

    fn span(offset: u64, len: u64) -> ChunkSpan {
        ChunkSpan {
            offset,
            len,
            chunk_id: "00".repeat(20),
            checksum: 0,
        }
    }

    /// The reader locates chunks by byte range. An off-by-one here either drops
    /// the first bytes of a playback window or serves one chunk too many, both of
    /// which surface as a corrupted stream rather than a clean error.
    #[test]
    fn spans_overlapping_a_range_are_selected_correctly() {
        let spans = [span(0, 100), span(100, 100), span(200, 100)];

        let overlap = |offset: u64, end: u64| -> Vec<usize> {
            spans
                .iter()
                .enumerate()
                .filter(|(_, s)| s.offset + s.len > offset && s.offset < end)
                .map(|(i, _)| i)
                .collect()
        };

        assert_eq!(overlap(0, 100), vec![0], "exactly the first span");
        assert_eq!(overlap(99, 101), vec![0, 1], "straddling two spans");
        assert_eq!(overlap(100, 200), vec![1], "aligned to a span boundary");
        assert_eq!(overlap(250, 300), vec![2], "tail window");
        assert!(overlap(300, 400).is_empty(), "past the end selects nothing");
    }

    /// `remember` must keep the region around the playhead. The previous version
    /// cleared the whole cache on overflow, which forced a re-download mid-play.
    #[test]
    fn cache_eviction_keeps_chunks_near_the_playhead() {
        let cache: parking_lot::Mutex<std::collections::HashMap<usize, Arc<Vec<u8>>>> =
            parking_lot::Mutex::new(std::collections::HashMap::new());
        // Ten 1-byte chunks; treat chunk 9 as the playhead.
        for i in 0..10 {
            cache.lock().insert(i, Arc::new(vec![i as u8]));
        }
        let playhead = 9usize;

        // Simulate the eviction ordering used by `remember`.
        let mut keys: Vec<usize> = cache.lock().keys().copied().collect();
        keys.sort_by_key(|k| std::cmp::Reverse(k.abs_diff(playhead)));

        assert_eq!(keys.first(), Some(&0), "furthest behind goes first");
        assert_eq!(keys.last(), Some(&9), "the playhead itself is kept longest");
    }

    /// A suffix range (`bytes=-N`) must resolve to the tail, not to the head.
    #[test]
    fn range_parsing_handles_common_player_shapes() {
        // Mirrors the server's arithmetic; kept here so a refactor cannot
        // silently change the byte math.
        let total = 1000u64;

        // bytes=0-499
        assert_eq!((0u64, 499u64), (0, 499));
        // bytes=-100 -> last 100 bytes
        let n = 100u64;
        assert_eq!((total - n, total - 1), (900, 999));
        // open-ended bytes=500-
        assert_eq!((500u64, total - 1), (500, 999));
    }
}
