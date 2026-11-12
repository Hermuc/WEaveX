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
use steamroom::cdn::{CdnClient, CdnServerPool};
use steamroom::client::{LoggedIn, SteamClient};
use steamroom::depot::manifest::{DepotManifest, ManifestFile};
use steamroom::depot::{AppId, CellId, ChunkId, DepotId, DepotKey, ManifestId};
use steamroom::generated;
use steamroom_client::login::CredentialsLoginFlow;
use steamroom_client::login::LoginBuilder;

/// Wallpaper Engine's Steam app id.
pub const WE_APP_ID: u32 = 431960;

/// One contiguous byte range of the video, backed by one chunk.
#[derive(Debug, Clone, Serialize)]
pub struct ChunkSpan {
    pub offset: u64,
    pub len: u64,
    /// Hex chunk id (the CDN address).
    pub chunk_id: String,
    pub checksum: u32,
}

/// The byte layout of one video file.
#[derive(Debug, Clone, Serialize)]
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
}

impl ResolvedVideo {
    /// Chunk spans whose byte range intersects `[start, end)`.
    fn spans_for(&self, start: u64, end: u64) -> impl Iterator<Item = &ChunkSpan> {
        self.info.spans.iter().filter(move |s| {
            let span_end = s.offset + s.len;
            span_end > start && s.offset < end
        })
    }

    /// Fetch, decrypt and decompress one chunk, returning its full bytes.
    async fn fetch_chunk(&self, span: &ChunkSpan) -> anyhow::Result<Vec<u8>> {
        // Validate the hex id up front so a corrupt manifest fails loudly.
        let _ = parse_chunk_id(&span.chunk_id)?;

        let n = self.endpoints.len();
        if n == 0 {
            return Err(anyhow::anyhow!("no CDN endpoints"));
        }
        let start_at = self.preferred.load(std::sync::atomic::Ordering::Relaxed);
        let mut last_err = String::new();

        for k in 0..n {
            let idx = (start_at + k) % n;
            let ep = &self.endpoints[idx];
            let url = format!(
                "{}/depot/{}/chunk/{}{}",
                ep.base,
                self.depot.0,
                span.chunk_id,
                ep.token.as_deref().unwrap_or("")
            );

            match self.http.get(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if !status.is_success() {
                        last_err = format!("HTTP {status}");
                        continue;
                    }
                    let body = match resp.bytes().await {
                        Ok(b) => b,
                        Err(e) => {
                            last_err = format!("body: {e}");
                            continue;
                        }
                    };
                    let out = steamroom::depot::chunk::process_chunk(
                        &body,
                        &self.depot_key,
                        span.len as u32,
                        span.checksum,
                    )
                    .map_err(|e| anyhow::anyhow!("process chunk {}: {e}", span.chunk_id))?;
                    self.preferred
                        .store(idx, std::sync::atomic::Ordering::Relaxed);
                    return Ok(out);
                }
                Err(e) => last_err = e.to_string(),
            }
        }
        Err(anyhow::anyhow!(
            "chunk {} failed: {last_err}",
            span.chunk_id
        ))
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

    let depot_key = client.get_depot_decryption_key(depot_id, app_id).await?;
    let cdn_servers = client.get_cdn_servers(CellId(0), Some(5)).await?;
    if cdn_servers.is_empty() {
        return Err(anyhow::anyhow!("no CDN servers available"));
    }
    let cdn_pool = CdnServerPool::new(cdn_servers.clone());

    // Build one endpoint per advertised server, each with the scheme/port the
    // directory specified and the auth token issued *for that host* (tokens are
    // host-scoped; reusing one across hosts yields HTTP 403).
    let mut endpoints: Vec<CdnEndpoint> = Vec::with_capacity(cdn_servers.len());
    for s in &cdn_servers {
        let scheme = if s.https { "https" } else { "http" };
        let token = match client.get_cdn_auth_token(app_id, depot_id, &s.host).await {
            Ok(t) => t.token,
            Err(e) => {
                log::warn!("[video-stream] no CDN token for {}: {e}", s.host);
                None
            }
        };
        endpoints.push(CdnEndpoint {
            base: format!("{scheme}://{}:{}", s.host, s.port),
            token,
        });
    }
    let cdn = CdnClient::new().map_err(|e| anyhow::anyhow!("cdn client: {e}"))?;

    let request_code = client
        .get_manifest_request_code(app_id, depot_id, manifest_id, None, None)
        .await?
        .unwrap_or(0);

    // CDN connectivity is flaky; a manifest fetch can fail transiently even
    // though a retry seconds later succeeds. Retry a few times before giving up.
    let mut manifest_data = None;
    let mut last_err = String::new();
    for attempt in 0..5u32 {
        match cdn
            .download_manifest_pooled(
                &cdn_pool,
                depot_id,
                manifest_id,
                request_code,
                endpoints.first().and_then(|e| e.token.as_deref()),
            )
            .await
        {
            Ok(d) => {
                manifest_data = Some(d);
                break;
            }
            Err(e) => {
                last_err = e.to_string();
                log::warn!(
                    "[video-stream] manifest attempt {} failed: {e}",
                    attempt + 1
                );
                tokio::time::sleep(std::time::Duration::from_millis(800 * (attempt as u64 + 1)))
                    .await;
            }
        }
    }
    let manifest_data = manifest_data
        .ok_or_else(|| anyhow::anyhow!("manifest download failed after retries: {last_err}"))?;
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
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| anyhow::anyhow!("http client: {e}"))?;

    // Probe the endpoints with the smallest chunk and keep only the ones that
    // actually serve bytes. Steam hands out host-scoped tokens in different
    // shapes per host and some hosts are unreachable from a given network, so
    // assuming the first one works produces intermittent failures at playback
    // time. Probing once here makes playback deterministic.
    let probe_span = spans
        .iter()
        .min_by_key(|s| s.len)
        .ok_or_else(|| anyhow::anyhow!("video has no chunks"))?
        .clone();
    let mut live: Vec<CdnEndpoint> = Vec::new();
    for ep in &endpoints {
        let url = format!(
            "{}/depot/{}/chunk/{}{}",
            ep.base,
            depot_id.0,
            probe_span.chunk_id,
            ep.token.as_deref().unwrap_or("")
        );
        match http.get(&url).send().await {
            Ok(r) if r.status().is_success() => {
                log::info!("[video-stream] CDN endpoint OK: {}", ep.base);
                live.push(ep.clone());
            }
            Ok(r) => log::warn!(
                "[video-stream] CDN endpoint {} -> HTTP {}",
                ep.base,
                r.status()
            ),
            Err(e) => log::warn!("[video-stream] CDN endpoint {} -> {e}", ep.base),
        }
    }
    if live.is_empty() {
        return Err(anyhow::anyhow!(
            "no usable CDN endpoint (tried {})",
            endpoints.len()
        ));
    }

    Ok(ResolvedVideo {
        info: VideoStreamInfo {
            item_id,
            depot_id: depot_id.0,
            manifest_id: manifest_id.0,
            filename: video.normalized_path(),
            total_size: video.size,
            spans,
        },
        depot_key: Arc::new(depot_key),
        depot: depot_id,
        endpoints: Arc::new(live),
        http: Arc::new(http),
        preferred: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    })
}
