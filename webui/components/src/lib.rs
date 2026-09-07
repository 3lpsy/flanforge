//! Shared widgets and pure display helpers for the flanforge web UI.

mod format;
mod table;
mod widgets;

pub use format::{absolute_time, human_time, now_secs, short_duration, short_id};
pub use table::{
    FilterSelect, Pager, SearchBox, Sort, SortableTh, distinct, page_slice, row_matches,
    total_pages,
};
pub use widgets::{ErrorState, Loading, LoginPrompt, StateBadge};
