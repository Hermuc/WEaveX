//! Collection-contents page parser.

use regex::Regex;
use scraper::{Html, Selector};
use serde_json::Value;

use crate::workshop::{CollectionContents, CollectionRef, WorkshopItem};

use super::common::*;

pub fn parse_collection_contents(html: &str, collection_id: &str) -> CollectionContents {
    let doc = Html::parse_document(html);
    let mut c = CollectionContents {
        collection_id: collection_id.to_string(),
        ..Default::default()
    };

    if let Some(el) = select_first(&doc, ".workshopItemTitle") {
        c.title = text(el);
    }
    // The original PyQt build kept the *full* description (and only
    // truncated it for display when needed). Earlier we used inner_text
    // which strips whitespace and silently drops <br>-separated lines.
    // Switch to a newline-preserving extractor so the panel shows the
    // entire description Steam returns.
    if let Some(el) = select_first(&doc, ".workshopItemDescription") {
        c.description = inner_text_multiline(el);
    }
    // Background image first (Steam uses it as the hero on collection
    // pages); fall back to the standard og:image / preview slots.
    if let Some(bg) = select_first(&doc, ".collectionBackgroundImage") {
        if let Some(src) = bg.value().attr("src") {
            c.preview_url = src.to_string();
        }
    }
    if c.preview_url.is_empty() {
        for sel in [
            "#previewImageMain",
            ".workshopItemPreviewImage",
            ".highlight_screenshot img",
            "meta[property=\"og:image\"]",
        ] {
            if let Some(el) = select_first(&doc, sel) {
                let val = el
                    .value()
                    .attr("src")
                    .or_else(|| el.value().attr("content"))
                    .unwrap_or("");
                if !val.is_empty() {
                    c.preview_url = val.to_string();
                    break;
                }
            }
        }
    }

    // Author + author_url. Steam's collection page renders this as
    //   <div class="friendBlockContent"> Author<br>Author Name </div>
    //   with a sibling <a class="friendBlockLinkOverlay" href="…">.
    // Older variants: `.workshopItemAuthorName a`.
    if let Some(a) = select_first(&doc, ".workshopItemAuthorName a") {
        c.author = text(a);
        if let Some(href) = a.value().attr("href") {
            c.author_url = href.to_string();
        }
    } else if let Some(a) = select_first(&doc, ".friendBlockContent a") {
        c.author = text(a);
        if let Some(href) = a.value().attr("href") {
            c.author_url = href.to_string();
        }
    }
    if c.author_url.is_empty() {
        if let Some(a) = select_first(&doc, ".friendBlockLinkOverlay") {
            if let Some(href) = a.value().attr("href") {
                c.author_url = href.to_string();
            }
        }
    }

    // -------- info dictionary (rating, counts, dates, tag groups) --------
    let mut info = serde_json::Map::new();
    if let Some(img) = select_first(&doc, ".fileRatingDetails img") {
        if let Some(src) = img.value().attr("src") {
            let path = src.split('?').next().unwrap_or(src);
            let filename = path.rsplit('/').next().unwrap_or("");
            let stem = filename
                .trim_end_matches(".png")
                .trim_end_matches(".jpg")
                .trim_end_matches(".gif");
            if !stem.is_empty() {
                info.insert("rating_star_file".into(), Value::String(stem.to_string()));
            }
        }
    }
    if let Some(el) = select_first(&doc, ".numRatings") {
        let raw = text(el);
        let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            info.insert("num_ratings".into(), Value::String(digits));
        }
    }
    if let Some(el) = select_first(&doc, ".rightSectionTopTitle") {
        let raw = text(el);
        if let Some(m) = Regex::new(r"(\d+)").unwrap().captures(&raw) {
            if let Ok(n) = m[1].parse::<u64>() {
                info.insert(
                    "item_count".into(),
                    Value::Number(serde_json::Number::from(n)),
                );
            }
        }
    }
    let stat_left_sel = Selector::parse(".detailsStatLeft").unwrap();
    let stat_right_sel = Selector::parse(".detailsStatRight").unwrap();
    let lefts: Vec<_> = doc.select(&stat_left_sel).collect();
    let rights: Vec<_> = doc.select(&stat_right_sel).collect();
    for (l, r) in lefts.iter().zip(rights.iter()) {
        let left = text(*l);
        let right = text(*r);
        let lr = right.to_lowercase();
        let ll = left.to_lowercase();
        if lr.contains("unique") {
            info.insert("unique_visitors".into(), Value::String(left));
        } else if lr.contains("total") && lr.contains("favorit") {
            info.insert("total_favorited".into(), Value::String(left));
        } else if lr.contains("favorit") {
            info.insert("favorited".into(), Value::String(left));
        } else if lr.contains("subscriber") {
            info.insert("subscribers".into(), Value::String(left));
        } else if ll.contains("posted")
            || ll.contains("added")
            || ll.contains("created")
        {
            info.insert("posted_date".into(), Value::String(right));
        } else if ll.contains("updated") || ll.contains("changed")
        {
            info.insert("updated_date".into(), Value::String(right));
        }
    }
    // Tag groups (.workshopTags > .workshopTagsTitle + <a> links).
    let tag_block_sel = Selector::parse(".workshopTags").unwrap();
    let tag_title_sel = Selector::parse(".workshopTagsTitle").unwrap();
    let tag_link_sel = Selector::parse("a").unwrap();
    for td in doc.select(&tag_block_sel) {
        let Some(title_el) = td.select(&tag_title_sel).next() else {
            continue;
        };
        let mut key = text(title_el);
        key = key
            .trim_end_matches(|c: char| c == ':' || c.is_whitespace() || c == '\u{00a0}')
            .trim()
            .to_string();
        if key.is_empty() {
            continue;
        }
        let values: Vec<String> = td
            .select(&tag_link_sel)
            .map(text)
            .filter(|v| !v.is_empty())
            .collect();
        if !values.is_empty() {
            info.insert(key, Value::String(values.join(", ")));
        }
    }
    c.info = Value::Object(info);

    // -------- items in this collection (with their own author chips) ----
    let item_sel = Selector::parse(".collectionItem").unwrap();
    let link_sel = Selector::parse(r#"a[href*="filedetails"]"#).unwrap();
    let preview_sel = Selector::parse(".workshopItemPreviewImage").unwrap();
    let img_sel = Selector::parse("img").unwrap();
    let title_sel = Selector::parse(".workshopItemTitle").unwrap();
    let author_sel = Selector::parse(".workshopItemAuthorName a").unwrap();

    let mut seen = std::collections::HashSet::new();
    for e in doc.select(&item_sel) {
        let Some(link) = e.select(&link_sel).next() else {
            continue;
        };
        let Some(id) = extract_pubfileid(link.value().attr("href").unwrap_or("")) else {
            continue;
        };
        if id == collection_id || !seen.insert(id.clone()) {
            continue;
        }
        let title = e.select(&title_sel).next().map(text).unwrap_or_default();
        let preview = e
            .select(&preview_sel)
            .next()
            .and_then(|i| i.value().attr("src"))
            .or_else(|| {
                e.select(&img_sel)
                    .next()
                    .and_then(|i| i.value().attr("src"))
            })
            .unwrap_or("")
            .to_string();
        let mut author = String::new();
        let mut author_url = String::new();
        if let Some(a) = e.select(&author_sel).next() {
            author = text(a);
            if let Some(href) = a.value().attr("href") {
                author_url = href.to_string();
            }
        }
        c.items.push(WorkshopItem {
            pubfileid: id,
            title,
            preview_url: preview,
            author,
            author_url,
            ..Default::default()
        });
    }

    // -------- related collections sidebar ------------------------------
    let related_sel = Selector::parse(".collections > .workshopItem").unwrap();
    let count_sel = Selector::parse(".workshopCollectionNumChildren").unwrap();
    let digits_re = Regex::new(r"(\d+)").unwrap();
    for r in doc.select(&related_sel) {
        let Some(link) = r.select(&link_sel).next() else {
            continue;
        };
        let Some(id) = extract_pubfileid(link.value().attr("href").unwrap_or("")) else {
            continue;
        };
        if id == collection_id || !seen.insert(id.clone()) {
            continue;
        }
        let title = r.select(&title_sel).next().map(text).unwrap_or_default();
        let preview = r
            .select(&preview_sel)
            .next()
            .and_then(|i| i.value().attr("src"))
            .unwrap_or("")
            .to_string();
        let mut item_count: u32 = 0;
        if let Some(ce) = r.select(&count_sel).next() {
            let raw = text(ce);
            if let Some(m) = digits_re.captures(&raw) {
                if let Ok(n) = m[1].parse::<u32>() {
                    item_count = n;
                }
            }
        }
        let collections = if item_count > 0 {
            vec![CollectionRef {
                id: id.clone(),
                title: String::new(),
                item_count,
            }]
        } else {
            Vec::new()
        };
        c.related_collections.push(WorkshopItem {
            pubfileid: id,
            title,
            preview_url: preview,
            is_collection: true,
            collections,
            ..Default::default()
        });
    }

    c
}
