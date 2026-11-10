//! Browse-page listing parser.
//!
//! Steam's redesigned Workshop browse page no longer exposes the legacy
//! `.workshopItem` markup, so we combine three strategies:
//!   1. SSR JSON payload (`window.SSR.renderContext`) — primary source
//!      for pubfileid / title / preview.
//!   2. Per-tile container parsing — recovers author info SSR lacks.
//!   3. Loose `a[href*="filedetails"]` scan — last-ditch safety net.
//!
//! Only pubfileid, an image URL and an author link are stable markers.

use regex::Regex;
use scraper::{Html, Selector};
use serde_json::Value;
use std::collections::HashMap;

use crate::workshop::{WorkshopItem, WorkshopPage};

use super::common::*;

pub fn parse_browse(html: &str, current_page: u32) -> WorkshopPage {
    let doc = Html::parse_document(html);

    // Fast path: try SSR data first — Steam embeds the items as a JSON
    // payload in a `<script>` tag. This is what the Python WebView code
    // does in `parseSSRData()`, and it gives us pubfileid/title/preview
    // straight away (without ever depending on obfuscated CSS classes).
    let mut items = parse_browse_ssr(html);

    // The SSR payload doesn't include `author` / `author_url`, so always
    // fall back to the per-tile container parser (`.tmIrUKf-Mh8-, …`).
    // It's also our only recovery if SSR data is missing.
    let mut html_items = parse_browse_html(&doc);
    if html_items.is_empty() && items.is_empty() {
        // Last-ditch fallback: scan loose `a[href*="filedetails"]` links.
        // This is what we used to do unconditionally — keep it as a safety
        // net for unexpected markup changes / synthetic test fixtures.
        html_items = parse_browse_loose(&doc, html);
    }
    if items.is_empty() {
        items = html_items;
    } else {
        // Merge author info from html parsing into SSR items.
        let by_id: HashMap<String, &WorkshopItem> = html_items
            .iter()
            .map(|i| (i.pubfileid.clone(), i))
            .collect();
        for it in &mut items {
            if it.author.is_empty() {
                if let Some(src) = by_id.get(&it.pubfileid) {
                    it.author = src.author.clone();
                    it.author_url = src.author_url.clone();
                    if it.preview_url.is_empty() {
                        it.preview_url = src.preview_url.clone();
                    }
                }
            }
        }
    }

    for it in &mut items {
        if it.preview_url.is_empty() {
            it.is_collection = true;
        }
        if it.title.is_empty() {
            it.title = format!("Wallpaper {}", it.pubfileid);
        }
    }

    let (total_pages, total_items) = parse_pagination(&doc, html, &items);
    WorkshopPage {
        items,
        current_page,
        total_pages: total_pages.max(1),
        total_items,
    }
}

