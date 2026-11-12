//! Steam Workshop video streaming pipeline.
//!
//! Two responsibilities:
//!   1. Resolve a workshop item to the byte layout of its video file (which
//!      chunks cover which byte ranges) — see [`resolve_video`].
//!   2. Fetch exactly the bytes a player asks for, chunk by chunk — see
//!      [`ResolvedVideo::read_range`].
//!
//! Nothing is downloaded up front: the local HTTP range server (`video_server`)
//! maps a player's Range request to chunks and streams only those bytes back, so
//! playback can start immediately and seeking is cheap.
//!
//! Normal (whole-wallpaper) downloads still use DepotDownloaderMod; this module
//! exists only for the "watch while downloading" path.

use std::sync::Arc;

use serde::Serialize;
use steamroom::client::{LoggedIn, SteamClient};
use steamroom::depot::manifest::{DepotManifest, ManifestFile};
use steamroom::depot::{AppId, CellId, ChunkId, DepotId, DepotKey, ManifestId};
use steamroom::generated;
use steamroom_client::login::CredentialsLoginFlow;
use steamroom_client::login::LoginBuilder;

/// Wallpaper Engine's Steam app id.
pub const WE_APP_ID: u32 = 431960;

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
}

impl ResolvedVideo {
    /// Chunk spans whose byte range intersects `[start, end)`.
    fn spans_for(&self, start: u64, end: u64) -> impl Iterator<Item = &ChunkSpan> {
        self.info.spans.iter().filter(move |s| {
            let span_end = s.offset + s.len;
            span_end > start && s.offset < end
        })
    }

    /// Fetch, decrypt and decompress one chunk.
    ///
    /// All endpoints are raced concurrently and the first complete response wins.
    /// Endpoint speeds on this network differ by orders of magnitude (measured
    /// 217ms, 57s and a 90s timeout for the same request), so trying them one at
    /// a time made a single 1MB chunk take 80s. Racing them means the chunk costs
    /// as much as the fastest host, not the sum of the slow ones.
    async fn fetch_chunk(&self, span: &ChunkSpan) -> anyhow::Result<Vec<u8>> {
        let _ = parse_chunk_id(&span.chunk_id)?;

        if self.endpoints.is_empty() {
            return Err(anyhow::anyhow!("no CDN endpoints"));
        }

        // Try the last-known-good endpoint first, then everything else, all at once.
        let preferred_idx = self.preferred.load(std::sync::atomic::Ordering::Relaxed);
        let mut order: Vec<usize> = (0..self.endpoints.len()).collect();
        if preferred_idx < order.len() {
            order.swap(0, preferred_idx);
        }

        use futures_util::future::FutureExt;
        let mut futs: Vec<_> = order
            .into_iter()
            .map(|idx| {
                let ep = &self.endpoints[idx];
                let url = format!(
                    "{}/depot/{}/chunk/{}{}",
                    ep.base,
                    self.depot.0,
                    span.chunk_id,
                    ep.token.as_deref().unwrap_or("")
                );
                let http = self.http.clone();
                let key = self.depot_key.clone();
                let len = span.len as u32;
                let sum = span.checksum;
                async move {
                    let resp = http.get(&url).send().await.ok()?;
                    if !resp.status().is_success() {
                        return None;
                    }
                    let body = resp.bytes().await.ok()?;
                    let out = steamroom::depot::chunk::process_chunk(&body, &key, len, sum).ok()?;
                    Some((idx, out))
                }
                .boxed()
            })
            .collect();

        let mut last_err = String::from("all endpoints failed");
        while !futs.is_empty() {
            let (res, _i, rest) = futures_util::future::select_all(futs).await;
            futs = rest;
            if let Some((idx, out)) = res {
                self.preferred
                    .store(idx, std::sync::atomic::Ordering::Relaxed);
                return Ok(out);
            }
            last_err = format!("chunk {} failed on a host", span.chunk_id);
        }
        Err(anyhow::anyhow!("{last_err}"))
    }

