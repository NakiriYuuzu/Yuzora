//! Result sets: what a query produced, kept here (not in the script) so the
//! native grid can page through 100k rows without copying them across the
//! script boundary.
//!
//! A result fills while its query streams; whoever shows it [`watch`]es it and
//! redraws on each signal. Pending cell edits live on the result too, keyed
//! by the row's position in `rows` (sorting only changes the view order).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Instant;

use serde::Serialize;

use crate::model::{Cell, Column, ObjectRef};

pub type ResultId = u64;

/// Where edits of a result are written: the table it was read from, and the
/// connection it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditTarget {
    pub connection: String,
    pub database: Option<String>,
    pub owner: String,
    pub table: ObjectRef,
    /// The primary key's columns, as indexes into the result's columns.
    pub key: Vec<usize>,
    /// Columns the database computes (generated, computed, an identity it
    /// numbers itself): shown, never edited.
    pub read_only: Vec<usize>,
}

#[derive(Debug)]
pub struct ResultSet {
    pub id: ResultId,
    pub statement: String,
    pub columns: Vec<Column>,
    /// Rows in the order the server sent them.
    pub rows: Vec<Vec<Cell>>,
    /// The view order (indexes into `rows`) while sorted; empty = as sent.
    pub order: Vec<u32>,
    pub sort: Option<(usize, bool)>,
    /// The query has finished producing this result.
    pub complete: bool,
    /// Stopped at the row cap: the query had more.
    pub truncated: bool,
    /// `count(*)` of the table, when the result is a table's rows.
    pub total: Option<u64>,
    pub affected: Option<u64>,
    pub error: Option<String>,
    pub started: Instant,
    pub elapsed_ms: u64,
    pub edit: Option<EditTarget>,
    /// Why this result can't be edited (shown instead of editing).
    pub not_editable: Option<String>,
    /// Changed cells: (row in `rows`, column) → new value.
    pub edits: BTreeMap<(u32, u32), Cell>,
    /// Writing edits back is in flight.
    pub writing: bool,
    /// The last write-back failed with this.
    pub write_error: Option<String>,
    /// Bumped on every change, so a watcher can tell what it has seen.
    pub version: u64,
}

