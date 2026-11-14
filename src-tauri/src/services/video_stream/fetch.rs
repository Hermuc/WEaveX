//! CDN token fetching, endpoint building and manifest download for video
//! streaming.
//!
//! Split out of `video_stream.rs`; these are the network-facing helpers
//! `resolve_video` calls.

use std::sync::Arc;

use steamroom::client::{LoggedIn, SteamClient};
use steamroom::depot::{AppId, DepotId, DepotKey, ManifestId};

use super::{
    CdnEndpoint, ResolvedVideo, VideoStreamInfo, MANIFEST_DOWNLOAD_TIMEOUT,
    MANIFEST_RETRY_BACKOFF,
};


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
pub(super) async fn fetch_cdn_tokens(
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
pub(super) fn build_endpoints(
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
pub(super) fn build_resolved_video(
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
pub(super) async fn fetch_manifest(
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