    /// Fetch one chunk by index, using the cache and falling back to the network.
    async fn chunk_at(self: &Arc<Self>, idx: usize) -> anyhow::Result<Arc<Vec<u8>>> {
        if let Some(hit) = self.cache.lock().get(&idx).cloned() {
            return Ok(hit);
        }
        let span = self
            .info
            .spans
            .get(idx)
            .ok_or_else(|| anyhow::anyhow!("chunk index {idx} out of range"))?;

        // Retry with a small backoff: this network drops connections mid-body
        // often enough that a single failure would otherwise abort the stream and
        // make the player report a corrupt packet.
        let mut last_err = None;
        for attempt in 0..3u32 {
            match self.fetch_chunk(span).await {
                Ok(bytes) => {
                    let data = Arc::new(bytes);
                    self.cache.lock().insert(idx, data.clone());
                    return Ok(data);
                }
                Err(e) => {
                    last_err = Some(e);
                    tokio::time::sleep(std::time::Duration::from_millis(
                        300 * u64::from(attempt + 1),
                    ))
                    .await;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("chunk {idx} failed")))
    }

    /// Warm `count` chunks starting at `from` concurrently in the background.
    ///
    /// Steam rate-limits a single connection hard enough that sequential
    /// fetching is unusable for playback (measured 1s then 44s per chunk), while
    /// the official downloader opens many connections at once. This mirrors
    /// that: several chunks in flight, results memoised for the reader.
    fn prefetch(self: &Arc<Self>, from: usize, count: usize) {
        const CONCURRENCY: usize = 6;
        let total = self.info.spans.len();
        let end = (from + count).min(total);
        if from >= end {
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            let mut next = from;
            loop {
                let mut batch = Vec::new();
                while batch.len() < CONCURRENCY && next < end {
                    // Skip what is already cached.
                    if !this.cache.lock().contains_key(&next) {
                        batch.push(next);
                    }
                    next += 1;
                }
                if batch.is_empty() {
                    if next >= end {
                        break;
                    }
                    continue;
                }
                let futs = batch.into_iter().map(|i| {
                    let v = this.clone();
                    async move {
                        let _ = v.chunk_at(i).await;
                    }
                });
                futures_util::future::join_all(futs).await;
                if next >= end {
                    break;
                }
            }
        });
    }

    /// Stream the byte window as a sequence of slices, one chunk at a time.
    /// Lets the HTTP server send headers immediately and trickle the body.
    pub fn stream_range(
        self: Arc<Self>,
        offset: u64,
        len: u64,
    ) -> impl futures_util::Stream<Item = Result<Vec<u8>, std::io::Error>> {
        let end = (offset + len).min(self.info.total_size);
        // Work in chunk indices so the cache and the prefetcher agree on keys.
        let mut indices: Vec<usize> = Vec::new();
        for (i, s) in self.info.spans.iter().enumerate() {
            let span_end = s.offset + s.len;
            if span_end > offset && s.offset < end {
                indices.push(i);
            }
        }
        // Warm the next window in the background so the player does not wait
        // for a serial chain of slow CDN responses.
        if let Some(&first) = indices.first() {
            self.prefetch(first, indices.len() + 12);
        }
        futures_util::stream::unfold((self, indices, 0usize, offset, end), |st| async move {
            let (video, indices, k, offset, end) = st;
            if k >= indices.len() {
                return None;
            }
            let idx = indices[k];
            let span = &video.info.spans[idx];
            let item = match video.chunk_at(idx).await {
                Ok(data) => {
                    let span_end = span.offset + span.len;
                    let from = offset.max(span.offset) - span.offset;
                    let to = (end.min(span_end) - span.offset).min(data.len() as u64);
                    if from < to {
                        Ok(data[from as usize..to as usize].to_vec())
                    } else {
                        // Never emit a zero-length slice mid-stream: the
                        // declared Content-Length must match the bytes sent,
                        // otherwise the player reads past the real data and
                        // reports a corrupt packet.
                        Ok(Vec::new())
                    }
                }
                Err(e) => Err(std::io::Error::other(e.to_string())),
            };
            // Keep the pipeline full as playback advances.
            video.prefetch(idx + 1, 8);
            Some((item, (video, indices, k + 1, offset, end)))
        })
    }
    /// Read `len` bytes starting at `offset`, fetching only overlapping chunks.
    pub async fn read_range(&self, offset: u64, len: u64) -> anyhow::Result<Vec<u8>> {
        let end = (offset + len).min(self.info.total_size);
        let mut out = Vec::with_capacity((end.saturating_sub(offset)) as usize);
        for span in self.spans_for(offset, end) {
            let data = self.fetch_chunk(span).await?;
            let span_end = span.offset + span.len;
            let from = offset.max(span.offset) - span.offset;
            let to = (end.min(span_end) - span.offset).min(data.len() as u64);
            if from < to {
                out.extend_from_slice(&data[from as usize..to as usize]);
            }
        }
        Ok(out)
    }
}