impl ResultSet {
    fn new(id: ResultId, statement: String) -> Self {
        Self {
            id,
            statement,
            columns: Vec::new(),
            rows: Vec::new(),
            order: Vec::new(),
            sort: None,
            complete: false,
            truncated: false,
            total: None,
            affected: None,
            error: None,
            started: Instant::now(),
            elapsed_ms: 0,
            edit: None,
            not_editable: None,
            edits: BTreeMap::new(),
            writing: false,
            write_error: None,
            version: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The row shown at view position `i`.
    pub fn row_index(&self, i: usize) -> usize {
        if self.order.is_empty() { i } else { self.order.get(i).map_or(i, |r| *r as usize) }
    }

    /// A cell as shown: its pending edit if any.
    pub fn cell(&self, row: usize, column: usize) -> Option<&Cell> {
        self.edits.get(&(row as u32, column as u32)).or_else(|| self.rows.get(row)?.get(column))
    }

    pub fn is_edited(&self, row: usize, column: usize) -> bool {
        self.edits.contains_key(&(row as u32, column as u32))
    }

    pub fn editable(&self) -> bool {
        self.edit.is_some() && self.complete && self.error.is_none()
    }

    /// Whether a cell of `column` may be edited (not one the database computes).
    pub fn column_editable(&self, column: usize) -> bool {
        self.edit.as_ref().is_none_or(|e| !e.read_only.contains(&column))
    }

    /// More rows from the server. A sorted view takes them in where they
    /// sort, after the rows already there that tie with them.
    pub fn push_rows(&mut self, rows: impl IntoIterator<Item = Vec<Cell>>) {
        let from = self.rows.len() as u32;
        self.rows.extend(rows);
        let Some(sort) = self.sort else { return };
        let merged = {
            let order = self.row_order(sort);
            let mut added: Vec<u32> = (from..self.rows.len() as u32).collect();
            added.sort_by(&order);
            let mut merged = Vec::with_capacity(self.order.len() + added.len());
            let (mut old, mut new) = (self.order.iter().peekable(), added.iter().peekable());
            while let (Some(&&a), Some(&&b)) = (old.peek(), new.peek()) {
                if order(&b, &a).is_lt() {
                    merged.push(b);
                    new.next();
                } else {
                    merged.push(a);
                    old.next();
                }
            }
            merged.extend(old.chain(new));
            merged
        };
        self.order = merged;
    }

    /// How two rows (indexes into `rows`) compare under `sort`.
    fn row_order(&self, (column, descending): (usize, bool)) -> impl Fn(&u32, &u32) -> std::cmp::Ordering + '_ {
        let numeric = self.columns.get(column).is_some_and(|c| c.numeric);
        move |a, b| {
            let (x, y) = (self.rows[*a as usize].get(column), self.rows[*b as usize].get(column));
            let o = match (x, y) {
                (Some(x), Some(y)) => x.compare(y, numeric),
                _ => std::cmp::Ordering::Equal,
            };
            if descending { o.reverse() } else { o }
        }
    }

    /// Sorts the view by `column` (`None` restores the server's order).
    pub fn sort_by(&mut self, sort: Option<(usize, bool)>) {
        self.sort = sort;
        self.order.clear();
        if let Some(sort) = sort {
            let mut order: Vec<u32> = (0..self.rows.len() as u32).collect();
            order.sort_by(self.row_order(sort));
            self.order = order;
        }
        self.version += 1;
    }

    /// Sets (or, back to the original value, clears) one pending edit. A
    /// column the database computes takes none.
    pub fn set_edit(&mut self, row: usize, column: usize, value: Cell) {
        if !self.column_editable(column) {
            return;
        }
        let original = self.rows.get(row).and_then(|r| r.get(column));
        if original == Some(&value) {
            self.edits.remove(&(row as u32, column as u32));
        } else {
            self.edits.insert((row as u32, column as u32), value);
        }
        self.write_error = None;
        self.version += 1;
    }

    pub fn discard_edits(&mut self) {
        self.edits.clear();
        self.write_error = None;
        self.version += 1;
    }

    /// Makes the `written` edits the rows' values. An edit made while they
    /// were being written stays pending.
    pub fn commit_edits(&mut self, written: &BTreeMap<(u32, u32), Cell>) {
        for (&(row, column), value) in written {
            if let Some(cell) = self.rows.get_mut(row as usize).and_then(|r| r.get_mut(column as usize)) {
                *cell = value.clone();
            }
            if self.edits.get(&(row, column)) == Some(value) {
                self.edits.remove(&(row, column));
            }
        }
        if let Some(sort) = self.sort {
            self.sort_by(Some(sort));
        }
        self.write_error = None;
        self.version += 1;
    }

    pub fn summary(&self) -> Summary {
        Summary {
            id: self.id,
            statement: self.statement.clone(),
            columns: self.columns.len(),
            rows: self.rows.len(),
            total: self.total,
            complete: self.complete,
            truncated: self.truncated,
            affected: self.affected,
            error: self.error.clone(),
            elapsed_ms: self.elapsed_ms,
            editable: self.editable(),
            not_editable: self.not_editable.clone(),
            edits: self.edits.len(),
        }
    }
}

/// What a script sees of a result.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub id: ResultId,
    pub statement: String,
    pub columns: usize,
    pub rows: usize,
    pub total: Option<u64>,
    pub complete: bool,
    pub truncated: bool,
    pub affected: Option<u64>,
    pub error: Option<String>,
    pub elapsed_ms: u64,
    pub editable: bool,
    pub not_editable: Option<String>,
    pub edits: usize,
}

struct Entry {
    set: Arc<Mutex<ResultSet>>,
    /// Who owns it (a pane's key): released together.
    owner: String,
    watchers: Vec<async_channel::Sender<()>>,
}

struct Store {
    next: ResultId,
    entries: HashMap<ResultId, Entry>,
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store { next: 1, entries: HashMap::new() }));

fn store() -> MutexGuard<'static, Store> {
    STORE.lock().unwrap_or_else(|e| e.into_inner())
}

/// A new empty result for `statement`, owned by `owner`.
pub fn create(owner: &str, statement: &str) -> Arc<Mutex<ResultSet>> {
    let mut store = store();
    let id = store.next;
    store.next += 1;
    let set = Arc::new(Mutex::new(ResultSet::new(id, statement.to_owned())));
    store.entries.insert(id, Entry { set: set.clone(), owner: owner.to_owned(), watchers: Vec::new() });
    set
}

pub fn get(id: ResultId) -> Option<Arc<Mutex<ResultSet>>> {
    store().entries.get(&id).map(|e| e.set.clone())
}

/// Locks a result (a poisoned lock still reads: a panic mid-write leaves rows, not garbage).
pub fn lock(set: &Mutex<ResultSet>) -> MutexGuard<'_, ResultSet> {
    set.lock().unwrap_or_else(|e| e.into_inner())
}

/// Wakes everyone watching `id` (coalesced: one pending signal each).
pub fn changed(id: ResultId) {
    let mut store = store();
    if let Some(entry) = store.entries.get_mut(&id) {
        entry.watchers.retain(|w| match w.try_send(()) {
            Ok(()) | Err(async_channel::TrySendError::Full(())) => true,
            Err(async_channel::TrySendError::Closed(())) => false,
        });
    }
}