/// Parse the per-tile containers used by Steam's redesigned Workshop
/// browse page. Mirrors `_build_html_item_parser()` from the original
/// `workshop_scripts.py`:
///
///   `doc.querySelectorAll('.tmIrUKf-Mh8-, .sDK5fonBQMA-, .workshopItem, .workshopItemCollection')`
fn parse_browse_html(doc: &Html) -> Vec<WorkshopItem> {
    let container_sel = Selector::parse(
        ".tmIrUKf-Mh8-, .sDK5fonBQMA-, .workshopItem, .workshopItemCollection, \
         a.workshopItemCollection",
    )
    .unwrap();
    let link_sel = Selector::parse(r#"a[href*="filedetails"]"#).unwrap();
    let title_sel = Selector::parse("._3rvey4VpXts- a, .workshopItemTitle").unwrap();
    let img_sel = Selector::parse("img").unwrap();
    // Author chip selectors. On the Collections browse tiles Steam ships
    // just `<span class="workshopItemAuthorName">NAME</span>` (preceded by
    // a loose "Collection by" text node — no link, no URL). On regular
    // Workshop tiles it's `<div class="workshopItemAuthorName">by&nbsp;<a
    // class="workshop_author_link" href="…">NAME</a></div>`. We try the
    // linked form first so `author_url` is populated when available, then
    // fall back to plain text.
    let author_link_sel = Selector::parse(
        ".CmHGWYJjMk0- a, .workshopItemAuthorName a, \
         a.workshop_author_link, \
         a[href*=\"/myworkshopfiles/\"], a[href*=\"/myworkshopcollections/\"]",
    )
    .unwrap();
    let author_text_sel = Selector::parse(".workshopItemAuthorName").unwrap();
    let author_re = Regex::new(r"(?i)^\s*(?:by|collection\s+by)\s+").unwrap();

    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for tile in doc.select(&container_sel) {
        // The tile itself may be the link (collection tiles use
        // `<a class="workshopItemCollection" href="…filedetails?id=…">`).
        let tile_name = tile.value().name();
        let tile_href = if tile_name == "a" {
            tile.value().attr("href")
        } else {
            None
        };
        let href = tile_href.or_else(|| {
            tile.select(&link_sel)
                .next()
                .and_then(|l| l.value().attr("href"))
        });
        let Some(href) = href else { continue };
        let Some(id) = extract_pubfileid(href) else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }

        let mut item = WorkshopItem {
            pubfileid: id,
            ..Default::default()
        };

        // Title — explicit element first, then fall back to the link text /
        // image alt attribute (used when the tile is a collection card).
        if let Some(t) = tile.select(&title_sel).next() {
            item.title = inner_text(t);
        }
        if item.title.is_empty() {
            if let Some(link) = tile.select(&link_sel).next() {
                let lt = inner_text(link);
                if !lt.starts_with("By ") {
                    item.title = lt;
                }
            }
        }
        if item.title.is_empty() {
            if let Some(img) = tile.select(&img_sel).next() {
                if let Some(alt) = img.value().attr("alt") {
                    item.title = alt.trim().to_string();
                }
            }
        }

        // Preview image
        if let Some(img) = tile.select(&img_sel).next() {
            if let Some(src) = img
                .value()
                .attr("src")
                .or_else(|| img.value().attr("data-src"))
            {
                item.preview_url = src.to_string();
            }
        }

        // Author — prefer the anchor form (gets us URL for click-through),
        // fall back to the plain `.workshopItemAuthorName` span for
        // collection tiles where Steam doesn't render a link at all.
        if let Some(a) = tile.select(&author_link_sel).next() {
            let raw = inner_text(a);
            item.author = author_re.replace(&raw, "").trim().to_string();
            item.author_url = a.value().attr("href").unwrap_or("").to_string();
        } else if let Some(span) = tile.select(&author_text_sel).next() {
            let raw = inner_text(span);
            item.author = author_re.replace(&raw, "").trim().to_string();
        }

        items.push(item);
    }

    items
}