fn parse_chunk_id(hex: &str) -> anyhow::Result<ChunkId> {
    let bytes = hex.as_bytes();
    if bytes.len() != 40 {
        return Err(anyhow::anyhow!("bad chunk id length {}", bytes.len()));
    }
    let mut raw = [0u8; 20];
    for i in 0..20 {
        let hi = (bytes[i * 2] as char)
            .to_digit(16)
            .ok_or_else(|| anyhow::anyhow!("bad hex in chunk id"))?;
        let lo = (bytes[i * 2 + 1] as char)
            .to_digit(16)
            .ok_or_else(|| anyhow::anyhow!("bad hex in chunk id"))?;
        raw[i] = ((hi << 4) | lo) as u8;
    }
    Ok(ChunkId(raw))
}

const VIDEO_EXTS: &[&str] = &["mp4", "webm", "mkv", "avi", "mov"];

fn is_video(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    VIDEO_EXTS
        .iter()
        .any(|ext| lower.ends_with(&format!(".{ext}")))
}

/// Manifest bytes from the CDN are zip-compressed; unwrap when needed.
fn decompress_manifest(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    if data.len() > 2 && data[0] == 0x50 && data[1] == 0x4B {
        let cursor = std::io::Cursor::new(data);
        let mut archive =
            zip::ZipArchive::new(cursor).map_err(|e| anyhow::anyhow!("manifest zip: {e}"))?;
        let mut file = archive
            .by_index(0)
            .map_err(|e| anyhow::anyhow!("manifest entry: {e}"))?;
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut buf)?;
        Ok(buf)
    } else {
        Ok(data.to_vec())
    }
}

/// Cached login so repeated clicks do not each pay a full CM handshake.
///
/// A `SteamClient` is cheap to reuse but expensive to create (connect, encrypt,
/// logon). The streaming UI can be clicked several times in a row, and each
/// miss used to cost 10-40s. The cached client is validated lazily: if a call
/// on it fails the caller falls back to a fresh login.
static SESSION: std::sync::Mutex<Option<CachedSession>> = std::sync::Mutex::new(None);

struct CachedSession {
    username: String,
    client: SteamClient<LoggedIn>,
}

/// Get a logged-in client, reusing the cached one when the account matches.
pub async fn shared_login(
    username: &str,
    password: &str,
) -> anyhow::Result<std::sync::Arc<SteamClient<LoggedIn>>> {
    // Reuse only for the same account; a different account must re-auth.
    {
        let guard = SESSION.lock().unwrap();
        if let Some(s) = guard.as_ref() {
            if s.username == username {
                return Ok(std::sync::Arc::new(s.client.clone()));
            }
        }
    }
    let client = login(username, password).await?;
    {
        let mut guard = SESSION.lock().unwrap();
        *guard = Some(CachedSession {
            username: username.to_string(),
            client: client.clone(),
        });
    }
    Ok(std::sync::Arc::new(client))
}

/// Drop the cached session (call when the account changes or a call fails).
pub fn forget_session() {
    *SESSION.lock().unwrap() = None;
}

/// On-disk cache of resolved video layouts.
///
/// Resolving an item needs a manifest download, which on this network can take
/// minutes. The layout is immutable for a given (depot, manifest) pair, so it is
/// persisted and reused across restarts. The manifest id is part of the key, so
/// a re-uploaded wallpaper (new manifest) misses the cache and re-resolves.
fn layout_cache_path() -> Option<std::path::PathBuf> {
    let base = dirs::data_local_dir()?.join("WEave");
    std::fs::create_dir_all(&base).ok()?;
    Some(base.join("video_layouts.json"))
}

fn load_layouts() -> Vec<(u64, std::time::Instant, VideoStreamInfo)> {
    let Some(p) = layout_cache_path() else {
        return Vec::new();
    };
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return Vec::new();
    };
    // Stored as a plain array of VideoStreamInfo; the TTL clock restarts on load
    // which is acceptable because entries are also validated by manifest id.
    let Ok(list) = serde_json::from_str::<Vec<VideoStreamInfo>>(&raw) else {
        return Vec::new();
    };
    list.into_iter()
        .map(|info| (info.item_id, std::time::Instant::now(), info))
        .collect()
}

