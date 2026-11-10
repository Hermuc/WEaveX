//! Steam Workshop HTML parsers, split by concern:
//!
//!   - `common`     – shared text / regex helpers
//!   - `browse`     – browse-page listing parser (`parse_browse`)
//!   - `item`       – item-details parser (`parse_item_details`)
//!   - `collection` – collection-contents parser (`parse_collection_contents`)
//!
//! The public surface is re-exported here so callers keep using
//! `parser::parse_browse(..)` etc. unchanged.

mod browse;
mod collection;
mod common;
mod item;

pub use browse::parse_browse;
pub use collection::parse_collection_contents;
pub use item::parse_item_details;
