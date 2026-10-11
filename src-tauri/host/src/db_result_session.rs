//! Bounded, exact-owner materialized result sessions.

use std::collections::HashMap;
use std::mem::size_of;
use std::sync::{Arc, Mutex};

use crate::db_service::{
    ConnectionIdentity, DbValue, DescriptorId, EffectOutcome, QueryRunOwner, ResultPage,
    ResultSession, ResultSessionLifecycle, ResultSessionOwner,
};

pub const RESULT_PAGE_ROWS: usize = 500;
pub const DEFAULT_FIELD_BYTES: usize = 1024 * 1024;
pub const DEFAULT_ROW_BYTES: usize = 8 * 1024 * 1024;
pub const DEFAULT_SESSION_BYTES: usize = 64 * 1024 * 1024;
pub const DEFAULT_PROCESS_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultLimitKind {
    Field,
    Row,
    Session,
    Process,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushRowOutcome {
    Stored,
    LimitReached,
    ValueTooLarge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemainingBudget {
    pub field: usize,
    pub row: usize,
    pub session: usize,
    pub process: usize,
}

impl RemainingBudget {
    pub fn convertible_remaining(&self) -> usize {
        self.row.min(self.session).min(self.process)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum NextPage {
    Cached(ResultPage),
    Continue { page_index: usize },
}

pub type SessionLifecycle = ResultSessionLifecycle;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionError {
    OwnerMismatch,
    SessionAlreadyExists,
    SessionNotFound,
    PageNotFound,
    BudgetExceeded,
    LockUnavailable,
}

#[derive(Clone)]
struct ActiveRun {
    owner: QueryRunOwner,
}

struct StoredSession {
    owner: ResultSessionOwner,
    // The stored map key owns an immutable allocation separate from the owner ID.
    map_key_capacity: usize,
    columns: Vec<String>,
    pages: Vec<Vec<Vec<DbValue>>>,
    ready_pages: usize,
    current_page: usize,
    bytes: usize,
    effect_outcome: EffectOutcome,
    lifecycle: SessionLifecycle,
    result_limit_reached: bool,
    value_too_large: bool,
}

pub struct ResultSessionRegistry {
    sessions: HashMap<String, StoredSession>,
    active_runs: HashMap<String, ActiveRun>,
    field_limit: usize,
    row_limit: usize,
    session_limit: usize,
    process_limit: usize,
    total_bytes: usize,
}

impl Default for ResultSessionRegistry {
    fn default() -> Self {
        Self::with_limits(DEFAULT_SESSION_BYTES, DEFAULT_PROCESS_BYTES)
    }
}

impl ResultSessionRegistry {
    pub fn with_limits(session_limit: usize, process_limit: usize) -> Self {
        Self::with_ceilings(
            DEFAULT_FIELD_BYTES,
            DEFAULT_ROW_BYTES,
            session_limit,
            process_limit,
        )
    }

    pub fn with_ceilings(
        field_limit: usize,
        row_limit: usize,
        session_limit: usize,
        process_limit: usize,
    ) -> Self {
        Self {
            sessions: HashMap::new(),
            active_runs: HashMap::new(),
            field_limit,
            row_limit,
            session_limit,
            process_limit,
            total_bytes: 0,
        }
    }

    /// Starts one descriptor's new run and deterministically releases all
    /// materialized sessions from its previous run.
    pub fn begin_run(&mut self, owner: &QueryRunOwner) -> Result<(), SessionError> {
        self.release_descriptor(&owner.descriptor_id);
        self.active_runs.insert(
            owner.descriptor_id.0.clone(),
            ActiveRun {
                owner: owner.clone(),
            },
        );
        Ok(())
    }

    pub fn begin_session(
        &mut self,
        owner: ResultSessionOwner,
        columns: Vec<String>,
    ) -> Result<(), SessionError> {
        self.validate_active_run(&owner)?;
        let key = owner.result_session_id.0.clone();
        if self.sessions.contains_key(&key) {
            return Err(SessionError::SessionAlreadyExists);
        }
        let mut probe = StoredSession {
            owner,
            map_key_capacity: 0,
            columns,
            pages: vec![Vec::new()],
            ready_pages: 0,
            current_page: 0,
            bytes: 0,
            effect_outcome: EffectOutcome::Unknown,
            lifecycle: ResultSessionLifecycle::Streaming,
            result_limit_reached: false,
            value_too_large: false,
        };
        let session_bytes = estimate_session_retained_bytes(&key, &probe);
        let map_growth = map_insert_growth_bytes(self.sessions.len(), self.sessions.capacity());
        let projected_process = self
            .total_bytes
            .saturating_add(session_bytes)
            .saturating_add(map_growth);
        if session_bytes > self.session_limit || projected_process > self.process_limit {
            return Err(SessionError::BudgetExceeded);
        }
        let stored_key = key.clone();
        probe.map_key_capacity = stored_key.capacity();
        probe.bytes = session_bytes;
        self.sessions.insert(stored_key, probe);
        self.refresh_accounting();
        debug_assert!(self
            .sessions
            .get(&key)
            .is_some_and(|session| session.bytes <= self.session_limit));
        debug_assert!(self.total_bytes <= self.process_limit);
        Ok(())
    }

    pub fn remaining_budget(
        &self,
        owner: &ResultSessionOwner,
    ) -> Result<RemainingBudget, SessionError> {
        self.validate_active_run(owner)?;
        let session = self
            .sessions
            .get(&owner.result_session_id.0)
            .ok_or(SessionError::SessionNotFound)?;
        if session.owner != *owner {
            return Err(SessionError::OwnerMismatch);
        }
        Ok(RemainingBudget {
            field: self.field_limit,
            row: self.row_limit,
            session: self.session_limit.saturating_sub(session.bytes),
            process: self.process_limit.saturating_sub(self.total_bytes),
        })
    }

    pub fn field_limit(&self) -> usize {
        self.field_limit
    }

    pub fn row_limit(&self) -> usize {
        self.row_limit
    }

    pub fn classify_raw_field(
        &self,
        raw_len: usize,
        row_used: usize,
        owner: &ResultSessionOwner,
    ) -> Result<Option<ResultLimitKind>, SessionError> {
        let budget = self.remaining_budget(owner)?;
        if raw_len > budget.field {
            return Ok(Some(ResultLimitKind::Field));
        }
        if row_used.saturating_add(raw_len) > budget.row {
            return Ok(Some(ResultLimitKind::Row));
        }
        let remaining = budget.session.min(budget.process);
        if row_used.saturating_add(raw_len) > remaining {
            return Ok(Some(if budget.session <= budget.process {
                ResultLimitKind::Session
            } else {
                ResultLimitKind::Process
            }));
        }
        Ok(None)
    }

    pub fn mark_result_limit_reached(
        &mut self,
        owner: &ResultSessionOwner,
    ) -> Result<(), SessionError> {
        self.validate_active_run(owner)?;
        let session = self
            .sessions
            .get_mut(&owner.result_session_id.0)
            .ok_or(SessionError::SessionNotFound)?;
        if session.owner != *owner {
            return Err(SessionError::OwnerMismatch);
        }
        session.result_limit_reached = true;
        Ok(())
    }

    pub fn mark_value_too_large(&mut self, owner: &ResultSessionOwner) -> Result<(), SessionError> {
        self.validate_active_run(owner)?;
        let session = self
            .sessions
            .get_mut(&owner.result_session_id.0)
            .ok_or(SessionError::SessionNotFound)?;
        if session.owner != *owner {
            return Err(SessionError::OwnerMismatch);
        }
        session.value_too_large = true;
        Ok(())
    }

    pub fn push_row(
        &mut self,
        owner: &ResultSessionOwner,
        row: Vec<DbValue>,
    ) -> Result<PushRowOutcome, SessionError> {
        self.validate_active_run(owner)?;
        let key = owner.result_session_id.0.as_str();
        let session = self
            .sessions
            .get_mut(key)
            .ok_or(SessionError::SessionNotFound)?;
        if session.owner != *owner {
            return Err(SessionError::OwnerMismatch);
        }
        if session.value_too_large {
            return Ok(PushRowOutcome::ValueTooLarge);
        }
        if session.result_limit_reached {
            return Ok(PushRowOutcome::LimitReached);
        }
        let row_bytes = match classify_converted_row(&row, self.field_limit, self.row_limit) {
            Ok(bytes) => bytes,
            Err(_) => {
                session.value_too_large = true;
                return Ok(PushRowOutcome::ValueTooLarge);
            }
        };
        let projected_session = estimate_session_after_push(
            &owner.result_session_id.0,
            session.map_key_capacity,
            session,
            row_bytes,
        );
        let process_without_session = self.total_bytes.saturating_sub(session.bytes);
        let projected_process = process_without_session.saturating_add(projected_session);
        if projected_session > self.session_limit || projected_process > self.process_limit {
            session.result_limit_reached = true;
            return Ok(PushRowOutcome::LimitReached);
        }
        let new_page = session
            .pages
            .last()
            .is_some_and(|page| page.len() == RESULT_PAGE_ROWS);
        let pages_capacity_before = session.pages.capacity();
        let rows_capacity_before = if new_page {
            0
        } else {
            session.pages.last().map(Vec::capacity).unwrap_or(0)
        };
        if new_page {
            session.pages.push(Vec::new());
        }
        let page = session
            .pages
            .last_mut()
            .expect("a materialized session always has one page");
        page.push(row);
        let added_bytes = row_bytes
            .saturating_add(
                page.capacity()
                    .saturating_sub(rows_capacity_before)
                    .saturating_mul(size_of::<Vec<DbValue>>()),
            )
            .saturating_add(
                session
                    .pages
                    .capacity()
                    .saturating_sub(pages_capacity_before)
                    .saturating_mul(size_of::<Vec<Vec<DbValue>>>()),
            );
        // Appending a row changes only these allocations. The map, owner,
        // columns and other sessions keep their already-accounted storage.
        session.bytes = session.bytes.saturating_add(added_bytes);
        self.total_bytes = self.total_bytes.saturating_add(added_bytes);
        debug_assert!(self
            .sessions
            .get(key)
            .is_some_and(|session| session.bytes <= self.session_limit));
        debug_assert!(self.total_bytes <= self.process_limit);
        Ok(PushRowOutcome::Stored)
    }

    pub fn finish_session(
        &mut self,
        owner: &ResultSessionOwner,
        effect_outcome: EffectOutcome,
    ) -> Result<ResultSession, SessionError> {
        self.finish_session_with_lifecycle(owner, effect_outcome, ResultSessionLifecycle::Complete)
    }

    pub fn finish_session_with_lifecycle(
        &mut self,
        owner: &ResultSessionOwner,
        effect_outcome: EffectOutcome,
        lifecycle: ResultSessionLifecycle,
    ) -> Result<ResultSession, SessionError> {
        self.validate_active_run(owner)?;
        let session = self
            .sessions
            .get_mut(&owner.result_session_id.0)
            .ok_or(SessionError::SessionNotFound)?;
        if session.owner != *owner {
            return Err(SessionError::OwnerMismatch);
        }
        session.effect_outcome = effect_outcome;
        session.lifecycle = lifecycle;
        session.ready_pages = session.pages.len();
        let initial_page = page_from_session(session, 0)?;
        Ok(ResultSession {
            owner: owner.clone(),
            columns: session.columns.clone(),
            initial_page,
        })
    }

    pub fn mark_page_ready(
        &mut self,
        owner: &ResultSessionOwner,
        page_index: usize,
    ) -> Result<ResultPage, SessionError> {
        self.validate_active_run(owner)?;
        let session = self
            .sessions
            .get_mut(&owner.result_session_id.0)
            .ok_or(SessionError::SessionNotFound)?;
        if session.owner != *owner || page_index >= session.pages.len() {
            return Err(SessionError::OwnerMismatch);
        }
        if page_index > session.ready_pages {
            return Err(SessionError::PageNotFound);
        }
        session.ready_pages = session.ready_pages.max(page_index + 1);
        page_from_session(session, page_index)
    }

    pub fn result_session(
        &self,
        owner: &ResultSessionOwner,
    ) -> Result<ResultSession, SessionError> {
        self.validate_active_run(owner)?;
        let session = self
            .sessions
            .get(&owner.result_session_id.0)
            .ok_or(SessionError::SessionNotFound)?;
        if session.owner != *owner {
            return Err(SessionError::OwnerMismatch);
        }
        Ok(ResultSession {
            owner: owner.clone(),
            columns: session.columns.clone(),
            initial_page: page_from_session(session, 0)?,
        })
    }

    pub fn page(
        &self,
        owner: &ResultSessionOwner,
        page_index: usize,
    ) -> Result<ResultPage, SessionError> {
        self.validate_active_run(owner)?;
        let session = self
            .sessions
            .get(&owner.result_session_id.0)
            .ok_or(SessionError::OwnerMismatch)?;
        if session.owner != *owner {
            return Err(SessionError::OwnerMismatch);
        }
        page_from_session(session, page_index)
    }

    pub fn previous(&mut self, owner: &ResultSessionOwner) -> Result<ResultPage, SessionError> {
        self.validate_active_run(owner)?;
        let session = self
            .sessions
            .get_mut(&owner.result_session_id.0)
            .ok_or(SessionError::SessionNotFound)?;
        if session.owner != *owner {
            return Err(SessionError::OwnerMismatch);
        }
        if session.current_page == 0 {
            return Err(SessionError::PageNotFound);
        }
        session.current_page -= 1;
        page_from_session(session, session.current_page)
    }

    pub fn next(&mut self, owner: &ResultSessionOwner) -> Result<NextPage, SessionError> {
        self.validate_active_run(owner)?;
        let session = self
            .sessions
            .get_mut(&owner.result_session_id.0)
            .ok_or(SessionError::SessionNotFound)?;
        if session.owner != *owner {
            return Err(SessionError::OwnerMismatch);
        }
        if session.lifecycle == ResultSessionLifecycle::Released {
            return Err(SessionError::PageNotFound);
        }
        let page_index = session
            .current_page
            .checked_add(1)
            .ok_or(SessionError::PageNotFound)?;
        if page_index < session.ready_pages {
            session.current_page = page_index;
            return Ok(NextPage::Cached(page_from_session(session, page_index)?));
        }
        if session.lifecycle == ResultSessionLifecycle::Streaming
            && page_index < session.pages.len()
        {
            return Ok(NextPage::Continue { page_index });
        }
        Err(SessionError::PageNotFound)
    }

    pub fn complete_next(
        &mut self,
        owner: &ResultSessionOwner,
        page_index: usize,
    ) -> Result<ResultPage, SessionError> {
        self.validate_active_run(owner)?;
        let session = self
            .sessions
            .get_mut(&owner.result_session_id.0)
            .ok_or(SessionError::SessionNotFound)?;
        if session.owner != *owner
            || page_index != session.current_page.saturating_add(1)
            || page_index >= session.ready_pages
        {
            return Err(SessionError::PageNotFound);
        }
        session.current_page = page_index;
        page_from_session(session, page_index)
    }

    pub fn current_page(&self, owner: &ResultSessionOwner) -> Result<ResultPage, SessionError> {
        self.validate_active_run(owner)?;
        let session = self
            .sessions
            .get(&owner.result_session_id.0)
            .ok_or(SessionError::SessionNotFound)?;
        if session.owner != *owner {
            return Err(SessionError::OwnerMismatch);
        }
        page_from_session(session, session.current_page)
    }

    pub fn is_streaming(&self, owner: &ResultSessionOwner) -> Result<bool, SessionError> {
        self.validate_active_run(owner)?;
        let session = self
            .sessions
            .get(&owner.result_session_id.0)
            .ok_or(SessionError::SessionNotFound)?;
        if session.owner != *owner {
            return Err(SessionError::OwnerMismatch);
        }
        Ok(session.lifecycle == ResultSessionLifecycle::Streaming)
    }

    pub fn lifecycle(&self, owner: &ResultSessionOwner) -> Result<SessionLifecycle, SessionError> {
        self.validate_active_run(owner)?;
        let session = self
            .sessions
            .get(&owner.result_session_id.0)
            .ok_or(SessionError::SessionNotFound)?;
        if session.owner != *owner {
            return Err(SessionError::OwnerMismatch);
        }
        Ok(session.lifecycle)
    }

    pub fn result_limit_reached(&self, owner: &ResultSessionOwner) -> Result<bool, SessionError> {
        self.validate_active_run(owner)?;
        let session = self
            .sessions
            .get(&owner.result_session_id.0)
            .ok_or(SessionError::SessionNotFound)?;
        if session.owner != *owner {
            return Err(SessionError::OwnerMismatch);
        }
        Ok(session.result_limit_reached)
    }

    pub fn value_too_large(&self, owner: &ResultSessionOwner) -> Result<bool, SessionError> {
        self.validate_active_run(owner)?;
        let session = self
            .sessions
            .get(&owner.result_session_id.0)
            .ok_or(SessionError::SessionNotFound)?;
        if session.owner != *owner {
            return Err(SessionError::OwnerMismatch);
        }
        Ok(session.value_too_large)
    }

    /// User-facing release keeps every ready cache page but makes continuation
    /// terminal. A new run or connection teardown performs the actual discard.
    pub fn release(&mut self, owner: &ResultSessionOwner) -> Result<(), SessionError> {
        self.validate_active_run(owner)?;
        let key = owner.result_session_id.0.as_str();
        let session = self
            .sessions
            .get_mut(key)
            .ok_or(SessionError::SessionNotFound)?;
        if session.owner != *owner {
            return Err(SessionError::OwnerMismatch);
        }
        if session.ready_pages == 0 {
            self.sessions.remove(key);
            self.sessions.shrink_to_fit();
            self.refresh_accounting();
            return Ok(());
        }
        session.lifecycle = ResultSessionLifecycle::Released;
        Ok(())
    }

    pub fn release_with_effect(
        &mut self,
        owner: &ResultSessionOwner,
        effect_outcome: EffectOutcome,
    ) -> Result<(), SessionError> {
        self.validate_active_run(owner)?;
        let session = self
            .sessions
            .get_mut(&owner.result_session_id.0)
            .ok_or(SessionError::SessionNotFound)?;
        if session.owner != *owner {
            return Err(SessionError::OwnerMismatch);
        }
        session.effect_outcome = effect_outcome;
        self.release(owner)
    }

    /// Failure cleanup before a session becomes a public result. Unlike user
    /// release this drops the partial cache and returns its process budget.
    pub fn discard(&mut self, owner: &ResultSessionOwner) -> Result<(), SessionError> {
        self.validate_active_run(owner)?;
        let key = owner.result_session_id.0.as_str();
        let session = self
            .sessions
            .get(key)
            .ok_or(SessionError::SessionNotFound)?;
        if session.owner != *owner {
            return Err(SessionError::OwnerMismatch);
        }
        let removed = self.sessions.remove(key).expect("session was validated");
        drop(removed);
        self.sessions.shrink_to_fit();
        self.refresh_accounting();
        Ok(())
    }

    pub fn release_connection(
        &mut self,
        identity: &ConnectionIdentity,
    ) -> Result<(), SessionError> {
        self.sessions.retain(|_, session| {
            let exact = session.owner.descriptor_id == identity.descriptor_id
                && session.owner.connection_id == identity.connection_id
                && session.owner.connection_generation == identity.connection_generation;
            !exact
        });
        self.sessions.shrink_to_fit();
        if self
            .active_runs
            .get(&identity.descriptor_id.0)
            .is_some_and(|active| {
                active.owner.connection_id == identity.connection_id
                    && active.owner.connection_generation == identity.connection_generation
            })
        {
            self.active_runs.remove(&identity.descriptor_id.0);
        }
        self.refresh_accounting();
        Ok(())
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    #[cfg(test)]
    pub(crate) fn session_bytes(&self, owner: &ResultSessionOwner) -> Result<usize, SessionError> {
        self.validate_active_run(owner)?;
        let session = self
            .sessions
            .get(&owner.result_session_id.0)
            .ok_or(SessionError::SessionNotFound)?;
        if session.owner != *owner {
            return Err(SessionError::OwnerMismatch);
        }
        Ok(session.bytes)
    }

    fn validate_active_run(&self, owner: &ResultSessionOwner) -> Result<(), SessionError> {
        match self.active_runs.get(&owner.descriptor_id.0) {
            Some(active)
                if active.owner.descriptor_id == owner.descriptor_id
                    && active.owner.connection_id == owner.connection_id
                    && active.owner.connection_generation == owner.connection_generation
                    && active.owner.query_run_id == owner.query_run_id =>
            {
                Ok(())
            }
            _ => Err(SessionError::OwnerMismatch),
        }
    }

    fn release_descriptor(&mut self, descriptor_id: &DescriptorId) {
        self.sessions
            .retain(|_, session| session.owner.descriptor_id != *descriptor_id);
        self.sessions.shrink_to_fit();
        self.refresh_accounting();
    }

    fn refresh_accounting(&mut self) {
        // Session storage is accounted at creation and on each row append.
        // Membership changes affect only the retained sessions and map capacity.
        self.total_bytes = estimate_registry_retained_bytes(&self.sessions);
    }
}

fn page_from_session(
    session: &StoredSession,
    page_index: usize,
) -> Result<ResultPage, SessionError> {
    if page_index >= session.ready_pages {
        return Err(SessionError::PageNotFound);
    }
    let rows = session
        .pages
        .get(page_index)
        .cloned()
        .ok_or(SessionError::PageNotFound)?;
    Ok(ResultPage {
        owner: session.owner.clone(),
        page_index,
        columns: session.columns.clone(),
        rows,
        has_previous: page_index > 0,
        has_next: session.lifecycle != ResultSessionLifecycle::Released
            && (page_index + 1 < session.ready_pages
                || (session.lifecycle == ResultSessionLifecycle::Streaming
                    && page_index + 1 < session.pages.len())),
        effect_outcome: session.effect_outcome,
        lifecycle: session.lifecycle,
        result_limit_reached: session.result_limit_reached,
        value_too_large: session.value_too_large,
    })
}

// A HashMap reserves more buckets than its public element capacity. Charging
// two full entry slots per reported capacity deliberately overstates both the
// current hashbrown load-factor slack and its one-byte control metadata.
const HASH_MAP_SLOT_MULTIPLIER: usize = 2;

fn estimate_registry_retained_bytes(sessions: &HashMap<String, StoredSession>) -> usize {
    let occupied = sessions
        .values()
        .fold(0usize, |total, session| total.saturating_add(session.bytes));
    let conservative_slots = sessions.capacity().saturating_mul(HASH_MAP_SLOT_MULTIPLIER);
    let spare_slots = conservative_slots.saturating_sub(sessions.len());
    occupied.saturating_add(spare_slots.saturating_mul(session_map_slot_bytes()))
}

fn estimate_session_retained_bytes(key: &String, session: &StoredSession) -> usize {
    session_map_slot_bytes()
        .saturating_add(key.capacity())
        .saturating_add(estimate_result_owner_heap_bytes(&session.owner))
        .saturating_add(estimate_columns_retained_bytes(
            &session.columns,
            session.columns.capacity(),
        ))
        .saturating_add(estimate_pages_retained_bytes(
            &session.pages,
            session.pages.capacity(),
        ))
}

fn session_map_slot_bytes() -> usize {
    size_of::<(String, StoredSession)>().saturating_add(size_of::<usize>())
}

fn estimate_result_owner_heap_bytes(owner: &ResultSessionOwner) -> usize {
    [
        &owner.descriptor_id.0,
        &owner.connection_id.0,
        &owner.connection_generation.0,
        &owner.query_run_id.0,
        &owner.statement_execution_id.0,
        &owner.result_session_id.0,
    ]
    .into_iter()
    .fold(0usize, |total, value| {
        total.saturating_add(value.capacity())
    })
}

fn estimate_pages_retained_bytes(pages: &[Vec<Vec<DbValue>>], pages_capacity: usize) -> usize {
    let page_slots = pages_capacity.saturating_mul(size_of::<Vec<Vec<DbValue>>>());
    pages.iter().fold(page_slots, |page_total, page| {
        let row_slots = page.capacity().saturating_mul(size_of::<Vec<DbValue>>());
        page.iter()
            .fold(page_total.saturating_add(row_slots), |row_total, row| {
                row_total.saturating_add(estimate_row_heap_bytes(row, row.capacity()))
            })
    })
}

fn estimate_row_heap_bytes(row: &[DbValue], row_capacity: usize) -> usize {
    let values = row_capacity.saturating_mul(size_of::<DbValue>());
    row.iter().fold(values, |total, value| {
        let string_capacity = match value {
            DbValue::Null | DbValue::Boolean { .. } => 0,
            DbValue::Integer { value }
            | DbValue::Decimal { value }
            | DbValue::Text { value }
            | DbValue::Json { value }
            | DbValue::Date { value }
            | DbValue::Time { value }
            | DbValue::DateTime { value } => value.capacity(),
            DbValue::Binary { hex } => hex.capacity(),
        };
        total.saturating_add(string_capacity)
    })
}

fn estimate_columns_retained_bytes(columns: &[String], columns_capacity: usize) -> usize {
    let slots = columns_capacity.saturating_mul(size_of::<String>());
    columns.iter().fold(slots, |total, column| {
        total.saturating_add(column.capacity())
    })
}

fn next_vec_capacity(current: usize) -> usize {
    match current {
        0 => 4,
        n => n.saturating_mul(2),
    }
}

fn map_insert_growth_bytes(len: usize, capacity: usize) -> usize {
    if len < capacity {
        0
    } else {
        let grown = next_vec_capacity(capacity);
        grown
            .saturating_sub(capacity)
            .saturating_mul(session_map_slot_bytes())
    }
}

fn estimate_session_after_push(
    key: &String,
    stored_key_capacity: usize,
    session: &StoredSession,
    row_bytes: usize,
) -> usize {
    // Keep the caller key's original capacity charge. Cached bytes include the
    // stored map key, which is a different allocation from either owner ID.
    let current = if session.bytes == usize::MAX {
        // Saturation lost information needed to replace just the key term.
        estimate_session_retained_bytes(key, session)
    } else {
        session
            .bytes
            .saturating_sub(stored_key_capacity)
            .saturating_add(key.capacity())
    };
    let last_len = session.pages.last().map(Vec::len).unwrap_or(0);
    let last_cap = session.pages.last().map(Vec::capacity).unwrap_or(0);
    let need_new_page = last_len == RESULT_PAGE_ROWS;
    let extra_page_slots = if need_new_page && session.pages.len() == session.pages.capacity() {
        next_vec_capacity(session.pages.capacity())
            .saturating_sub(session.pages.capacity())
            .saturating_mul(size_of::<Vec<Vec<DbValue>>>())
    } else {
        0
    };
    let page_cap_now = if need_new_page { 0 } else { last_cap };
    let page_len_now = if need_new_page { 0 } else { last_len };
    let extra_row_slots = if page_len_now == page_cap_now {
        next_vec_capacity(page_cap_now)
            .saturating_sub(page_cap_now)
            .saturating_mul(size_of::<Vec<DbValue>>())
    } else {
        0
    };
    current
        .saturating_add(extra_page_slots)
        .saturating_add(extra_row_slots)
        .saturating_add(row_bytes)
}

fn db_value_retained_bytes(value: &DbValue) -> usize {
    match value {
        DbValue::Null | DbValue::Boolean { .. } => 0,
        DbValue::Integer { value }
        | DbValue::Decimal { value }
        | DbValue::Text { value }
        | DbValue::Json { value }
        | DbValue::Date { value }
        | DbValue::Time { value }
        | DbValue::DateTime { value } => value.capacity(),
        DbValue::Binary { hex } => hex.capacity(),
    }
}

fn classify_converted_row(
    row: &Vec<DbValue>,
    field_limit: usize,
    row_limit: usize,
) -> Result<usize, ResultLimitKind> {
    let mut used = row.capacity().saturating_mul(size_of::<DbValue>());
    for value in row {
        let retained = db_value_retained_bytes(value);
        let raw = match value {
            DbValue::Binary { hex } => hex.len() / 2,
            _ => retained,
        };
        if raw > field_limit {
            return Err(ResultLimitKind::Field);
        }
        used = used.saturating_add(retained);
        if used > row_limit {
            return Err(ResultLimitKind::Row);
        }
    }
    Ok(used)
}

#[derive(Clone, Default)]
pub struct ResultSessionState(pub Arc<Mutex<ResultSessionRegistry>>);

impl ResultSessionState {
    pub fn with_limits(session_limit: usize, process_limit: usize) -> Self {
        Self(Arc::new(Mutex::new(ResultSessionRegistry::with_limits(
            session_limit,
            process_limit,
        ))))
    }

    pub fn with_ceilings(
        field_limit: usize,
        row_limit: usize,
        session_limit: usize,
        process_limit: usize,
    ) -> Self {
        Self(Arc::new(Mutex::new(ResultSessionRegistry::with_ceilings(
            field_limit,
            row_limit,
            session_limit,
            process_limit,
        ))))
    }

    pub fn lock(&self) -> Result<std::sync::MutexGuard<'_, ResultSessionRegistry>, SessionError> {
        self.0.lock().map_err(|_| SessionError::LockUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_service::{
        ConnectionGeneration, ConnectionId, ConnectionIdentity, DbValue, DescriptorId,
        EffectOutcome, QueryRunId, QueryRunOwner, ResultSessionId, ResultSessionOwner,
        StatementExecutionId,
    };

    fn run_owner(run: &str) -> QueryRunOwner {
        QueryRunOwner {
            descriptor_id: DescriptorId("descriptor-a".into()),
            connection_id: ConnectionId("connection-a".into()),
            connection_generation: ConnectionGeneration("generation-a".into()),
            query_run_id: QueryRunId(run.into()),
        }
    }

    fn owner(run: &str, statement: &str, session: &str) -> ResultSessionOwner {
        ResultSessionOwner {
            descriptor_id: DescriptorId("descriptor-a".into()),
            connection_id: ConnectionId("connection-a".into()),
            connection_generation: ConnectionGeneration("generation-a".into()),
            query_run_id: QueryRunId(run.into()),
            statement_execution_id: StatementExecutionId(statement.into()),
            result_session_id: ResultSessionId(session.into()),
        }
    }

    #[test]
    fn validated_row_bytes_preserve_capacities_and_first_limit_failure() {
        fn spare(value: &str, capacity: usize) -> String {
            let mut text = String::with_capacity(capacity);
            text.push_str(value);
            text
        }

        let mut row = Vec::with_capacity(17);
        row.extend([
            DbValue::Null,
            DbValue::Boolean { value: true },
            DbValue::Integer {
                value: spare("1", 64),
            },
            DbValue::Decimal {
                value: spare("1.25", 64),
            },
            DbValue::Text {
                value: spare("資料", 64),
            },
            DbValue::Json {
                value: spare("{}", 64),
            },
            DbValue::Date {
                value: spare("2026-10-05", 64),
            },
            DbValue::Time {
                value: spare("00:00:00", 64),
            },
            DbValue::DateTime {
                value: spare("2026-10-05T00:00:00Z", 64),
            },
            DbValue::Binary {
                hex: spare("00ff", 96),
            },
        ]);
        let expected = estimate_row_heap_bytes(&row, row.capacity());
        assert_eq!(
            classify_converted_row(&row, usize::MAX, expected),
            Ok(expected)
        );
        assert_eq!(
            classify_converted_row(&row, usize::MAX, expected - 1),
            Err(ResultLimitKind::Row)
        );

        let field_first = vec![DbValue::Text {
            value: spare("x", 64),
        }];
        assert_eq!(
            classify_converted_row(&field_first, 63, 0),
            Err(ResultLimitKind::Field)
        );
        let row_first = vec![
            DbValue::Null,
            DbValue::Text {
                value: spare("x", 64),
            },
        ];
        assert_eq!(
            classify_converted_row(&row_first, 0, 0),
            Err(ResultLimitKind::Row)
        );

        let binary = vec![DbValue::Binary {
            hex: spare("00ff", 96),
        }];
        let binary_bytes = estimate_row_heap_bytes(&binary, binary.capacity());
        assert_eq!(
            classify_converted_row(&binary, 2, binary_bytes),
            Ok(binary_bytes)
        );
        assert_eq!(
            classify_converted_row(&binary, 1, binary_bytes),
            Err(ResultLimitKind::Field)
        );

        // Preserve the existing empty-row classification; projection still charges its capacity.
        let empty = Vec::<DbValue>::with_capacity(4);
        assert_eq!(
            classify_converted_row(&empty, 0, 0),
            Ok(estimate_row_heap_bytes(&empty, empty.capacity()))
        );
    }

    #[test]
    fn validated_row_projection_preserves_saturated_cache_fallback() {
        let mut registry = ResultSessionRegistry::default();
        let mut caller = owner("saturated", "statement", "session");
        registry.begin_run(&run_owner("saturated")).unwrap();
        registry
            .begin_session(caller.clone(), vec!["value".into()])
            .unwrap();
        registry
            .push_row(
                &caller,
                vec![DbValue::Text {
                    value: "existing".into(),
                }],
            )
            .unwrap();
        {
            let session = registry
                .sessions
                .get_mut(&caller.result_session_id.0)
                .unwrap();
            session.pages.last_mut().unwrap().reserve(8);
            session.bytes = usize::MAX;
        }
        caller.result_session_id.0.reserve(4096);
        let mut row = Vec::with_capacity(5);
        row.push(DbValue::Text {
            value: "next".into(),
        });
        let bytes = classify_converted_row(&row, usize::MAX, usize::MAX).unwrap();
        let (stored_key, session) = registry
            .sessions
            .get_key_value(&caller.result_session_id.0)
            .unwrap();
        let full = estimate_session_retained_bytes(&caller.result_session_id.0, session);
        assert_eq!(
            estimate_session_after_push(
                &caller.result_session_id.0,
                stored_key.capacity(),
                session,
                bytes,
            ),
            full.saturating_add(estimate_row_heap_bytes(&row, row.capacity()))
        );
        assert_eq!(
            estimate_session_after_push(
                &caller.result_session_id.0,
                stored_key.capacity(),
                session,
                usize::MAX
            ),
            usize::MAX
        );
    }

    #[test]
    fn projection_preserves_caller_key_capacity_at_both_budget_boundaries() {
        let compact = owner("key-capacity", "statement", "session");
        let mut overallocated = compact.clone();
        overallocated.result_session_id.0.reserve(4096);
        assert_eq!(overallocated, compact);

        for stored_owner_spare in [0, 1024] {
            let make_registry = || {
                let mut registry = ResultSessionRegistry::default();
                registry.begin_run(&run_owner("key-capacity")).unwrap();
                let mut stored_owner = compact.clone();
                // The owner ID and HashMap key are separate allocations.
                stored_owner.result_session_id.0.reserve(stored_owner_spare);
                registry
                    .begin_session(stored_owner, vec!["value".into()])
                    .unwrap();
                registry
            };
            let mut probe = make_registry();
            assert_eq!(
                probe.push_row(&compact, vec![DbValue::Null]).unwrap(),
                PushRowOutcome::Stored
            );
            let exact_session_limit = probe.session_bytes(&compact).unwrap();
            let exact_process_limit = probe.total_bytes();
            for process_limit in [false, true] {
                for (caller, below_boundary, expected) in [
                    (&compact, 0, PushRowOutcome::Stored),
                    (&compact, 1, PushRowOutcome::LimitReached),
                    (&overallocated, 0, PushRowOutcome::LimitReached),
                ] {
                    let mut registry = make_registry();
                    // Isolate row projection from conservative map reservation.
                    if process_limit {
                        registry.process_limit = exact_process_limit - below_boundary;
                    } else {
                        registry.session_limit = exact_session_limit - below_boundary;
                    }
                    let before = registry.total_bytes();
                    assert_eq!(
                        registry.push_row(caller, vec![DbValue::Null]).unwrap(),
                        expected
                    );
                    if expected == PushRowOutcome::LimitReached {
                        assert_eq!(registry.total_bytes(), before);
                        assert!(registry.result_limit_reached(&compact).unwrap());
                    }
                    registry
                        .finish_session(&compact, EffectOutcome::None)
                        .unwrap();
                    assert_eq!(
                        registry.page(&compact, 0).unwrap().rows.len(),
                        usize::from(expected == PushRowOutcome::Stored)
                    );
                }
            }
        }
    }

    #[test]
    fn appended_rows_match_full_accounting_through_capacity_and_page_growth() {
        fn assert_full_accounting(registry: &ResultSessionRegistry) {
            for (key, session) in &registry.sessions {
                assert_eq!(session.bytes, estimate_session_retained_bytes(key, session));
            }
            assert_eq!(
                registry.total_bytes,
                estimate_registry_retained_bytes(&registry.sessions)
            );
        }

        let mut registry = ResultSessionRegistry::default();
        registry.begin_run(&run_owner("accounting")).unwrap();
        let background = owner("accounting", "background", "background-result");
        registry
            .begin_session(background.clone(), vec!["value".into()])
            .unwrap();
        for _ in 0..64 {
            registry
                .push_row(
                    &background,
                    vec![DbValue::Text {
                        value: "retained".into(),
                    }],
                )
                .unwrap();
        }
        registry
            .finish_session(&background, EffectOutcome::None)
            .unwrap();
        let mut target = owner("accounting", "target", "target-result");
        target.result_session_id.0.reserve(128);
        let mut columns = Vec::with_capacity(16);
        for name in ["text", "binary", "integer", "boolean", "null", "decimal"] {
            let mut column = String::with_capacity(48);
            column.push_str(name);
            columns.push(column);
        }
        registry.begin_session(target.clone(), columns).unwrap();
        for index in 0..1001 {
            let mut text = String::with_capacity(31 + index % 129);
            text.push_str("資料");
            let mut hex = String::with_capacity(17 + index % 5);
            hex.push_str("ab");
            let mut row = Vec::with_capacity(8 + index % 3);
            row.extend([
                DbValue::Text { value: text },
                DbValue::Binary { hex },
                DbValue::Integer {
                    value: index.to_string(),
                },
                DbValue::Boolean {
                    value: index % 2 == 0,
                },
                DbValue::Null,
                DbValue::Decimal {
                    value: "12345678901234567890.01".into(),
                },
            ]);
            assert_eq!(
                registry.push_row(&target, row).unwrap(),
                PushRowOutcome::Stored
            );
            assert_full_accounting(&registry);
        }
        registry
            .finish_session(&target, EffectOutcome::None)
            .unwrap();
        let retained = registry.total_bytes();
        registry.release(&target).unwrap();
        assert_eq!(registry.total_bytes(), retained);
        assert_full_accounting(&registry);
        registry.discard(&target).unwrap();
        assert_full_accounting(&registry);
        assert_eq!(registry.page(&background, 0).unwrap().rows.len(), 64);
        registry
            .release_connection(&ConnectionIdentity {
                descriptor_id: target.descriptor_id,
                connection_id: target.connection_id,
                connection_generation: target.connection_generation,
            })
            .unwrap();
        assert_eq!(registry.session_count(), 0);
        assert_eq!(registry.total_bytes(), 0);
        assert_full_accounting(&registry);
    }

    #[test]
    fn exact_owner_sessions_are_paged_and_a_new_descriptor_run_releases_the_old_run() {
        let mut registry = ResultSessionRegistry::with_limits(1 << 20, 2 << 20);
        let first = owner("run-a", "statement-a", "session-a");
        registry.begin_run(&run_owner("run-a")).unwrap();
        registry
            .begin_session(first.clone(), vec!["value".into()])
            .unwrap();
        for value in 0..501 {
            assert_eq!(
                registry.push_row(
                    &first,
                    vec![DbValue::Integer {
                        value: value.to_string(),
                    }],
                ),
                Ok(PushRowOutcome::Stored)
            );
        }
        let session = registry
            .finish_session(&first, EffectOutcome::None)
            .unwrap();
        assert_eq!(session.initial_page.rows.len(), 500);
        assert!(session.initial_page.has_next);
        assert_eq!(registry.page(&first, 1).unwrap().rows.len(), 1);

        registry.begin_run(&run_owner("run-b")).unwrap();
        assert_eq!(registry.page(&first, 0), Err(SessionError::OwnerMismatch));
        assert_eq!(registry.total_bytes(), 0);
    }

    #[test]
    fn materialized_boundaries_never_exceed_500_rows_or_create_a_blank_terminal_page() {
        for row_count in [0usize, 499, 500, 501, 1000, 1001, 1201] {
            let mut registry = ResultSessionRegistry::with_limits(1 << 20, 2 << 20);
            let run = format!("run-{row_count}");
            let result_owner = owner(&run, "statement-a", &format!("session-{row_count}"));
            registry.begin_run(&run_owner(&run)).unwrap();
            registry
                .begin_session(result_owner.clone(), vec!["value".into()])
                .unwrap();
            for value in 0..row_count {
                assert_eq!(
                    registry.push_row(
                        &result_owner,
                        vec![DbValue::Integer {
                            value: value.to_string(),
                        }],
                    ),
                    Ok(PushRowOutcome::Stored)
                );
            }
            registry
                .finish_session(&result_owner, EffectOutcome::None)
                .unwrap();

            let expected_pages = row_count.max(1).div_ceil(RESULT_PAGE_ROWS);
            for page_index in 0..expected_pages {
                let page = registry.page(&result_owner, page_index).unwrap();
                let expected_rows = if row_count == 0 {
                    0
                } else {
                    (row_count - page_index * RESULT_PAGE_ROWS).min(RESULT_PAGE_ROWS)
                };
                assert_eq!(page.rows.len(), expected_rows, "row_count={row_count}");
                assert!(page.rows.len() <= RESULT_PAGE_ROWS);
                assert_eq!(page.has_previous, page_index > 0);
                assert_eq!(page.has_next, page_index + 1 < expected_pages);
            }
            assert_eq!(
                registry.page(&result_owner, expected_pages),
                Err(SessionError::PageNotFound),
                "row_count={row_count} exposed a blank terminal page"
            );
        }
    }

    #[test]
    fn lookahead_proves_next_and_navigation_never_jumps_an_unread_page() {
        let mut registry = ResultSessionRegistry::with_limits(1 << 20, 2 << 20);
        let result_owner = owner("run-a", "statement-a", "session-a");
        registry.begin_run(&run_owner("run-a")).unwrap();
        registry
            .begin_session(result_owner.clone(), vec!["value".into()])
            .unwrap();

        for value in 0..501 {
            registry
                .push_row(
                    &result_owner,
                    vec![DbValue::Integer {
                        value: value.to_string(),
                    }],
                )
                .unwrap();
        }
        let first = registry.mark_page_ready(&result_owner, 0).unwrap();
        assert_eq!(first.rows.len(), 500);
        assert!(first.has_next, "only the 501st cached row proves Next");
        assert_eq!(
            registry.page(&result_owner, 1),
            Err(SessionError::PageNotFound),
            "the lookahead row is not yet a readable page"
        );
        assert_eq!(
            registry.next(&result_owner),
            Ok(NextPage::Continue { page_index: 1 })
        );
        assert_eq!(
            registry.previous(&result_owner),
            Err(SessionError::PageNotFound)
        );

        for value in 501..1001 {
            registry
                .push_row(
                    &result_owner,
                    vec![DbValue::Integer {
                        value: value.to_string(),
                    }],
                )
                .unwrap();
        }
        registry.mark_page_ready(&result_owner, 1).unwrap();
        let second = registry.complete_next(&result_owner, 1).unwrap();
        assert_eq!(second.rows.len(), 500);
        assert!(second.has_next);

        let previous = registry.previous(&result_owner).unwrap();
        assert_eq!(previous.page_index, 0);
        assert_eq!(registry.next(&result_owner), Ok(NextPage::Cached(second)));
        assert_eq!(
            registry.next(&result_owner),
            Ok(NextPage::Continue { page_index: 2 })
        );

        registry
            .finish_session(&result_owner, EffectOutcome::Committed)
            .unwrap();
        let terminal = registry.complete_next(&result_owner, 2).unwrap();
        assert_eq!(terminal.rows.len(), 1);
        assert!(!terminal.has_next);
        assert_eq!(terminal.effect_outcome, EffectOutcome::Committed);
    }

    #[test]
    fn session_and_process_budgets_reject_the_next_row_without_losing_cached_pages() {
        fn first_row() -> Vec<DbValue> {
            vec![DbValue::Text {
                value: "cached".into(),
            }]
        }

        fn candidate_row() -> Vec<DbValue> {
            vec![DbValue::Text {
                value: "x".repeat(1024),
            }]
        }

        let result_owner = owner("run-a", "statement-a", "session-a");
        let mut probe = ResultSessionRegistry::with_limits(usize::MAX, usize::MAX);
        probe.begin_run(&run_owner("run-a")).unwrap();
        probe
            .begin_session(result_owner.clone(), vec!["value".into()])
            .unwrap();
        assert_eq!(
            probe.push_row(&result_owner, first_row()),
            Ok(PushRowOutcome::Stored)
        );
        assert_eq!(
            probe.push_row(&result_owner, candidate_row()),
            Ok(PushRowOutcome::Stored)
        );
        let session_limit = probe.session_bytes(&result_owner).unwrap() - 1;

        let mut registry = ResultSessionRegistry::with_limits(session_limit, usize::MAX);
        registry.begin_run(&run_owner("run-a")).unwrap();
        registry
            .begin_session(result_owner.clone(), vec!["value".into()])
            .unwrap();
        assert_eq!(
            registry.push_row(&result_owner, first_row()),
            Ok(PushRowOutcome::Stored)
        );
        assert_eq!(
            registry.push_row(&result_owner, candidate_row()),
            Ok(PushRowOutcome::LimitReached)
        );
        let session = registry
            .finish_session(&result_owner, EffectOutcome::Unknown)
            .unwrap();
        assert_eq!(session.initial_page.rows.len(), 1);
        assert_eq!(session.initial_page.effect_outcome, EffectOutcome::Unknown);
        assert!(registry.result_limit_reached(&result_owner).unwrap());
        assert!(registry.session_bytes(&result_owner).unwrap() <= session_limit);
        assert_eq!(DEFAULT_FIELD_BYTES, 1024 * 1024);
        assert_eq!(DEFAULT_ROW_BYTES, 8 * 1024 * 1024);
        assert_eq!(DEFAULT_SESSION_BYTES, 64 * 1024 * 1024);
        assert_eq!(DEFAULT_PROCESS_BYTES, 256 * 1024 * 1024);
    }

    #[test]
    fn process_budget_rejection_keeps_other_session_pages_readable_and_in_bounds() {
        let first = owner("run-process", "statement-a", "session-a");
        let second = owner("run-process", "statement-b", "session-b");
        let cached = || vec![DbValue::Integer { value: "1".into() }];
        let candidate = || {
            vec![DbValue::Text {
                value: "x".repeat(4096),
            }]
        };

        let mut probe = ResultSessionRegistry::with_limits(usize::MAX, usize::MAX);
        probe.begin_run(&run_owner("run-process")).unwrap();
        probe
            .begin_session(first.clone(), vec!["value".into()])
            .unwrap();
        probe.push_row(&first, cached()).unwrap();
        probe.finish_session(&first, EffectOutcome::None).unwrap();
        probe
            .begin_session(second.clone(), vec!["value".into()])
            .unwrap();
        let before_candidate = probe.total_bytes();
        probe.push_row(&second, candidate()).unwrap();
        let process_limit = probe.total_bytes() - 1;
        assert!(before_candidate <= process_limit);

        let mut bounded = ResultSessionRegistry::with_limits(usize::MAX, process_limit);
        bounded.begin_run(&run_owner("run-process")).unwrap();
        bounded
            .begin_session(first.clone(), vec!["value".into()])
            .unwrap();
        bounded.push_row(&first, cached()).unwrap();
        bounded.finish_session(&first, EffectOutcome::None).unwrap();
        bounded
            .begin_session(second.clone(), vec!["value".into()])
            .unwrap();
        assert_eq!(
            bounded.push_row(&second, candidate()),
            Ok(PushRowOutcome::LimitReached)
        );
        assert!(bounded.total_bytes() <= process_limit);
        assert_eq!(bounded.page(&first, 0).unwrap().rows, vec![cached()]);
        assert!(bounded.result_limit_reached(&second).unwrap());
    }

    #[test]
    fn null_heavy_rows_charge_their_observable_vec_capacity() {
        let mut registry = ResultSessionRegistry::with_limits(usize::MAX, usize::MAX);
        let result_owner = owner("run-null", "statement-null", "session-null");
        registry.begin_run(&run_owner("run-null")).unwrap();
        registry
            .begin_session(result_owner.clone(), vec!["nullable".into()])
            .unwrap();

        let before = registry.session_bytes(&result_owner).unwrap();
        let mut row = Vec::with_capacity(128);
        row.resize(64, DbValue::Null);
        let observable_allocation = row
            .capacity()
            .saturating_mul(size_of::<DbValue>())
            .saturating_add(size_of::<Vec<DbValue>>());

        assert_eq!(
            registry.push_row(&result_owner, row),
            Ok(PushRowOutcome::Stored)
        );
        let charged = registry
            .session_bytes(&result_owner)
            .unwrap()
            .saturating_sub(before);
        assert!(
            charged >= observable_allocation,
            "NULL payloads still retain the row Vec allocation: charged={charged}, observable={observable_allocation}"
        );
    }

    #[test]
    fn columns_pages_and_session_owner_charge_fixed_and_capacity_allocations() {
        let mut column = String::with_capacity(512);
        column.push_str("value");
        let mut columns = Vec::with_capacity(16);
        columns.push(column);
        let observable_columns = columns
            .capacity()
            .saturating_mul(size_of::<String>())
            .saturating_add(columns[0].capacity());

        let result_owner = owner(
            "run-containers",
            "statement-containers",
            "session-containers",
        );
        let observable_owner = estimate_result_owner_heap_bytes(&result_owner);
        let minimum_page_container = size_of::<Vec<Vec<DbValue>>>();
        let minimum_charge = session_map_slot_bytes()
            .saturating_add(observable_owner)
            .saturating_add(observable_columns)
            .saturating_add(minimum_page_container);

        let mut registry = ResultSessionRegistry::with_limits(usize::MAX, usize::MAX);
        registry.begin_run(&run_owner("run-containers")).unwrap();
        registry
            .begin_session(result_owner.clone(), columns)
            .unwrap();

        assert!(
            registry.session_bytes(&result_owner).unwrap() >= minimum_charge,
            "columns Vec, column String, page Vec, map slot, and owner capacities must all be charged"
        );
    }

    #[test]
    fn spare_capacity_limit_rolls_back_allocation_and_preserves_cached_rows() {
        fn cached_row() -> Vec<DbValue> {
            vec![DbValue::Text {
                value: "cached".into(),
            }]
        }

        fn spare_row() -> (Vec<DbValue>, usize) {
            let mut value = String::with_capacity(4096);
            value.push('x');
            let string_capacity = value.capacity();
            let mut row = Vec::with_capacity(64);
            row.push(DbValue::Text { value });
            let observable = row
                .capacity()
                .saturating_mul(size_of::<DbValue>())
                .saturating_add(string_capacity);
            (row, observable)
        }

        let result_owner = owner("run-spare", "statement-spare", "session-spare");
        let mut probe = ResultSessionRegistry::with_limits(usize::MAX, usize::MAX);
        probe.begin_run(&run_owner("run-spare")).unwrap();
        probe
            .begin_session(result_owner.clone(), vec!["value".into()])
            .unwrap();
        assert_eq!(
            probe.push_row(&result_owner, cached_row()),
            Ok(PushRowOutcome::Stored)
        );
        let before_candidate = probe.session_bytes(&result_owner).unwrap();
        let (candidate, observable) = spare_row();
        assert_eq!(
            probe.push_row(&result_owner, candidate),
            Ok(PushRowOutcome::Stored)
        );
        let with_candidate = probe.session_bytes(&result_owner).unwrap();
        assert!(
            with_candidate.saturating_sub(before_candidate) >= observable,
            "String and row spare capacity must be charged"
        );

        let session_limit = with_candidate - 1;
        let mut bounded = ResultSessionRegistry::with_limits(session_limit, usize::MAX);
        bounded.begin_run(&run_owner("run-spare")).unwrap();
        bounded
            .begin_session(result_owner.clone(), vec!["value".into()])
            .unwrap();
        assert_eq!(
            bounded.push_row(&result_owner, cached_row()),
            Ok(PushRowOutcome::Stored)
        );
        let retained_before_rejection = bounded.session_bytes(&result_owner).unwrap();
        let (candidate, _) = spare_row();
        assert_eq!(
            bounded.push_row(&result_owner, candidate),
            Ok(PushRowOutcome::LimitReached)
        );
        assert!(bounded.session_bytes(&result_owner).unwrap() <= session_limit);
        assert!(
            bounded.session_bytes(&result_owner).unwrap() <= retained_before_rejection,
            "rollback must release any capacity allocated by the rejected mutation"
        );
        let session = bounded
            .finish_session(&result_owner, EffectOutcome::Unknown)
            .unwrap();
        assert_eq!(session.initial_page.rows, vec![cached_row()]);
        assert!(session.initial_page.result_limit_reached);
    }

    #[test]
    fn session_operations_reject_wrong_connection_and_generation_for_the_active_run() {
        let mut registry = ResultSessionRegistry::default();
        registry.begin_run(&run_owner("run-a")).unwrap();
        let mut wrong_connection = owner("run-a", "statement-a", "session-a");
        wrong_connection.connection_id = ConnectionId("connection-stale".into());
        assert_eq!(
            registry.begin_session(wrong_connection, vec!["value".into()]),
            Err(SessionError::OwnerMismatch)
        );
        let mut wrong_generation = owner("run-a", "statement-a", "session-b");
        wrong_generation.connection_generation = ConnectionGeneration("generation-stale".into());
        assert_eq!(
            registry.begin_session(wrong_generation, vec!["value".into()]),
            Err(SessionError::OwnerMismatch)
        );
    }

    #[test]
    fn user_release_preserves_cache_and_effect_but_disables_next_until_drop() {
        let mut registry = ResultSessionRegistry::default();
        registry.begin_run(&run_owner("run-a")).unwrap();
        let first = owner("run-a", "statement-a", "session-a");
        let second = owner("run-a", "statement-b", "session-b");
        for session in [&first, &second] {
            registry
                .begin_session(session.clone(), vec!["value".into()])
                .unwrap();
            registry
                .push_row(
                    session,
                    vec![DbValue::Text {
                        value: "cached".into(),
                    }],
                )
                .unwrap();
        }
        registry
            .finish_session(&first, EffectOutcome::Committed)
            .unwrap();
        registry
            .finish_session(&second, EffectOutcome::None)
            .unwrap();
        let before = registry.total_bytes();
        registry.release(&first).unwrap();
        assert_eq!(registry.total_bytes(), before);
        assert_eq!(registry.lifecycle(&first), Ok(SessionLifecycle::Released));
        let retained = registry.page(&first, 0).unwrap();
        assert_eq!(retained.rows.len(), 1);
        assert_eq!(retained.effect_outcome, EffectOutcome::Committed);
        assert!(!retained.has_next);
        assert_eq!(registry.next(&first), Err(SessionError::PageNotFound));
        registry.release(&first).unwrap();
        assert_eq!(registry.total_bytes(), before);

        registry.discard(&first).unwrap();
        assert!(registry.total_bytes() < before);
        assert_eq!(registry.page(&first, 0), Err(SessionError::OwnerMismatch));

        let mut stale = second.clone();
        stale.connection_generation = ConnectionGeneration("generation-stale".into());
        assert_eq!(registry.release(&stale), Err(SessionError::OwnerMismatch));
        registry
            .release_connection(&ConnectionIdentity {
                descriptor_id: second.descriptor_id.clone(),
                connection_id: second.connection_id.clone(),
                connection_generation: second.connection_generation.clone(),
            })
            .unwrap();
        assert_eq!(registry.total_bytes(), 0);
        assert_eq!(registry.session_count(), 0);
    }

    #[test]
    fn unfinished_internal_release_drops_partial_cache_for_p6_abort_compatibility() {
        let mut registry = ResultSessionRegistry::default();
        let result_owner = owner("run-a", "statement-a", "session-a");
        registry.begin_run(&run_owner("run-a")).unwrap();
        registry
            .begin_session(result_owner.clone(), vec!["value".into()])
            .unwrap();
        registry
            .push_row(
                &result_owner,
                vec![DbValue::Text {
                    value: "partial".into(),
                }],
            )
            .unwrap();
        assert!(registry.total_bytes() > 0);

        registry.release(&result_owner).unwrap();

        assert_eq!(registry.total_bytes(), 0);
        assert_eq!(registry.session_count(), 0);
        assert_eq!(
            registry.page(&result_owner, 0),
            Err(SessionError::OwnerMismatch)
        );
    }

    #[test]
    fn column_metadata_is_budgeted_before_a_session_is_inserted() {
        let mut registry = ResultSessionRegistry::with_limits(32, 64);
        registry.begin_run(&run_owner("run-a")).unwrap();
        assert_eq!(
            registry.begin_session(
                owner("run-a", "statement-a", "session-a"),
                vec!["a_column_alias_that_exceeds_the_entire_session_budget".into()],
            ),
            Err(SessionError::BudgetExceeded)
        );
        assert_eq!(registry.session_count(), 0);
        assert_eq!(registry.total_bytes(), 0);
    }

    #[test]
    fn one_value_over_the_field_ceiling_is_rejected_before_retention() {
        let mut registry = ResultSessionRegistry::with_ceilings(8, 1024, 1 << 20, 2 << 20);
        let result_owner = owner("run-field", "statement-field", "session-field");
        registry.begin_run(&run_owner("run-field")).unwrap();
        registry
            .begin_session(result_owner.clone(), vec!["value".into()])
            .unwrap();
        let before = registry.session_bytes(&result_owner).unwrap();
        assert_eq!(
            registry.classify_raw_field(9, 0, &result_owner).unwrap(),
            Some(ResultLimitKind::Field)
        );
        assert_eq!(
            registry.push_row(
                &result_owner,
                vec![DbValue::Text {
                    value: "012345678".into(),
                }],
            ),
            Ok(PushRowOutcome::ValueTooLarge)
        );
        assert_eq!(registry.session_bytes(&result_owner).unwrap(), before);
        assert!(registry.value_too_large(&result_owner).unwrap());
        let session = registry
            .finish_session(&result_owner, EffectOutcome::Unknown)
            .unwrap();
        assert!(session.initial_page.rows.is_empty());
        assert!(session.initial_page.value_too_large);
    }

    #[test]
    fn cumulative_row_ceiling_rejects_before_the_next_column_is_retained() {
        let mut registry = ResultSessionRegistry::with_ceilings(64, 16, 1 << 20, 2 << 20);
        let result_owner = owner("run-row", "statement-row", "session-row");
        registry.begin_run(&run_owner("run-row")).unwrap();
        registry
            .begin_session(result_owner.clone(), vec!["a".into(), "b".into()])
            .unwrap();
        assert_eq!(
            registry.classify_raw_field(8, 0, &result_owner).unwrap(),
            None
        );
        assert_eq!(
            registry.classify_raw_field(8, 12, &result_owner).unwrap(),
            Some(ResultLimitKind::Row)
        );
        assert_eq!(
            registry.push_row(
                &result_owner,
                vec![
                    DbValue::Text {
                        value: "abcdefgh".into(),
                    },
                    DbValue::Text {
                        value: "ijklmnop".into(),
                    },
                ],
            ),
            Ok(PushRowOutcome::ValueTooLarge)
        );
        assert!(registry.value_too_large(&result_owner).unwrap());
        assert_eq!(
            registry
                .finish_session(&result_owner, EffectOutcome::Unknown)
                .unwrap()
                .initial_page
                .rows
                .len(),
            0
        );
    }

    #[test]
    fn session_and_process_reservation_is_atomic_across_concurrent_pushers() {
        let first = owner("run-race", "statement-a", "session-a");
        let second = owner("run-race", "statement-b", "session-b");
        let candidate = || {
            vec![DbValue::Text {
                value: "x".repeat(2048),
            }]
        };

        let mut probe = ResultSessionRegistry::with_limits(usize::MAX, usize::MAX);
        probe.begin_run(&run_owner("run-race")).unwrap();
        probe
            .begin_session(first.clone(), vec!["value".into()])
            .unwrap();
        probe
            .begin_session(second.clone(), vec!["value".into()])
            .unwrap();
        let before = probe.total_bytes();
        probe.push_row(&first, candidate()).unwrap();
        let one_row = probe.total_bytes().saturating_sub(before);
        assert!(one_row > 0);
        let process_limit = before + one_row + one_row / 2;

        let state = ResultSessionState::with_limits(usize::MAX, process_limit);
        {
            let mut registry = state.lock().unwrap();
            registry.begin_run(&run_owner("run-race")).unwrap();
            registry
                .begin_session(first.clone(), vec!["value".into()])
                .unwrap();
            registry
                .begin_session(second.clone(), vec!["value".into()])
                .unwrap();
        }

        let left = state.clone();
        let right = state.clone();
        let left_owner = first.clone();
        let right_owner = second.clone();
        let left_thread = std::thread::spawn(move || {
            left.lock()
                .unwrap()
                .push_row(&left_owner, candidate())
                .unwrap()
        });
        let right_thread = std::thread::spawn(move || {
            right
                .lock()
                .unwrap()
                .push_row(&right_owner, candidate())
                .unwrap()
        });
        let outcomes = [left_thread.join().unwrap(), right_thread.join().unwrap()];
        let stored = outcomes
            .iter()
            .filter(|outcome| **outcome == PushRowOutcome::Stored)
            .count();
        let rejected = outcomes
            .iter()
            .filter(|outcome| **outcome == PushRowOutcome::LimitReached)
            .count();
        assert_eq!(stored, 1, "exactly one reservation must win: {outcomes:?}");
        assert_eq!(rejected, 1, "the loser must not insert: {outcomes:?}");
        {
            let registry = state.lock().unwrap();
            assert!(registry.total_bytes() <= process_limit);
        }
        let mut registry = state.lock().unwrap();
        registry
            .finish_session(&first, EffectOutcome::Unknown)
            .unwrap();
        registry
            .finish_session(&second, EffectOutcome::Unknown)
            .unwrap();
        let accepted = registry.page(&first, 0).unwrap().rows.len()
            + registry.page(&second, 0).unwrap().rows.len();
        assert_eq!(accepted, 1);
        assert!(registry.total_bytes() <= process_limit);
    }
}

#[cfg(test)]
mod membership_accounting_tests {
    use super::*;
    use crate::db_service::{
        ConnectionGeneration, ConnectionId, QueryRunId, ResultSessionId, StatementExecutionId,
    };

    fn owners(
        descriptor: &str,
        run_id: &str,
        session_id: &str,
    ) -> (QueryRunOwner, ResultSessionOwner) {
        let run = QueryRunOwner {
            descriptor_id: DescriptorId(descriptor.into()),
            connection_id: ConnectionId(format!("connection-{descriptor}")),
            connection_generation: ConnectionGeneration("generation".into()),
            query_run_id: QueryRunId(run_id.into()),
        };
        let owner = ResultSessionOwner {
            descriptor_id: run.descriptor_id.clone(),
            connection_id: run.connection_id.clone(),
            connection_generation: run.connection_generation.clone(),
            query_run_id: run.query_run_id.clone(),
            statement_execution_id: StatementExecutionId("statement".into()),
            result_session_id: ResultSessionId(session_id.into()),
        };
        (run, owner)
    }

    fn identity(owner: &ResultSessionOwner) -> ConnectionIdentity {
        ConnectionIdentity {
            descriptor_id: owner.descriptor_id.clone(),
            connection_id: owner.connection_id.clone(),
            connection_generation: owner.connection_generation.clone(),
        }
    }

    fn assert_full_model(registry: &ResultSessionRegistry) {
        let mut occupied = 0usize;
        for (key, session) in &registry.sessions {
            assert_eq!(session.map_key_capacity, key.capacity());
            let full = estimate_session_retained_bytes(key, session);
            assert_eq!(session.bytes, full);
            occupied = occupied.saturating_add(full);
        }
        let spare_slots = registry
            .sessions
            .capacity()
            .saturating_mul(HASH_MAP_SLOT_MULTIPLIER)
            .saturating_sub(registry.sessions.len());
        assert_eq!(
            registry.total_bytes(),
            occupied.saturating_add(spare_slots.saturating_mul(session_map_slot_bytes()))
        );
    }

    #[test]
    fn session_membership_matches_full_model_through_map_growth_and_retirement() {
        let mut registry = ResultSessionRegistry::default();
        let mut all = Vec::new();
        for index in 0..24 {
            let (run, owner) = owners(
                &format!("descriptor-{index}"),
                "run",
                &format!("session-{index}"),
            );
            registry.begin_run(&run).unwrap();
            let mut stored_owner = owner.clone();
            stored_owner.result_session_id.0.reserve(128 + index);
            let mut columns = Vec::with_capacity(8 + index);
            let mut column = String::with_capacity(64 + index);
            column.push_str("value");
            columns.push(column);
            registry.begin_session(stored_owner, columns).unwrap();
            assert_full_model(&registry);
            for row in 0..[0, 1, 4, 8, 500, 501][index % 6] {
                let mut value = String::with_capacity(40 + row % 13);
                value.push_str("cached value");
                let mut values = Vec::with_capacity(4 + row % 3);
                values.push(DbValue::Text { value });
                assert_eq!(
                    registry.push_row(&owner, values),
                    Ok(PushRowOutcome::Stored)
                );
            }
            if index % 4 == 1 {
                registry
                    .finish_session(&owner, EffectOutcome::None)
                    .unwrap();
            }
            assert_full_model(&registry);
            all.push(owner);
        }
        for (index, owner) in all.iter().enumerate() {
            match index % 4 {
                0 => registry.release(owner).unwrap(),
                1 => {
                    let original = registry.page(owner, 0).unwrap().rows;
                    let before = registry.total_bytes();
                    registry.release(owner).unwrap();
                    assert_eq!(registry.total_bytes(), before);
                    assert_eq!(registry.page(owner, 0).unwrap().rows, original);
                    registry.discard(owner).unwrap();
                }
                2 => registry.discard(owner).unwrap(),
                _ => registry.release_connection(&identity(owner)).unwrap(),
            }
            assert_full_model(&registry);
            registry.release_connection(&identity(owner)).unwrap();
            assert_full_model(&registry);
        }
        assert_eq!(registry.session_count(), 0);
        assert_eq!(registry.total_bytes(), 0);
        assert!(registry.active_runs.is_empty());
    }

    #[test]
    fn repeated_new_runs_keep_other_connection_pages_and_accounting_intact() {
        let mut registry = ResultSessionRegistry::default();
        let (stable_run, stable) = owners("stable", "stable-run", "stable-session");
        registry.begin_run(&stable_run).unwrap();
        registry
            .begin_session(stable.clone(), vec!["value".into()])
            .unwrap();
        for _ in 0..501 {
            registry
                .push_row(&stable, vec![DbValue::Integer { value: "42".into() }])
                .unwrap();
        }
        registry
            .finish_session(&stable, EffectOutcome::None)
            .unwrap();
        let stable_bytes = registry.session_bytes(&stable).unwrap();
        let stable_page = registry.page(&stable, 1).unwrap().rows;
        let mut previous = None;
        for index in 0..100 {
            let (run, current) = owners("changing", &format!("run-{index}"), "changing-session");
            registry.begin_run(&run).unwrap();
            if let Some(old) = &previous {
                assert_eq!(registry.page(old, 0), Err(SessionError::OwnerMismatch));
            }
            assert_full_model(&registry);
            registry
                .begin_session(current.clone(), vec!["value".repeat(index + 1)])
                .unwrap();
            registry.push_row(&current, vec![DbValue::Null]).unwrap();
            registry
                .finish_session(&current, EffectOutcome::Unknown)
                .unwrap();
            if index % 2 == 0 {
                registry.release(&current).unwrap();
            }
            assert_full_model(&registry);
            assert_eq!(registry.session_bytes(&stable).unwrap(), stable_bytes);
            assert_eq!(registry.page(&stable, 1).unwrap().rows, stable_page);
            previous = Some(current);
        }
        registry
            .release_connection(&identity(&previous.unwrap()))
            .unwrap();
        assert_full_model(&registry);
        registry.release_connection(&identity(&stable)).unwrap();
        assert_eq!(registry.total_bytes(), 0);
        assert!(registry.active_runs.is_empty());
    }
}