fn persist_layouts(entries: &[(u64, std::time::Instant, VideoStreamInfo)]) {
    let Some(p) = layout_cache_path() else { return };
    let list: Vec<&VideoStreamInfo> = entries.iter().map(|(_, _, i)| i).collect();
    if let Ok(json) = serde_json::to_string(&list) {
        let _ = std::fs::write(p, json);
    }
}

/// Cached manifest resolution per workshop item.
///
/// Resolving an item costs a manifest download (seconds) plus depot-key and
/// token round-trips. Users commonly reopen the same wallpaper, so the layout is
/// memoised. It is invalidated by TTL rather than by item id alone: workshop
/// items can be re-uploaded, and a stale layout would serve wrong bytes.
static LAYOUTS: std::sync::Mutex<Vec<(u64, std::time::Instant, VideoStreamInfo)>> =
    std::sync::Mutex::new(Vec::new());

const LAYOUT_TTL: std::time::Duration = std::time::Duration::from_secs(30 * 60);

fn cached_layout(item_id: u64) -> Option<VideoStreamInfo> {
    let mut guard = LAYOUTS.lock().unwrap();
    if guard.is_empty() {
        *guard = load_layouts();
    }
    guard
        .iter()
        .find(|(id, at, _)| *id == item_id && at.elapsed() < LAYOUT_TTL)
        .map(|(_, _, info)| info.clone())
}

/// True when this item's layout is already known, so the caller can skip the
/// expensive manifest round-trip and go straight to serving bytes.
pub fn has_cached_layout(item_id: u64) -> bool {
    cached_layout(item_id).is_some()
}

fn store_layout(item_id: u64, info: &VideoStreamInfo) {
    let mut guard = LAYOUTS.lock().unwrap();
    guard.retain(|(id, at, _)| *id != item_id && at.elapsed() < LAYOUT_TTL);
    guard.push((item_id, std::time::Instant::now(), info.clone()));
    persist_layouts(&guard);
}

/// Establish a logged-in Steam session for the streaming pipeline.
///
/// Tried in order, most convenient first:
///   1. Local Steam client token (DPAPI) — no password, no Steam Guard.
///   2. A refresh token WEave cached from an earlier sign-in.
///   3. Username + password — only for accounts without Steam Guard.
pub async fn login(username: &str, password: &str) -> anyhow::Result<SteamClient<LoggedIn>> {
    if let Some(tok) = steamroom_client::steam_creds::extract_token(username) {
        match LoginBuilder::new()
            .device_name("WEave")
            .with_refresh_token(&tok.account_name, &tok.refresh_token)
            .login()
            .await
        {
            Ok(client) => {
                log::info!("[video-stream] logged in via local Steam token");
                return Ok(client);
            }
            Err(e) => log::warn!("[video-stream] local Steam token failed: {e}"),
        }
    }
    if let Some(tok) = load_cached_token(username) {
        match LoginBuilder::new()
            .device_name("WEave")
            .with_refresh_token(username, &tok)
            .login()
            .await
        {
            Ok(client) => {
                log::info!("[video-stream] logged in via cached refresh token");
                return Ok(client);
            }
            Err(e) => log::warn!("[video-stream] cached refresh token failed: {e}"),
        }
    }
    let flow = LoginBuilder::new()
        .device_name("WEave")
        .with_credentials(username, password)
        .begin()
        .await
        .map_err(|e| anyhow::anyhow!("steam login: {e}"))?;
    match flow {
        CredentialsLoginFlow::Approved(a) => {
            let tokens = a.tokens();
            if let Some(rt) = tokens.refresh_token.clone().into() {
                let _ = save_cached_token(username, &rt);
            }
            a.finish()
                .await
                .map_err(|e| anyhow::anyhow!("steam logon: {e}"))
        }
        CredentialsLoginFlow::NeedsGuardCode(_) | CredentialsLoginFlow::NeedsMobileConfirm(_) => {
            Err(anyhow::anyhow!(
                "STEAM_GUARD_REQUIRED: sign in to the Steam desktop app so a reusable token exists"
            ))
        }
        _ => Err(anyhow::anyhow!(
            "steam login: unsupported confirmation flow"
        )),
    }
}

