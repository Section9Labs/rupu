//! Shared offset/limit pagination for the list endpoints.
//!
//! Query params are lenient: a missing or unparseable bound falls back to the
//! default (offset 0, limit 20) rather than erroring, so a bad query string
//! never 500s a list. `limit` is clamped to `[1, 200]`.

use serde::Deserialize;

/// Default page size when `limit` is absent.
pub const DEFAULT_LIMIT: usize = 20;
/// Hard cap on `limit`.
pub const MAX_LIMIT: usize = 200;

/// Optional `?offset=&limit=` query params for a list endpoint.
#[derive(Debug, Default, Deserialize)]
pub struct PageQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

impl PageQuery {
    /// Resolved offset (default 0).
    pub fn offset(&self) -> usize {
        self.offset.unwrap_or(0)
    }
    /// Resolved limit (default `DEFAULT_LIMIT`, clamped to `[1, MAX_LIMIT]`).
    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
    }
}

/// Slice `items` to the `[offset, offset+limit)` window. Out-of-range offset
/// yields an empty vec. Consumes the input so handlers can compute expensive
/// per-row work on the returned page only.
pub fn paginate<T>(items: Vec<T>, page: &PageQuery) -> Vec<T> {
    let offset = page.offset();
    let limit = page.limit();
    items.into_iter().skip(offset).take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(offset: Option<usize>, limit: Option<usize>) -> PageQuery {
        PageQuery { offset, limit }
    }

    #[test]
    fn default_limit_is_20() {
        let items: Vec<u32> = (0..50).collect();
        let page = paginate(items, &q(None, None));
        assert_eq!(page.len(), 20);
        assert_eq!(page[0], 0);
        assert_eq!(page[19], 19);
    }

    #[test]
    fn offset_and_limit_slice() {
        let items: Vec<u32> = (0..50).collect();
        let page = paginate(items, &q(Some(20), Some(5)));
        assert_eq!(page, vec![20, 21, 22, 23, 24]);
    }

    #[test]
    fn offset_past_end_is_empty() {
        let items: Vec<u32> = (0..10).collect();
        assert!(paginate(items, &q(Some(100), Some(20))).is_empty());
    }

    #[test]
    fn limit_is_clamped() {
        assert_eq!(q(None, Some(0)).limit(), 1);
        assert_eq!(q(None, Some(9999)).limit(), MAX_LIMIT);
        assert_eq!(q(None, Some(50)).limit(), 50);
    }
}
