//! Steam SSR (server-side-rendered) JSON payload parser.
//!
//! Steam ships the browse listing as an escaped JSON blob in
//! `window.SSR.renderContext`. Split out of `browse.rs`; `parse_browse_ssr`
//! and `unescape_json_string` are pure functions over the raw HTML/JSON string.

use serde_json::Value;

use super::selectors;
use crate::workshop::WorkshopItem;
/// Parse the SSR JSON payload Steam ships in a `<script>` tag. This
/// mirrors `parseSSRData()` from the original Python.
pub(super) fn parse_browse_ssr(html: &str) -> Vec<WorkshopItem> {
    if !html.contains("window.SSR.renderContext") {
        return Vec::new();
    }
    // Pull out the queries blob: `queries":"…escaped JSON…"`
    let re = &selectors::SSR_QUERIES;
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

pub(super) fn unescape_json_string(s: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescapes_common_json_escapes() {
        assert_eq!(unescape_json_string(r#"a\tb"#), "a\tb");
        assert_eq!(unescape_json_string(r#"line1\nline2"#), "line1\nline2");
        assert_eq!(unescape_json_string(r#"tab\there"#), "tab\there");
        assert_eq!(unescape_json_string(r#"back\\slash"#), "back\\slash");
        assert_eq!(unescape_json_string(r#"solidus\/"#), "solidus/");
    }

    #[test]
    fn unescapes_a_unicode_escape() {
        assert_eq!(unescape_json_string(r#"\u0041"#), "A");
    }

    #[test]
    fn keeps_an_unknown_escape_literally() {
        assert_eq!(unescape_json_string(r#"\q"#), "\\q");
    }

    #[test]
    fn passes_plain_text_through() {
        assert_eq!(unescape_json_string("no escapes here"), "no escapes here");
    }

    #[test]
    fn returns_nothing_without_the_ssr_marker() {
        assert!(parse_browse_ssr("<html><body>nothing</body></html>").is_empty());
    }
}
