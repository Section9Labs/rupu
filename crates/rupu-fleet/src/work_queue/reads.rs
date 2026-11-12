use super::{
    disk::{self, Store},
    types::*,
    validation::identifier,
};
use crate::Board;
use rusqlite::{params, params_from_iter, types::Value, OptionalExtension};

// Leave room for typed cursor, metadata and the outer CP owner/source envelope.
const RECORD_BUDGET: usize = MAX_PAGE_BYTES - 16 * 1024;

fn limit(limit: usize) -> Result<(), WorkError> {
    if limit == 0 || limit > MAX_PAGE_LIMIT {
        return Err(WorkError::InvalidInput(
            "Queue page limit must be between 1 and 200".into(),
        ));
    }
    Ok(())
}

fn byte_size<T: serde::Serialize>(value: &T) -> Result<usize, WorkError> {
    serde_json::to_vec(value)
        .map(|v| v.len())
        .map_err(disk::io_error)
}
fn too_large(requested: usize) -> WorkError {
    WorkError::Capacity {
        resource: WorkCapacityResource::PageBytes,
        limit: MAX_PAGE_BYTES as u64,
        current: 0,
        requested: requested as u64,
    }
}

impl Board {
    pub fn work_snapshot(&self) -> Result<WorkSnapshot, WorkError> {
        let Some(store) = Store::open(&self.root, false)? else {
            return Ok(disk::empty_snapshot());
        };
        let tx = store
            .conn
            .unchecked_transaction()
            .map_err(disk::sql_error)?;
        let result = disk::snapshot(&tx, &disk::read_meta(&tx)?)?;
        store.check_location()?;
        tx.commit().map_err(disk::sql_error)?;
        Ok(result)
    }

    pub fn work_get(&self, work_id: &str) -> Result<WorkItem, WorkError> {
        Ok(self.work_get_snapshot(work_id)?.item)
    }

    pub fn work_get_snapshot(&self, work_id: &str) -> Result<WorkDetail, WorkError> {
        identifier(work_id)?;
        let store = Store::open(&self.root, false)?.ok_or_else(|| WorkError::NotFound {
            work_id: work_id.into(),
        })?;
        let tx = store
            .conn
            .unchecked_transaction()
            .map_err(disk::sql_error)?;
        let snapshot = disk::snapshot(&tx, &disk::read_meta(&tx)?)?;
        let item = disk::item(&tx, work_id)?;
        if item.latest_event.queue_revision > snapshot.revision {
            return Err(WorkError::Corrupt("item exceeds Queue revision".into()));
        }
        store.check_location()?;
        tx.commit().map_err(disk::sql_error)?;
        Ok(WorkDetail { snapshot, item })
    }