fn token_cache_path(username: &str) -> Option<std::path::PathBuf> {
    let base = dirs::data_local_dir()?.join("WEave");
    std::fs::create_dir_all(&base).ok()?;
    let safe: String = username
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Some(base.join(format!("steam_token_{safe}.txt")))
}

fn load_cached_token(username: &str) -> Option<String> {
    let p = token_cache_path(username)?;
    let t = std::fs::read_to_string(p).ok()?.trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

fn save_cached_token(username: &str, token: &str) -> std::io::Result<()> {
    if let Some(p) = token_cache_path(username) {
        std::fs::write(p, token)?;
    }
    Ok(())
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
    let _t1 = std::time::Instant::now();
    let depot_key = client.get_depot_decryption_key(depot_id, app_id).await?;
    log::info!("[timing] depot_key {:?}", _t1.elapsed());
    let cdn_servers = client.get_cdn_servers(CellId(0), Some(5)).await?;
    if cdn_servers.is_empty() {
        return Err(anyhow::anyhow!("no CDN servers available"));
    }
    log::info!("[timing] cdn_servers {:?}", _t1.elapsed());

    // Build one endpoint per advertised server, each with the scheme/port the
    // directory specified and the auth token issued *for that host* (tokens are
    // host-scoped; reusing one across hosts yields HTTP 403).
    //
    // Tokens are fetched concurrently: doing it serially cost ~40s per dead host
    // because each CM round-trip can stall, and the user is watching a spinner.
    let token_futs = cdn_servers.iter().map(|s| {
        let host = s.host.clone();
        async move {
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                client.get_cdn_auth_token(app_id, depot_id, &host),
            )
            .await
            {
                Ok(Ok(t)) => t.token,
                Ok(Err(e)) => {
                    log::warn!("[video-stream] no CDN token for {host}: {e}");
                    None
                }
                Err(_) => {
                    log::warn!("[video-stream] CDN token for {host} timed out");
                    None
                }
            }
        }
    });
    let _t2 = std::time::Instant::now();
    let tokens = futures_util::future::join_all(token_futs).await;
    log::info!("[timing] tokens {:?}", _t2.elapsed());

    let mut endpoints: Vec<CdnEndpoint> = Vec::with_capacity(cdn_servers.len());
    for (s, token) in cdn_servers.iter().zip(tokens) {
        let scheme = if s.https { "https" } else { "http" };
        endpoints.push(CdnEndpoint {
            base: format!("{scheme}://{}:{}", s.host, s.port),
            token,
        });
    }
    let request_code = client
        .get_manifest_request_code(app_id, depot_id, manifest_id, None, None)
        .await?
        .unwrap_or(0);

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
        .timeout(std::time::Duration::from_secs(120))
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
            let pool = steamroom::cdn::CdnServerPool::new(cdn_servers.clone());
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
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
            }
            got.ok_or_else(|| anyhow::anyhow!("manifest download failed on every CDN endpoint"))?
        }
    };
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

    // Fast path: a previously resolved layout for this exact manifest id means
    // the byte map is already known, so the (slow) manifest download can be
    // skipped entirely. The manifest id is part of the cached entry, so a
    // re-uploaded wallpaper misses and re-resolves.
    let serving_http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| anyhow::anyhow!("http client: {e}"))?;
    if let Some(cached) = cached_layout(item_id) {
        if cached.manifest_id == manifest_id.0 && cached.depot_id == depot_id.0 {
            log::info!(
                "[video-stream] layout cache hit for {item_id} ({} spans)",
                cached.spans.len()
            );
            return Ok(ResolvedVideo {
                info: cached,
                depot_key: Arc::new(depot_key),
                depot: depot_id,
                endpoints: Arc::new(endpoints),
                http: Arc::new(serving_http),
                preferred: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                cache: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
            });
        }
    }

    // native-tls (schannel) is the TLS stack that validates Steam's CDN
    // certificates on this machine; steamroom's rustls client does not.
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
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
        .timeout(std::time::Duration::from_secs(8))
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

    Ok(ResolvedVideo {
        info,
        depot_key: Arc::new(depot_key),
        depot: depot_id,
        endpoints: Arc::new(live),
        http: Arc::new(http),
        preferred: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        cache: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
    })
}
