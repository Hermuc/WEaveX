//! The runtime half of a resolved video: chunk fetching, the prefetch
//! cache, cancellation and range streaming.
//!
//! Split out of `video_stream.rs`; `impl ResolvedVideo` only needs the
//! fields on the struct plus the helpers in this module.

use std::sync::Arc;

use super::manifest::parse_chunk_id;
use super::{ChunkSpan, ResolvedVideo, CACHE_SPIN_WAIT};


impl ResolvedVideo {
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

    /// Bytes of decoded chunks to keep. A 2GB wallpaper must not be pinned in
    /// RAM, so the cache evicts everything once it exceeds this budget. Playback
    /// re-fetches in the rare case it seeks back past the window, which is much
    /// cheaper than an out-of-memory kill.
    const CACHE_LIMIT: u64 = 128 * 1024 * 1024;

    /// Insert into the cache, evicting the oldest chunks when over budget.
    ///
    /// The previous version wiped the entire map on overflow. During playback
    /// that is catastrophic: the reader keeps needing the chunks it just read
    /// (and the ones right ahead), so a full clear forced a re-download of the
    /// whole window and the video stalled. Evicting the *farthest-behind*
    /// chunks instead keeps the playhead region intact.
    fn remember(&self, idx: usize, data: Arc<Vec<u8>>) {
        let size = data.len() as u64;
        let mut map = self.cache.lock();
        if map.contains_key(&idx) {
            return;
        }
        let mut current = self.cached_bytes.load(std::sync::atomic::Ordering::Relaxed);
        if current + size > Self::CACHE_LIMIT {
            // Drop chunks furthest from the current read position first. Chunks
            // ahead of the playhead are the ones about to be consumed, so the
            // distance is measured in one direction only.
            let mut keys: Vec<usize> = map.keys().copied().collect();
            keys.sort_by_key(|k| std::cmp::Reverse(k.abs_diff(idx)));
            for key in keys {
                if current + size <= Self::CACHE_LIMIT {
                    break;
                }
                if let Some(dropped) = map.remove(&key) {
                    current = current.saturating_sub(dropped.len() as u64);
                }
            }
        }
        map.insert(idx, data);
        self.cached_bytes
            .store(current + size, std::sync::atomic::Ordering::Relaxed);
    }