    pub fn work_list(&self, query: WorkPageQuery) -> Result<WorkPage, WorkError> {
        limit(query.limit)?;
        if let Some(run) = &query.filters.claimant_run_id {
            identifier(run)?;
        }
        let Some(store) = Store::open(&self.root, false)? else {
            if query.cursor.is_some() {
                return Err(WorkError::CursorConflict);
            }
            return Ok(WorkPage {
                snapshot: disk::empty_snapshot(),
                filters: query.filters,
                order: WorkOrder::CreationSequenceDescending,
                items: Vec::new(),
                counts: WorkCounts {
                    scope: WorkCountScope::MatchingFiltersAtSourceRevision,
                    returned: 0,
                    matching_total: 0,
                },
                next: None,
                has_more: false,
            });
        };
        let tx = store
            .conn
            .unchecked_transaction()
            .map_err(disk::sql_error)?;
        let meta = disk::read_meta(&tx)?;
        let snapshot = disk::snapshot(&tx, &meta)?;
        if let Some(cursor) = &query.cursor {
            if Some(&cursor.queue_id) != meta.queue_id.as_ref()
                || cursor.source_revision != meta.revision
                || cursor.filters != query.filters
                || cursor.before_creation_sequence == 0
                || cursor.before_creation_sequence > meta.items
            {
                return Err(WorkError::CursorConflict);
            }
        }
        let state = query.filters.state.map_or("*", WorkState::key);
        let claimant = query.filters.claimant_run_id.as_deref().unwrap_or("*");
        let total: u64 = tx
            .query_row(
                "SELECT count FROM counts WHERE state=?1 AND claimant=?2",
                params![state, claimant],
                |r| disk::number(r, 0),
            )
            .optional()
            .map_err(disk::sql_error)?
            .unwrap_or(0);
        if total > meta.items || (query.filters == WorkFilters::default() && total != meta.items) {
            return Err(WorkError::Corrupt(
                "Queue count index disagrees with metadata".into(),
            ));
        }
        let mut clauses = Vec::new();
        let mut values: Vec<Value> = Vec::new();
        if let Some(state) = query.filters.state {
            clauses.push("state=?");
            values.push(state.key().to_owned().into());
        }
        if let Some(run) = &query.filters.claimant_run_id {
            clauses.push("claimant=?");
            values.push(run.clone().into());
        }
        if let Some(cursor) = &query.cursor {
            clauses.push("creation_sequence<?");
            values.push((cursor.before_creation_sequence as i64).into());
        }
        let filter = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };
        values.push(((query.limit + 1) as i64).into());
        let sql=format!("SELECT id,creation_sequence,revision,summary FROM items{filter} ORDER BY creation_sequence DESC LIMIT ?");
        let mut statement = tx.prepare(&sql).map_err(disk::sql_error)?;
        let mut rows = statement
            .query(params_from_iter(values))
            .map_err(disk::sql_error)?;
        let mut items = Vec::new();
        let mut bytes = 0;
        let mut has_more = false;
        while let Some(row) = rows.next().map_err(disk::sql_error)? {
            if items.len() == query.limit {
                has_more = true;
                break;
            }
            let raw: String = row.get(3).map_err(disk::sql_error)?;
            let item: WorkItemSummary = disk::decode(&raw)?;
            if item.work_id != row.get::<_, String>(0).map_err(disk::sql_error)?
                || item.creation_sequence != disk::number(row, 1).map_err(disk::sql_error)?
                || item.revision != disk::number(row, 2).map_err(disk::sql_error)?
            {
                return Err(WorkError::Corrupt("Queue summary identity mismatch".into()));
            }
            if bytes + raw.len() > RECORD_BUDGET {
                if items.is_empty() {
                    return Err(too_large(raw.len()));
                }
                has_more = true;
                break;
            }
            bytes += raw.len() + 1;
            items.push(item);
        }
        drop(rows);
        drop(statement);
        let next = if has_more {
            Some(WorkPagePosition {
                queue_id: meta
                    .queue_id
                    .clone()
                    .ok_or_else(|| WorkError::Corrupt("items without Queue identity".into()))?,
                source_revision: meta.revision,
                filters: query.filters.clone(),
                before_creation_sequence: items.last().expect("page has record").creation_sequence,
            })
        } else {
            None
        };
        let page = WorkPage {
            snapshot,
            filters: query.filters,
            order: WorkOrder::CreationSequenceDescending,
            counts: WorkCounts {
                scope: WorkCountScope::MatchingFiltersAtSourceRevision,
                returned: items.len() as u64,
                matching_total: total,
            },
            items,
            next,
            has_more,
        };
        if byte_size(&page)? > MAX_PAGE_BYTES {
            return Err(too_large(byte_size(&page)?));
        }
        store.check_location()?;
        tx.commit().map_err(disk::sql_error)?;
        Ok(page)
    }

    pub fn work_timeline(&self, query: WorkTimelineQuery) -> Result<WorkTimelinePage, WorkError> {
        limit(query.limit)?;
        identifier(&query.work_id)?;
        let store = Store::open(&self.root, false)?.ok_or_else(|| WorkError::NotFound {
            work_id: query.work_id.clone(),
        })?;
        let tx = store
            .conn
            .unchecked_transaction()
            .map_err(disk::sql_error)?;
        let meta = disk::read_meta(&tx)?;
        let snapshot = disk::snapshot(&tx, &meta)?;
        let item = disk::item(&tx, &query.work_id)?;
        let queue_id = meta
            .queue_id
            .ok_or_else(|| WorkError::Corrupt("item without Queue identity".into()))?;
        if let Some(cursor) = &query.cursor {
            if cursor.queue_id != queue_id
                || cursor.work_id != query.work_id
                || cursor.source_item_revision != item.revision
                || cursor.after_event_sequence == 0
                || cursor.after_event_sequence > meta.revision
            {
                return Err(WorkError::CursorConflict);
            }
        }
        let after = query.cursor.as_ref().map_or(0, |c| c.after_event_sequence);
        let mut statement=tx.prepare("SELECT sequence,id,item_revision,payload FROM events WHERE item_id=?1 AND sequence>?2 ORDER BY sequence ASC LIMIT ?3").map_err(disk::sql_error)?;
        let mut rows = statement
            .query(params![
                query.work_id,
                after as i64,
                (query.limit + 1) as i64
            ])
            .map_err(disk::sql_error)?;
        let mut events = Vec::new();
        let mut bytes = 0;
        let mut has_more = false;
        while let Some(row) = rows.next().map_err(disk::sql_error)? {
            if events.len() == query.limit {
                has_more = true;
                break;
            }
            let raw: String = row.get(3).map_err(disk::sql_error)?;
            let event: WorkEvent = disk::decode(&raw)?;
            if event.sequence != disk::number(row, 0).map_err(disk::sql_error)?
                || event.event_id != row.get::<_, String>(1).map_err(disk::sql_error)?
                || event.item_revision != disk::number(row, 2).map_err(disk::sql_error)?
                || event.work_id != query.work_id
                || event.item_revision > item.revision
            {
                return Err(WorkError::Corrupt("Queue event identity mismatch".into()));
            }
            if bytes + raw.len() > RECORD_BUDGET {
                if events.is_empty() {
                    return Err(too_large(raw.len()));
                }
                has_more = true;
                break;
            }
            bytes += raw.len() + 1;
            events.push(event);
        }
        drop(rows);
        drop(statement);
        let next = if has_more {
            Some(WorkTimelinePosition {
                queue_id: queue_id.clone(),
                work_id: query.work_id.clone(),
                source_item_revision: item.revision,
                after_event_sequence: events.last().expect("page has record").sequence,
            })
        } else {
            None
        };
        let page = WorkTimelinePage {
            snapshot,
            queue_id,
            work_id: query.work_id,
            source_item_revision: item.revision,
            order: WorkOrder::EventSequenceAscending,
            counts: WorkCounts {
                scope: WorkCountScope::MatchingFiltersAtSourceRevision,
                returned: events.len() as u64,
                matching_total: item.event_count,
            },
            events,
            next,
            has_more,
        };
        if byte_size(&page)? > MAX_PAGE_BYTES {
            return Err(too_large(byte_size(&page)?));
        }
        store.check_location()?;
        tx.commit().map_err(disk::sql_error)?;
        Ok(page)
    }
}
