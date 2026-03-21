use crate::args::SortOrder;

pub fn modified_time(path: &std::path::Path) -> std::time::SystemTime {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
}

pub fn sort_notes_by<T>(items: &mut [T], key: impl Fn(&T) -> &std::path::Path, sort: &SortOrder) {
    match sort {
        SortOrder::PathAsc => items.sort_by(|a, b| key(a).cmp(key(b))),
        SortOrder::PathDesc => items.sort_by(|a, b| key(b).cmp(key(a))),
        SortOrder::ModifiedAsc => items.sort_by_key(|a| modified_time(key(a))),
        SortOrder::ModifiedDesc => items.sort_by_key(|b| std::cmp::Reverse(modified_time(key(b)))),
    }
}
