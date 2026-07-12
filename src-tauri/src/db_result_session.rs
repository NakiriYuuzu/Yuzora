//! Bounded, exact-owner materialized result sessions.

use std::collections::HashMap;
use std::mem::size_of;
use std::sync::{Arc, Mutex};

use crate::db_service::{
    ConnectionIdentity, DbValue, DescriptorId, EffectOutcome, QueryRunOwner, ResultPage,
    ResultSession, ResultSessionLifecycle, ResultSessionOwner,
};

pub const RESULT_PAGE_ROWS: usize = 500;
pub const DEFAULT_SESSION_BYTES: usize = 64 * 1024 * 1024;
pub const DEFAULT_PROCESS_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushRowOutcome {
    Stored,
    LimitReached,
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
    columns: Vec<String>,
    pages: Vec<Vec<Vec<DbValue>>>,
    ready_pages: usize,
    current_page: usize,
    bytes: usize,
    effect_outcome: EffectOutcome,
    lifecycle: SessionLifecycle,
    result_limit_reached: bool,
}

pub struct ResultSessionRegistry {
    sessions: HashMap<String, StoredSession>,
    active_runs: HashMap<String, ActiveRun>,
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
        Self {
            sessions: HashMap::new(),
            active_runs: HashMap::new(),
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
        self.sessions.insert(
            key.clone(),
            StoredSession {
                owner,
                columns,
                pages: vec![Vec::new()],
                ready_pages: 0,
                current_page: 0,
                bytes: 0,
                effect_outcome: EffectOutcome::Unknown,
                lifecycle: ResultSessionLifecycle::Streaming,
                result_limit_reached: false,
            },
        );
        self.refresh_accounting();
        let session_bytes = self
            .sessions
            .get(&key)
            .map(|session| session.bytes)
            .unwrap_or(usize::MAX);
        if session_bytes > self.session_limit || self.total_bytes > self.process_limit {
            self.sessions.remove(&key);
            self.sessions.shrink_to_fit();
            self.refresh_accounting();
            return Err(SessionError::BudgetExceeded);
        }
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
        if session.result_limit_reached {
            return Ok(PushRowOutcome::LimitReached);
        }
        if session
            .pages
            .last()
            .is_some_and(|page| page.len() == RESULT_PAGE_ROWS)
        {
            session.pages.push(Vec::new());
        }
        session
            .pages
            .last_mut()
            .expect("a materialized session always has one page")
            .push(row);
        self.refresh_accounting();
        let exceeds_limit = self
            .sessions
            .get(key)
            .is_none_or(|session| session.bytes > self.session_limit)
            || self.total_bytes > self.process_limit;
        if exceeds_limit {
            let session = self
                .sessions
                .get_mut(key)
                .expect("the exact session was validated before insertion");
            let page = session
                .pages
                .last_mut()
                .expect("a materialized session always has one page");
            drop(page.pop());
            page.shrink_to_fit();
            if page.is_empty() && session.pages.len() > 1 {
                session.pages.pop();
            }
            session.pages.shrink_to_fit();
            session.result_limit_reached = true;
            self.refresh_accounting();
            debug_assert!(self
                .sessions
                .get(key)
                .is_some_and(|session| session.bytes <= self.session_limit));
            debug_assert!(self.total_bytes <= self.process_limit);
            return Ok(PushRowOutcome::LimitReached);
        }
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
        self.sessions.retain(|_, session| {
            if session.owner.descriptor_id == *descriptor_id {
                false
            } else {
                true
            }
        });
        self.sessions.shrink_to_fit();
        self.refresh_accounting();
    }

    fn refresh_accounting(&mut self) {
        for (key, session) in &mut self.sessions {
            session.bytes = estimate_session_retained_bytes(key, session);
        }
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

#[derive(Clone, Default)]
pub struct ResultSessionState(pub Arc<Mutex<ResultSessionRegistry>>);

impl ResultSessionState {
    pub fn with_limits(session_limit: usize, process_limit: usize) -> Self {
        Self(Arc::new(Mutex::new(ResultSessionRegistry::with_limits(
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
}
