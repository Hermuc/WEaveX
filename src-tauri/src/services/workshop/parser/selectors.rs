//! Static CSS selectors and regexes for the Workshop HTML parsers.
//!
//! These used to be constructed inline on every parse call as
//! `Selector::parse(...).unwrap()`. The patterns are constant, so building
//! them once at first use removes both the repeated parsing cost and the
//! scattered `.unwrap()`s (a bad selector would now panic once, at startup,
//! rather than on every request).

use once_cell::sync::Lazy;
use regex::Regex;
use scraper::Selector;

macro_rules! selector {
    ($name:ident, $pat:expr) => {
        pub static $name: Lazy<Selector> =
            Lazy::new(|| Selector::parse($pat).expect(concat!("selector: ", $pat)));
    };
}

macro_rules! regex {
    ($name:ident, $pat:expr) => {
        pub static $name: Lazy<Regex> =
            Lazy::new(|| Regex::new($pat).expect(concat!("regex: ", $pat)));
    };
}

// ── browse.rs ────────────────────────────────────────────────────────────
selector!(BROWSE_CONTAINER, ".tmIrUKf-Mh8-, .sDK5fonBQMA-, .workshopItem, .workshopItemCollection, a.workshopItemCollection");
selector!(BROWSE_LINK, "a[href*=\"filedetails\"]");
selector!(BROWSE_LINK_EXACT, "a[href*=\"filedetails/?id=\"]");
selector!(BROWSE_TITLE, "._3rvey4VpXts- a, .workshopItemTitle");
selector!(IMG, "img");
selector!(BROWSE_AUTHOR_LINK, ".CmHGWYJjMk0- a, .workshopItemAuthorName a, a.workshop_author_link, a[href*=\"/myworkshopfiles/\"], a[href*=\"/myworkshopcollections/\"]");
selector!(BROWSE_AUTHOR_TEXT, ".workshopItemAuthorName");
regex!(BROWSE_AUTHOR_PREFIX, "(?i)^\\s*(?:by|collection\\s+by)\\s+");
regex!(BROWSE_AUTHOR_HREF, "<a href=\"(https://steamcommunity\\.com/(?:id|profiles)/[^\"]+/(?:myworkshopfiles|myworkshopcollections)/[^\"]*)\"[^>]*>By\\s+([^<]+)</a>");
regex!(FILEDETAILS_ID, "filedetails/\\?id=(\\d+)");
regex!(SSR_QUERIES, "queries\"\\s*:\\s*\"((?:\\\\.|[^\"\\\\])*)\"");

// ── pagination ───────────────────────────────────────────────────────────
regex!(PAGE_ELLIPSIS_LAST, "<div>\\.\\.\\.</div><div[^>]*class=\"[^\"]*Panel[^\"]*\"[^>]*tabindex=\"0\"[^>]*role=\"button\"[^>]*>\\s*([\\d,]+)\\s*</div>");
regex!(PAGE_TOTAL_SHORT, "(?:Showing)\\s+\\d+[\\s\\-–]\\d+\\s+(?:of)\\s+([\\d,.\\s]+)");
selector!(PAGE_CONTROLS, ".workshopBrowsePagingControls a, .workshopBrowsePagingControls span");
regex!(PAGE_SHOWING, "(?:Showing)\\s+(\\d+)[\\s\\-–](\\d+)\\s+(?:of)\\s+([\\d,.\\s]+)");
regex!(PAGE_LINK, "class=\"pagelink\"[^>]*>\\s*(\\d+)\\s*<");
regex!(PAGE_CURRENT, "class=\"page_current\"[^>]*>\\s*(\\d+)\\s*<");

// ── collection.rs ────────────────────────────────────────────────────────
regex!(DIGITS, "(\\d+)");
selector!(STAT_LEFT, ".detailsStatLeft");
selector!(STAT_RIGHT, ".detailsStatRight");
selector!(TAG_BLOCK, ".workshopTags");
selector!(TAG_TITLE, ".workshopTagsTitle");
selector!(ANCHOR, "a");
selector!(COLLECTION_ITEM, ".collectionItem");
selector!(COLLECTION_PREVIEW, ".workshopItemPreviewImage");
selector!(WORKSHOP_TITLE, ".workshopItemTitle");
selector!(WORKSHOP_AUTHOR_ANCHOR, ".workshopItemAuthorName a");
selector!(COLLECTION_RELATED, ".collections > .workshopItem");
selector!(COLLECTION_COUNT, ".workshopCollectionNumChildren");

// ── item.rs ──────────────────────────────────────────────────────────────
selector!(RATING_IMG, ".fileRatingDetails img");
selector!(NUM_RATINGS, ".numRatings");
selector!(PARENT_COLLECTION, ".parentCollection");
selector!(PARENT_COLLECTION_TITLE, ".parentCollectionTitle");
selector!(PARENT_COLLECTION_COUNT, ".parentCollectionNumChildren");
regex!(ID_EQ_DIGITS, "id=(\\d+)");