    /// Stop any background prefetch (called when the stream is torn down).
    pub fn cancel(&self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Fetch one chunk by index, using the cache and falling back to the network.
    ///
    /// In-flight requests for the same chunk are coalesced: the range server's
    /// reader and the background prefetcher routinely want the same chunk at the
    /// same moment, and without coalescing both would download it (Steam rate-
    /// limits us hard, so duplicated fetches directly cost playback throughput).
    async fn chunk_at(self: &Arc<Self>, idx: usize) -> anyhow::Result<Arc<Vec<u8>>> {
        loop {
            if let Some(hit) = self.cache.lock().get(&idx).cloned() {
                return Ok(hit);
            }
            // Are we the one who gets to fetch it?
            // The guard is scoped so it never lives across the await below —
            // parking_lot guards are not Send and this future is spawned.
            let claimed = {
                let mut inflight = self.inflight.lock();
                if inflight.contains(&idx) {
                    false
                } else {
                    inflight.insert(idx);
                    true
                }
            };
            if claimed {
                break;
            }
            // Someone else is already fetching it; wait a beat and re-check the
            // cache. Bounded so a stuck fetcher cannot hang the reader forever.
            tokio::time::sleep(CACHE_SPIN_WAIT).await;
        }

        let span = match self.info.spans.get(idx).cloned() {
            Some(s) => s,
            None => {
                self.inflight.lock().remove(&idx);
                return Err(anyhow::anyhow!("chunk index {idx} out of range"));
            }
        };

        // Retry with a small backoff: this network drops connections mid-body
        // often enough that a single failure would otherwise abort the stream and
        // make the player report a corrupt packet.
        let mut last_err = None;
        let result = {
            let mut out = None;
            for attempt in 0..3u32 {
                match self.fetch_chunk(&span).await {
                    Ok(bytes) => {
                        out = Some(Arc::new(bytes));
                        break;
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
            out
        };

        // Always release the in-flight marker, success or failure.
        self.inflight.lock().remove(&idx);

        match result {
            Some(data) => {
                self.remember(idx, data.clone());
                Ok(data)
            }
            None => Err(last_err.unwrap_or_else(|| anyhow::anyhow!("chunk {idx} failed"))),
        }
    }

    /// Warm `count` chunks starting at `from` concurrently in the background.
    ///
    /// Steam rate-limits a single connection hard enough that sequential
    /// fetching is unusable for playback (measured 1s then 44s per chunk), so
    /// several chunks are fetched at once and memoised for the reader.
    ///
    /// At most one prefetch task runs at a time: players issue many overlapping
    /// Range requests, and spawning a task per request piled up dozens of
    /// duplicate fetches for the same chunks.
    fn prefetch(self: &Arc<Self>, from: usize, count: usize) {
        // More parallelism than before: Steam throttles each individual
        // connection, and the old value of 6 could not keep a 4K stream fed.
        // Each chunk is ~1 MiB compressed, so 12 in flight ≈ 12 MiB of runway.
        const CONCURRENCY: usize = 12;
        // Always keep a healthy runway ahead of the reader. The caller's
        // `count` describes the window it is about to serve; adding the runway
        // here means playback does not stall the moment it reaches the end of
        // that window — which is exactly what made the video unwatchable.
        const RUNWAY: usize = 48;
        let total = self.info.spans.len();
        let end = (from + count.max(RUNWAY)).min(total);
        if from >= end || self.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        // Single-flight, but *forward-tracking*: if a prefetcher is already
        // running, just record the furthest position anyone has asked for and
        // let the running task keep going until it catches up. The previous
        // version dropped the request entirely, so the read window moved past
        // what was actually cached and playback had to fetch synchronously.
        let prev_target = self
            .prefetch_target
            .fetch_max(end, std::sync::atomic::Ordering::AcqRel);
        if prev_target >= end {
            return;
        }
        if self
            .prefetching
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            let mut next = from;
            while !this.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                // Extend as readers advance; stop only once we have covered
                // everything requested so far.
                let target = this
                    .prefetch_target
                    .load(std::sync::atomic::Ordering::Acquire)
                    .min(this.info.spans.len());
                if next >= target {
                    break;
                }
                let end = target;
                let mut batch = Vec::new();
                while batch.len() < CONCURRENCY && next < end {
                    if !this.cache.lock().contains_key(&next) {
                        batch.push(next);
                    }
                    next += 1;
                }
                if batch.is_empty() {
                    continue;
                }
                let futs = batch.into_iter().map(|i| {
                    let v = this.clone();
                    async move {
                        let _ = v.chunk_at(i).await;
                    }
                });
                futures_util::future::join_all(futs).await;
            }
            this.prefetching
                .store(false, std::sync::atomic::Ordering::Release);
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
        // Warm the whole requested window plus a runway before the first byte is
        // served. Prefetching only a couple of chunks at a time left the reader
        // repeatedly waiting on a slow CDN mid-playback, which is what made the
        // video stutter.
        if let Some(&first) = indices.first() {
            self.prefetch(first, indices.len());
        }
        // The reader below still awaits chunks in order (HTTP bodies must stay
        // ordered), but they are almost always already cached by the prefetcher,
        // so each `chunk_at` is a hash lookup rather than a network round-trip.
        futures_util::stream::unfold((self, indices, 0usize, offset, end), |st| async move {
            let (video, indices, k, offset, end) = st;
            if k >= indices.len() {
                return None;
            }
            let idx = indices[k];
            let span = video.info.spans[idx].clone();
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
            // Keep extending the runway as playback advances.
            video.prefetch(idx + 1, indices.len().saturating_sub(k));
            Some((item, (video, indices, k + 1, offset, end)))
        })
    }
}
