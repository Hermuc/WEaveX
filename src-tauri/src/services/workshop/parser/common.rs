//! Shared helpers used by more than one Workshop parser.
//!
//! These were previously private functions in a single 1000-line
//! `parser.rs`; they are now `pub(crate)` so the per-concern parser
//! submodules (`browse`, `item`, `collection`) can reuse them.

use regex::Regex;
use scraper::{ElementRef, Html, Selector};

pub(crate) fn extract_pubfileid(href: &str) -> Option<String> {
    let re = Regex::new(r#"[?&]id=(\d+)"#).unwrap();
    re.captures(href).map(|c| c[1].to_string())
}

pub(crate) fn inner_text(el: ElementRef<'_>) -> String {
    el.text().collect::<String>().trim().to_string()
}

pub(crate) fn text(el: ElementRef<'_>) -> String {
    inner_text(el)
}

pub(crate) fn select_first<'a>(doc: &'a Html, sel: &str) -> Option<ElementRef<'a>> {
    Selector::parse(sel)
        .ok()
        .and_then(|s| doc.select(&s).next())
}

/// `<element>...</element>` -> visible text, preserving line breaks for
/// `<br>` and block-level descendants. Used for collection descriptions
/// where the raw `el.text()` join collapses paragraphs.
pub(crate) fn inner_text_multiline(el: ElementRef<'_>) -> String {
    use ego_tree::iter::Edge;
    let mut out = String::new();
    for edge in el.traverse() {
        match edge {
            Edge::Open(node) => {
                if let Some(t) = node.value().as_text() {
                    out.push_str(t);
                } else if let Some(e) = node.value().as_element() {
                    let name = e.name();
                    if name == "br" {
                        out.push('\n');
                    }
                }
            }
            Edge::Close(node) => {
                if let Some(e) = node.value().as_element() {
                    let name = e.name();
                    if matches!(
                        name,
                        "p" | "div" | "li" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                    ) {
                        out.push('\n');
                    }
                }
            }
        }
    }
    // Collapse runs of 3+ newlines to 2 to keep descriptions tidy.
    let re = Regex::new(r"\n{3,}").unwrap();
    re.replace_all(out.trim(), "\n\n").to_string()
}