/// A signal for every change to `id` (none if it is gone).
pub fn watch(id: ResultId) -> Option<async_channel::Receiver<()>> {
    let mut store = store();
    let entry = store.entries.get_mut(&id)?;
    let (tx, rx) = async_channel::bounded(1);
    entry.watchers.push(tx);
    Some(rx)
}

/// Drops one result.
pub fn release(id: ResultId) {
    store().entries.remove(&id);
}

/// Drops every result `owner` made.
pub fn release_owner(owner: &str) {
    store().entries.retain(|_, e| e.owner != owner);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(owner: &str) -> Arc<Mutex<ResultSet>> {
        let set = create(owner, "select");
        {
            let mut r = lock(&set);
            r.columns = vec![Column::new("id", "int4"), Column::new("name", "text")];
            r.rows = vec![
                vec![Cell::text("10"), Cell::text("b")],
                vec![Cell::text("9"), Cell::Null],
                vec![Cell::text("100"), Cell::text("a")],
            ];
            r.complete = true;
        }
        set
    }

    #[test]
    fn sorting_reorders_the_view_not_the_rows() {
        let set = filled("t-sort");
        let mut r = lock(&set);
        r.sort_by(Some((0, false)));
        let shown: Vec<_> = (0..3).map(|i| r.rows[r.row_index(i)][0].display().into_owned()).collect();
        assert_eq!(shown, ["9", "10", "100"]);
        r.sort_by(Some((1, true)));
        assert_eq!(r.row_index(2), 1, "NULL sorts first, so last when descending");
        r.sort_by(None);
        assert_eq!(r.row_index(0), 0);
    }

    #[test]
    fn a_sort_takes_in_the_rows_that_arrive_after_it() {
        let set = filled("t-sort-stream");
        let mut r = lock(&set);
        r.complete = false;
        r.sort_by(Some((0, false)));
        r.push_rows(vec![vec![Cell::text("50"), Cell::Null], vec![Cell::text("1"), Cell::Null]]);
        r.push_rows(vec![vec![Cell::text("10"), Cell::text("tie")]]);
        let shown = |r: &ResultSet| (0..r.len()).map(|i| r.rows[r.row_index(i)][0].display().into_owned()).collect::<Vec<_>>();
        assert_eq!(shown(&r), ["1", "9", "10", "10", "50", "100"]);
        assert_eq!(r.row_index(3), 5, "ties keep the server's order");
        r.sort_by(Some((0, true)));
        r.push_rows(vec![vec![Cell::text("75"), Cell::Null]]);
        assert_eq!(shown(&r), ["100", "75", "50", "10", "10", "9", "1"]);
    }

    #[test]
    fn computed_columns_take_no_edits() {
        let set = filled("t-read-only");
        let mut r = lock(&set);
        r.edit = Some(EditTarget {
            connection: "c".into(),
            database: None,
            owner: "t-read-only".into(),
            table: ObjectRef { schema: String::new(), name: "t".into() },
            key: vec![0],
            read_only: vec![1],
        });
        r.set_edit(0, 1, Cell::text("z"));
        assert!(r.edits.is_empty(), "{:?}", r.edits);
        assert!(!r.column_editable(1) && r.column_editable(0));
        r.set_edit(0, 0, Cell::text("11"));
        assert!(r.is_edited(0, 0));
    }

    #[test]
    fn an_edit_back_to_the_original_value_is_no_edit() {
        let set = filled("t-edit");
        let mut r = lock(&set);
        r.set_edit(0, 1, Cell::text("z"));
        assert!(r.is_edited(0, 1));
        assert_eq!(r.cell(0, 1), Some(&Cell::text("z")));
        r.set_edit(0, 1, Cell::text("b"));
        assert!(r.edits.is_empty());
        r.set_edit(1, 1, Cell::text("x"));
        let written = r.edits.clone();
        r.set_edit(2, 1, Cell::text("during"));
        r.commit_edits(&written);
        assert_eq!(r.rows[1][1], Cell::text("x"));
        assert_eq!(r.edits.len(), 1, "an edit made while writing stays pending");
        assert!(r.is_edited(2, 1));
    }

    #[test]
    fn watchers_are_woken_once_per_burst_and_released_with_their_owner() {
        let set = filled("t-watch");
        let id = lock(&set).id;
        let rx = watch(id).unwrap();
        changed(id);
        changed(id);
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err(), "signals coalesce");
        release_owner("t-watch");
        assert!(get(id).is_none());
        assert!(watch(id).is_none());
    }
}
