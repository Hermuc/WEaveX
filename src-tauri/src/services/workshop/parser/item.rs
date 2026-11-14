//! Item-details page parser (`filedetails/?id=...`).

use scraper::Html;
use serde_json::{Map, Value};

use crate::workshop::{CollectionRef, WorkshopItem};

use super::common::*;
use super::selectors;

pub fn parse_item_details(html: &str, pubfileid: &str) -> WorkshopItem {
    let doc = Html::parse_document(html);
    let mut item = WorkshopItem {
        pubfileid: pubfileid.to_string(),
        ..Default::default()
    };

    if let Some(el) = select_first(&doc, ".workshopItemTitle") {
        item.title = text(el);
    }
    if item.title.is_empty() {
        if let Some(el) = select_first(&doc, "title") {
            let t = text(el);
            item.title = t.replace("Steam Workshop::", "").trim().to_string();
        }
    }
    if let Some(el) = select_first(&doc, ".workshopItemDescription") {
        let t = text(el);
        item.description = if t.len() > 1000 {
            t.char_indices()
                .nth(1000)
                .map(|(idx, _)| &t[..idx])
                .unwrap_or(&t)
                .to_string()
        } else {
            t
        };
    }
    for sel in [
        "#previewImageMain",
        ".workshopItemPreviewImageEnlarge img",
        ".workshopItemPreviewImage img",
        ".highlight_screenshot img",
        "meta[property=\"og:image\"]",
    ] {
        if let Some(el) = select_first(&doc, sel) {
            let src = el
                .value()
                .attr("src")
                .or_else(|| el.value().attr("content"))
                .unwrap_or("");
            if !src.is_empty() {
                item.preview_url = src.to_string();
                break;
            }
        }
    }

    if let Some(el) = select_first(
        &doc,
        ".friendBlockContent a, .workshopItemAuthorName a, .creatorsBlock .friendBlock a",
    ) {
        if let Some(href) = el.value().attr("href") {
            item.author_url = href.to_string();
        }
    }

    if let Some(el) = select_first(&doc, ".friendBlockContent") {
        let children: Vec<_> = el
            .children()
            .filter_map(|c| c.value().as_text().map(|t| t.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect();
        if let Some(first) = children.first() {
            item.author = first.clone();
        }
    }

    if let Some(el) = select_first(&doc, ".detailsStatRight") {
        item.file_size = text(el);
    }

    let stat_keys = ["Posted", "Posted:", "Updated", "Updated:"];
    let labels = &selectors::STAT_LEFT;
    let values = &selectors::STAT_RIGHT;
    let lbls: Vec<_> = doc.select(labels).map(text).collect();
    let vals: Vec<_> = doc.select(values).map(text).collect();
    for (i, lbl) in lbls.iter().enumerate() {
        if stat_keys.iter().any(|k| {
            lbl.trim_end_matches(':')
                .eq_ignore_ascii_case(k.trim_end_matches(':'))
        }) {
            let v = vals.get(i).cloned().unwrap_or_default();
            if lbl.to_lowercase().contains("post") {
                item.posted_date = v;
            } else if lbl.to_lowercase().contains("updat") {
                item.updated_date = v;
            }
        }
    }

    let rating_sel = &selectors::RATING_IMG;
    if let Some(img) = doc.select(rating_sel).next() {
        if let Some(src) = img.value().attr("src") {
            item.rating_star_file = src.to_string();
        }
    }

    let num_sel = &selectors::NUM_RATINGS;
    if let Some(el) = doc.select(num_sel).next() {
        let raw = text(el);
        let digits: String = raw
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == ',')
            .collect();
        item.num_ratings = digits;
    }

    // Workshop tags. Steam lists them as `<div class="workshopTags">Category:
    // <a>Tag1</a>, <a>Tag2</a></div>` — we extract both the category label
    // (e.g. "Type", "Genre", "Resolution") and the list of values so the UI
    // can render them grouped like the Python app does.
    let tag_block_sel = &selectors::TAG_BLOCK;
    let tag_link_sel = &selectors::ANCHOR;
    let mut tags = Vec::new();
    for block in doc.select(tag_block_sel) {
        // The category label is the leading text node before the <a> tags.
        // We pick the first text node that contains *real* content, not
        // just punctuation or whitespace — Steam sometimes renders a
        // leading "." or ", " separator that would otherwise become a
        // bogus category header in the UI.
        let mut category = String::new();
        for child in block.children() {
            if let Some(t) = child.value().as_text() {
                let raw = t.trim().trim_end_matches(':').trim();
                if raw.is_empty() {
                    continue;
                }
                // Skip pure-punctuation noise (".", ",", "·", etc.).
                if raw.chars().all(|c| !c.is_alphanumeric()) {
                    continue;
                }
                category = raw.to_string();
                break;
            }
        }
        for a in block.select(tag_link_sel) {
            let value = text(a);
            if value.is_empty() {
                continue;
            }
            let mut m = Map::new();
            m.insert("tag".into(), Value::String(value));
            if !category.is_empty() {
                m.insert("category".into(), Value::String(category.clone()));
            }
            tags.push(Value::Object(m));
        }
    }
    item.tags = Value::Array(tags);

    // Parent collections this item appears in. Steam emits:
    //   <div class="parentCollection" onClick="... id=12345 ...">
    //     <div class="parentCollectionTitle">Title</div>
    //     <div class="parentCollectionNumChildren">42 items</div>
    //   </div>
    let pc_sel = &selectors::PARENT_COLLECTION;
    let title_sel = &selectors::PARENT_COLLECTION_TITLE;
    let count_sel = &selectors::PARENT_COLLECTION_COUNT;
    let id_re = &selectors::ID_EQ_DIGITS;
    let num_re = &selectors::DIGITS;
    for pc in doc.select(pc_sel) {
        let onclick = pc
            .value()
            .attr("onClick")
            .or_else(|| pc.value().attr("onclick"))
            .unwrap_or("");
        let Some(cap) = id_re.captures(onclick) else {
            continue;
        };
        let id = cap[1].to_string();
        let title = pc.select(title_sel).next().map(text).unwrap_or_default();
        let count = pc
            .select(count_sel)
            .next()
            .map(|e| text(e))
            .and_then(|s| num_re.captures(&s).map(|c| c[1].to_string()))
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        item.collections.push(CollectionRef {
            id,
            title: if title.is_empty() {
                "Collection".to_string()
            } else {
                title
            },
            item_count: count,
        });
    }

    item
}