/// Loose, marker-based fallback when neither SSR nor container parsing
/// matches anything. Walks every `a[href*="filedetails"]` link, merges
/// duplicates by pubfileid, and pairs nearby `By NAME` author links.
fn parse_browse_loose(doc: &Html, html: &str) -> Vec<WorkshopItem> {
    let link_sel = Selector::parse(r#"a[href*="filedetails/?id="]"#).unwrap();
    let img_sel = Selector::parse("img").unwrap();
    let mut by_id: HashMap<String, WorkshopItem> = HashMap::new();

    for a in doc.select(&link_sel) {
        let Some(href) = a.value().attr("href") else {
            continue;
        };
        let Some(id) = extract_pubfileid(href) else {
            continue;
        };
        let entry = by_id.entry(id.clone()).or_insert_with(|| WorkshopItem {
            pubfileid: id.clone(),
            ..Default::default()
        });
        let txt = inner_text(a);
        if !txt.is_empty() && entry.title.is_empty() && !txt.starts_with("By ") {
            entry.title = txt;
        }
        if entry.preview_url.is_empty() {
            if let Some(img) = a.select(&img_sel).next() {
                if let Some(src) = img
                    .value()
                    .attr("src")
                    .or_else(|| img.value().attr("data-src"))
                {
                    entry.preview_url = src.to_string();
                }
                if entry.title.is_empty() {
                    if let Some(alt) = img.value().attr("alt") {
                        if !alt.is_empty() {
                            entry.title = alt.trim().to_string();
                        }
                    }
                }
            }
        }
    }

    // Pair `By NAME` links with the nearest preceding filedetails link.
    // Collection tiles link to `myworkshopcollections` instead of
    // `myworkshopfiles` so both have to be matched here.
    let author_re = Regex::new(
        r#"<a href="(https://steamcommunity\.com/(?:id|profiles)/[^"]+/(?:myworkshopfiles|myworkshopcollections)/[^"]*)"[^>]*>By\s+([^<]+)</a>"#,
    )
    .unwrap();
    let id_re = Regex::new(r#"filedetails/\?id=(\d+)"#).unwrap();
    let id_positions: Vec<(String, usize)> = id_re
        .captures_iter(html)
        .map(|c| (c[1].to_string(), c.get(0).unwrap().start()))
        .collect();
    for cap in author_re.captures_iter(html) {
        let start = cap.get(0).unwrap().start();
        let url = cap[1].to_string();
        let name = cap[2].trim().to_string();
        if let Some(id) = id_positions
            .iter()
            .rev()
            .find(|(_, p)| *p < start)
            .map(|(id, _)| id.clone())
        {
            if let Some(entry) = by_id.get_mut(&id) {
                if entry.author.is_empty() {
                    entry.author = name;
                    entry.author_url = url;
                }
            }
        }
    }

    let mut ordered: Vec<(usize, WorkshopItem)> = by_id
        .into_iter()
        .filter_map(|(id, item)| {
            id_positions
                .iter()
                .find(|(p_id, _)| p_id == &id)
                .map(|(_, p)| (*p, item))
        })
        .collect();
    ordered.sort_by_key(|(p, _)| *p);
    ordered.into_iter().map(|(_, it)| it).collect()
}

/// Parse the SSR JSON payload Steam ships in a `<script>` tag. This
/// mirrors `parseSSRData()` from the original Python.
fn parse_browse_ssr(html: &str) -> Vec<WorkshopItem> {
    if !html.contains("window.SSR.renderContext") {
        return Vec::new();
    }
    // Pull out the queries blob: `queries":"…escaped JSON…"`
    let re = Regex::new(r#"queries"\s*:\s*"((?:\\.|[^"\\])*)""#).unwrap();
    let Some(cap) = re.captures(html) else {
        return Vec::new();
    };
    let raw = cap[1].replace(r"\\", r"\");
    let unescaped = unescape_json_string(&raw);
    let Ok(value) = serde_json::from_str::<Value>(&unescaped) else {
        return Vec::new();
    };
    let Some(arr) = value.as_array() else {
        return Vec::new();
    };

    // Each Steam SSR query carries one or more queryKey strings —
    // the "items" payload we want lives under the workshop browse key
    // OR the user-files key (author wallpapers tab) OR the collection
    // browse key (Collections tab). Just grab the *first* query that
    // exposes `state.data.items` so the SSR path works in every view
    // without us hard-coding query names.
    let mut found = None;
    for q in arr {
        if let Some(items) = q.pointer("/state/data/items") {
            if items.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
                found = Some(items.clone());
                break;
            }
        }
    }
    let Some(items_val) = found else {
        return Vec::new();
    };
    let Some(items_arr) = items_val.as_array() else {
        return Vec::new();
    };
    items_arr
        .iter()
        .filter_map(|i| {
            let pubfileid = i
                .get("publishedfileid")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    i.get("publishedfileid")
                        .and_then(|v| v.as_u64())
                        .map(|n| n.to_string())
                })?;
            let title = i
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let preview_url = i
                .get("preview_url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Steam ships the author inside the SSR items in a couple of
            // shapes:
            //   - top-level fields: `creator_persona_name` / `creator`
            //   - a nested `creator_info` / `creator_data` object with
            //     `persona_name` and `profile_url`.
            // We sniff all of them so author chips are populated on the
            // first paint, before the in-card HTML fallback fires.
            let mut author = i
                .get("creator_persona_name")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_default();
            let mut author_url = String::new();
            for key in ["creator_info", "creator_data", "creator"] {
                if author.is_empty() {
                    if let Some(name) = i
                        .pointer(&format!("/{}/persona_name", key))
                        .and_then(|v| v.as_str())
                    {
                        author = name.to_string();
                    }
                }
                if author_url.is_empty() {
                    if let Some(url) = i
                        .pointer(&format!("/{}/profile_url", key))
                        .and_then(|v| v.as_str())
                    {
                        author_url = url.to_string();
                    }
                }
            }
            // Sometimes the creator's SteamID64 is inlined and the
            // profile URL has to be reconstructed.
            if author_url.is_empty() {
                if let Some(steamid) = i
                    .get("creator")
                    .and_then(|v| v.as_str())
                    .or_else(|| i.get("creator_steamid").and_then(|v| v.as_str()))
                {
                    author_url = format!("https://steamcommunity.com/profiles/{steamid}");
                }
            }

            Some(WorkshopItem {
                pubfileid,
                title,
                preview_url,
                author,
                author_url,
                ..Default::default()
            })
        })
        .collect()
}

fn unescape_json_string(s: &str) -> String {
    // The queries blob is double-escaped. After we unescape `\\` → `\`
    // we still need to handle simple JSON escapes.
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('/') => out.push('/'),
                Some('\\') => out.push('\\'),
                Some('u') => {
                    let hex: String = (&mut chars).take(4).collect();
                    if let Ok(n) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(n) {
                            out.push(ch);
                        }
                    }
                }
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => break,
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_pagination(doc: &Html, html: &str, items: &[WorkshopItem]) -> (u32, u64) {
    // Приоритет 1: ищем паттерн с многоточием и последним числом в пагинации
    // Паттерн: <div>...</div><div class="...Panel" tabindex="0" role="button">1,000</div>
    // ВАЖНО: ищем ПОСЛЕДНЕЕ совпадение, так как троеточий может быть несколько
    let re_ellipsis_last = Regex::new(
        r#"<div>\.\.\.</div><div[^>]*class="[^"]*Panel[^"]*"[^>]*tabindex="0"[^>]*role="button"[^>]*>\s*([\d,]+)\s*</div>"#
    ).unwrap();

    // Находим ВСЕ совпадения и берём последнее
    let last_match = re_ellipsis_last.captures_iter(html).last();
    if let Some(cap) = last_match {
        if let Ok(max) = cap[1].replace(',', "").parse::<u32>() {
            // Пытаемся определить общее количество элементов из текста
            let re_total = Regex::new(
                r#"(?:Showing|Отображаются?)\s+\d+[\s\-–]\d+\s+(?:of|из)\s+([\d,.\s]+)"#,
            )
            .unwrap();
            let total_items = re_total
                .captures(html)
                .and_then(|cap| cap[1].replace([',', '.', ' '], "").parse::<u64>().ok())
                .unwrap_or_else(|| {
                    // Если не удалось извлечь, оцениваем на основе количества элементов на странице
                    let per_page = items.len().max(30) as u64;
                    (max as u64) * per_page
                });
            return (max, total_items);
        }
    }

    // Приоритет 2: извлечение из текста "Showing X-Y of Z"
    let re_total =
        Regex::new(r#"(?:Showing|Отображаются?)\s+(\d+)[\s\-–](\d+)\s+(?:of|из)\s+([\d,.\s]+)"#)
            .unwrap();
    if let Some(cap) = re_total.captures(html) {
        let start: u64 = cap[1].replace([',', '.', ' '], "").parse().unwrap_or(0);
        let end: u64 = cap[2].replace([',', '.', ' '], "").parse().unwrap_or(0);
        let total: u64 = cap[3].replace([',', '.', ' '], "").parse().unwrap_or(0);
        if total > 0 {
            let per_page = (end - start + 1).max(1);
            return (total.div_ceil(per_page) as u32, total);
        }
    }

    // Приоритет 3: поиск по селекторам в controls пагинации (старый метод)
    if let Ok(paging_sel) =
        Selector::parse(".workshopBrowsePagingControls a, .workshopBrowsePagingControls span")
    {
        let mut max_page = 0;
        for el in doc.select(&paging_sel) {
            let text = el.text().collect::<String>().trim().to_string();
            if let Ok(p) = text.parse::<u32>() {
                if p > max_page {
                    max_page = p;
                }
            }
        }
        if max_page > 0 {
            let per_page = items.len().max(30) as u64;
            return (max_page, (max_page as u64) * per_page);
        }
    }

    // Приоритет 4: поиск по ссылкам пагинации с помощью регулярного выражения (старый метод, fallback)
    // Улучшенный регулярный выражение, поддерживающий любой порядок атрибутов
    let re_pages_1 = Regex::new(r#"class="pagelink"[^>]*>\s*(\d+)\s*<"#).unwrap();
    let re_pages_2 = Regex::new(r#"class="page_current"[^>]*>\s*(\d+)\s*<"#).unwrap();

    let max_page_1 = re_pages_1
        .captures_iter(html)
        .filter_map(|c| c[1].parse::<u32>().ok())
        .max();
    let max_page_2 = re_pages_2
        .captures_iter(html)
        .filter_map(|c| c[1].parse::<u32>().ok())
        .max();

    let max_page = max_page_1.max(max_page_2);
    if let Some(max) = max_page {
        let per_page = items.len().max(30) as u64;
        return (max, (max as u64) * per_page);
    }

    // Fallback: assume more pages exist if we filled a full page of 30
    if items.len() >= 30 {
        return (99, (items.len() as u64) * 99);
    }
    (1, items.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny synthetic fixture that mirrors the stable markup Steam still
    /// emits on the Workshop browse page (filedetails links with images +
    /// a "By Author" link per tile). Verifies that the regex-based parser
    /// can recover (pubfileid, title, preview_url, author) pairs even
    /// when everything around them uses obfuscated CSS class names.
    #[test]
    fn parses_minimal_browse_fixture() {
        let html = r##"
            <html><body>
            <div class="rKsVn">
                <a href="https://steamcommunity.com/sharedfiles/filedetails/?id=111">
                    <img src="https://example.com/a.jpg?imw=288" alt="Title A">
                </a>
                <a href="https://steamcommunity.com/id/user1/myworkshopfiles/?appid=431960">By Author One</a>
            </div>
            <div class="rKsVn">
                <a href="https://steamcommunity.com/sharedfiles/filedetails/?id=222">
                    <img src="https://example.com/b.jpg?imw=288" alt="Title B">
                </a>
                <a href="https://steamcommunity.com/id/user2/myworkshopfiles/?appid=431960">By Author Two</a>
            </div>
            </body></html>
        "##;
        let page = parse_browse(html, 1);
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0].pubfileid, "111");
        assert_eq!(page.items[0].title, "Title A");
        assert!(page.items[0].preview_url.contains("imw=288"));
        assert_eq!(page.items[0].author, "Author One");
        assert_eq!(page.items[1].pubfileid, "222");
        assert_eq!(page.items[1].author, "Author Two");
    }

    #[test]
    fn test_parse_pagination_old_style() {
        let html = r##"
            <div class="workshopBrowsePaging">
                <div>
                    Показывать на странице: 9&nbsp;<a href="?appid=431960&amp;p=1&amp;numperpage=18">18</a>&nbsp;<a href="?appid=431960&amp;p=1&amp;numperpage=30">30</a>&nbsp;
                </div>
                <div class="workshopBrowsePagingControls">
                     <span class="pagebtn disabled">&lt;</span><span class="page_current">1</span><a class="pagelink" href="?appid=431960&amp;p=2">2</a><a class="pagelink" href="?appid=431960&amp;p=3">3</a>&nbsp;...&nbsp;<a class="pagelink" href="?appid=431960&amp;p=24">24</a><a class="pagebtn" href="?appid=431960&amp;p=2">&gt;</a>
                </div>
             </div>
        "##;
        let doc = Html::parse_document(html);
        let items = vec![];
        let (pages, _items_count) = parse_pagination(&doc, html, &items);
        assert_eq!(pages, 24);
    }
}
