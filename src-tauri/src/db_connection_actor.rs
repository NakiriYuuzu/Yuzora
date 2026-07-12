//! Exact-owner execution lease and locked-driver ownership proofs.
//!
//! The lease core intentionally does not store a generic `Client + Stream`.
//! `rusqlite::Rows` and `tiberius::QueryStream` borrow their statement/client,
//! so claiming that arbitrary owned generic types prove those engines would be
//! false. Engine workers instead own their client and keep borrowed cursors in
//! a lexical execution scope beside this lease. PostgreSQL's `RowStream` is
//! owned and can be stored directly. The compile proofs below use the real
//! locked driver types for all three shapes.

use std::sync::Mutex;

use crate::db_service::{ConnectionIdentity, DbHandle, QueryRunOwner, ResultSessionOwner};

fn owner_belongs_to(owner: &QueryRunOwner, connection: &ConnectionIdentity) -> bool {
    owner.descriptor_id == connection.descriptor_id
        && owner.connection_id == connection.connection_id
        && owner.connection_generation == connection.connection_generation
}

fn result_owner_belongs_to_run(owner: &ResultSessionOwner, run: &QueryRunOwner) -> bool {
    owner.descriptor_id == run.descriptor_id
        && owner.connection_id == run.connection_id
        && owner.connection_generation == run.connection_generation
        && owner.query_run_id == run.query_run_id
}

fn run_owner_for_result(owner: &ResultSessionOwner) -> QueryRunOwner {
    QueryRunOwner {
        descriptor_id: owner.descriptor_id.clone(),
        connection_id: owner.connection_id.clone(),
        connection_generation: owner.connection_generation.clone(),
        query_run_id: owner.query_run_id.clone(),
    }
}

/// Unforgeable-by-callers token for one occupation of the execution lease.
///
/// `execution_id` prevents a late settlement of a previous execution from
/// releasing a newer execution that happens to reuse the same query-run ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionLease {
    owner: QueryRunOwner,
    execution_id: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetadataLease {
    operation_id: u64,
}

impl ExecutionLease {
    pub fn owner(&self) -> &QueryRunOwner {
        &self.owner
    }

    pub fn execution_id(&self) -> u64 {
        self.execution_id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelCapability {
    SqliteInterrupt,
    PostgresProtocolCancel,
    /// tiberius 0.12.3 exposes no public TDS ATTENTION API. Explicit Cancel
    /// therefore ends this live connection and requires a reconnect.
    MssqlConnectionTermination,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriverCancelPrimitive {
    SqliteInterrupt,
    PostgresCancelToken,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelRequest {
    DriverCancellationRequired(DriverCancelPrimitive),
    ConnectionTerminationRequired,
    AlreadyRequested,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseRequest {
    Requested,
    AlreadyRequested,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultContinuationOutcome {
    PageReady,
    End,
    LimitReached,
    Released,
    Cancelled,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResultContinuationAck {
    pub outcome: ResultContinuationOutcome,
}

pub enum ResultContinuationCommand {
    Next {
        respond_to: tokio::sync::oneshot::Sender<ResultContinuationAck>,
    },
    Release {
        respond_to: tokio::sync::oneshot::Sender<ResultContinuationAck>,
    },
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorError {
    OwnerMismatch,
    ConnectionBusy,
    NoActiveExecution,
    StaleLease,
    ExecutionIdExhausted,
    CancelFailed,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settlement {
    pub cancel_requested: bool,
    pub release_requested: bool,
    pub connection_termination_required: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TeardownReport {
    pub unreleased_execution: bool,
    pub metadata_in_flight: bool,
    pub unreleased_result_sessions: usize,
    pub closed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleDriverAction {
    None,
    SqliteInterrupt,
    /// Lifecycle mutation terminates the old PostgreSQL transport. This is not
    /// the P6 user-facing protocol Cancel adapter and is never reported as one.
    PostgresTransportTermination,
    /// tiberius has no public TDS ATTENTION primitive. The actor stays
    /// non-reusable until the borrowed execution settles and the client drops.
    MssqlConnectionTermination,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleTeardownRequest {
    pub exact_owner: Option<QueryRunOwner>,
    pub cancel_requested: bool,
    pub release_requested: bool,
    pub connection_termination_required: bool,
    pub busy: bool,
    pub driver_action: LifecycleDriverAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LifecycleRequestState {
    exact_owner: Option<QueryRunOwner>,
    cancel_requested: bool,
    release_requested: bool,
    connection_termination_required: bool,
    busy: bool,
}

struct ActiveExecution {
    lease: ExecutionLease,
    cancel_capability: CancelCapability,
    cancel_requested: bool,
    release_requested: bool,
    connection_termination_required: bool,
}

struct ActiveResultContinuation {
    owner: ResultSessionOwner,
    sender: tokio::sync::mpsc::UnboundedSender<ResultContinuationCommand>,
}

/// Per-live-connection ownership gate shared by the engine worker.
///
/// The worker owns the actual driver client, cursor/future, and cancellation
/// resource. This core owns the canonical generation-bound lease and refuses a
/// second execution until the worker explicitly reports settlement/close.
pub struct ConnectionActor {
    identity: ConnectionIdentity,
    active: Option<ActiveExecution>,
    pending_cancel_dispatch: Option<QueryRunOwner>,
    metadata: Option<MetadataLease>,
    next_execution_id: u64,
    closed: bool,
}

impl ConnectionActor {
    pub fn new(identity: ConnectionIdentity) -> Self {
        Self {
            identity,
            active: None,
            pending_cancel_dispatch: None,
            metadata: None,
            next_execution_id: 1,
            closed: false,
        }
    }

    pub fn identity(&self) -> &ConnectionIdentity {
        &self.identity
    }

    pub fn is_busy(&self) -> bool {
        self.active.is_some() || self.pending_cancel_dispatch.is_some() || self.metadata.is_some()
    }

    pub fn active_owner(&self) -> Option<&QueryRunOwner> {
        self.active.as_ref().map(|active| active.lease.owner())
    }

    pub fn cancel_requested(&self, lease: &ExecutionLease) -> Result<bool, ActorError> {
        let active = self.validate_execution_lease(lease)?;
        Ok(active.cancel_requested)
    }

    fn validate_execution_lease(
        &self,
        lease: &ExecutionLease,
    ) -> Result<&ActiveExecution, ActorError> {
        let active = self.active.as_ref().ok_or(ActorError::NoActiveExecution)?;
        if active.lease != *lease {
            return Err(ActorError::StaleLease);
        }
        Ok(active)
    }

    fn validate_result_owner(
        &self,
        owner: &ResultSessionOwner,
    ) -> Result<&ActiveExecution, ActorError> {
        let active = self.active.as_ref().ok_or(ActorError::NoActiveExecution)?;
        if !result_owner_belongs_to_run(owner, active.lease.owner()) {
            return Err(ActorError::OwnerMismatch);
        }
        Ok(active)
    }

    fn mark_connection_terminated(&mut self, lease: &ExecutionLease) -> Result<(), ActorError> {
        let active = self.active.as_mut().ok_or(ActorError::NoActiveExecution)?;
        if active.lease != *lease {
            return Err(ActorError::StaleLease);
        }
        active.connection_termination_required = true;
        self.closed = true;
        Ok(())
    }

    fn owner_is_busy(&self, owner: &QueryRunOwner) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.lease.owner == *owner)
            || self.pending_cancel_dispatch.as_ref() == Some(owner)
    }

    pub fn acquire_execution(
        &mut self,
        owner: QueryRunOwner,
        cancel_capability: CancelCapability,
    ) -> Result<ExecutionLease, ActorError> {
        if !owner_belongs_to(&owner, &self.identity) {
            return Err(ActorError::OwnerMismatch);
        }
        if self.closed {
            return Err(ActorError::Closed);
        }
        if self.is_busy() {
            return Err(ActorError::ConnectionBusy);
        }

        let execution_id = self.next_execution_id;
        self.next_execution_id = self
            .next_execution_id
            .checked_add(1)
            .ok_or(ActorError::ExecutionIdExhausted)?;
        let lease = ExecutionLease {
            owner,
            execution_id,
        };
        self.active = Some(ActiveExecution {
            lease: lease.clone(),
            cancel_capability,
            cancel_requested: false,
            release_requested: false,
            connection_termination_required: false,
        });
        Ok(lease)
    }

    /// Metadata shares the same physical engine connection and therefore the
    /// same actor gate. It never waits behind an execution/result lease.
    pub fn acquire_metadata(&mut self) -> Result<MetadataLease, ActorError> {
        if self.closed {
            return Err(ActorError::Closed);
        }
        if self.is_busy() {
            return Err(ActorError::ConnectionBusy);
        }
        let operation_id = self.next_execution_id;
        self.next_execution_id = self
            .next_execution_id
            .checked_add(1)
            .ok_or(ActorError::ExecutionIdExhausted)?;
        let lease = MetadataLease { operation_id };
        self.metadata = Some(lease.clone());
        Ok(lease)
    }

    pub fn settle_metadata(&mut self, lease: &MetadataLease) -> Result<(), ActorError> {
        match self.metadata.as_ref() {
            Some(active) if active == lease => {
                self.metadata = None;
                Ok(())
            }
            Some(_) => Err(ActorError::StaleLease),
            None => Err(ActorError::NoActiveExecution),
        }
    }

    pub fn teardown_report(&self) -> TeardownReport {
        TeardownReport {
            unreleased_execution: self.active.is_some() || self.pending_cancel_dispatch.is_some(),
            metadata_in_flight: self.metadata.is_some(),
            // Result sessions are introduced in P7. P3 reports this exact zero
            // instead of pretending an untracked cache was released.
            unreleased_result_sessions: 0,
            closed: self.closed,
        }
    }

    pub fn begin_teardown(&mut self) -> Result<TeardownReport, ActorError> {
        let report = self.teardown_report();
        if self.is_busy() {
            return Err(ActorError::ConnectionBusy);
        }
        self.closed = true;
        Ok(TeardownReport {
            closed: true,
            ..report
        })
    }

    /// Lifecycle mutations are exact-actor termination requests, not a
    /// user-facing Cancel success. They make the actor non-reusable immediately
    /// and mark the occupied execution for cancel + release before the caller
    /// waits for settlement/retries teardown.
    fn request_lifecycle_teardown(&mut self) -> LifecycleRequestState {
        self.closed = true;
        let mut exact_owner = None;
        let mut cancel_requested = false;
        let mut release_requested = false;
        let mut connection_termination_required = false;
        if let Some(active) = self.active.as_mut() {
            exact_owner = Some(active.lease.owner.clone());
            active.cancel_requested = true;
            active.release_requested = true;
            if matches!(
                active.cancel_capability,
                CancelCapability::PostgresProtocolCancel
                    | CancelCapability::MssqlConnectionTermination
            ) {
                active.connection_termination_required = true;
            }
            cancel_requested = true;
            release_requested = true;
            connection_termination_required = active.connection_termination_required;
        }
        LifecycleRequestState {
            exact_owner,
            cancel_requested,
            release_requested,
            connection_termination_required,
            busy: self.is_busy(),
        }
    }

    /// Validates the exact canonical owner before exposing a driver action.
    /// The lease remains occupied until `settle_execution` is called.
    pub fn request_cancel(&mut self, owner: &QueryRunOwner) -> Result<CancelRequest, ActorError> {
        if !owner_belongs_to(owner, &self.identity) {
            return Err(ActorError::OwnerMismatch);
        }
        if self.pending_cancel_dispatch.as_ref() == Some(owner) {
            return Ok(CancelRequest::AlreadyRequested);
        }
        let active = self.active.as_mut().ok_or(ActorError::NoActiveExecution)?;
        if active.lease.owner != *owner {
            return Err(ActorError::OwnerMismatch);
        }
        if active.cancel_requested {
            return Ok(CancelRequest::AlreadyRequested);
        }

        active.cancel_requested = true;
        self.pending_cancel_dispatch = Some(owner.clone());
        let request = match active.cancel_capability {
            CancelCapability::SqliteInterrupt => {
                CancelRequest::DriverCancellationRequired(DriverCancelPrimitive::SqliteInterrupt)
            }
            CancelCapability::PostgresProtocolCancel => CancelRequest::DriverCancellationRequired(
                DriverCancelPrimitive::PostgresCancelToken,
            ),
            CancelCapability::MssqlConnectionTermination => {
                active.connection_termination_required = true;
                CancelRequest::ConnectionTerminationRequired
            }
        };
        Ok(request)
    }

    /// Completes the driver-request half of cancellation. The query future may
    /// settle before this call, but the actor remains busy until both halves
    /// have completed, preventing a late Cancel A from targeting Run B.
    pub fn complete_cancel_dispatch(&mut self, owner: &QueryRunOwner) -> Result<(), ActorError> {
        if !owner_belongs_to(owner, &self.identity) {
            return Err(ActorError::OwnerMismatch);
        }
        match self.pending_cancel_dispatch.as_ref() {
            Some(pending) if pending == owner => {
                self.pending_cancel_dispatch = None;
                Ok(())
            }
            Some(_) => Err(ActorError::OwnerMismatch),
            None => Err(ActorError::NoActiveExecution),
        }
    }

    /// Rolls back only the dispatch-intent state when the driver primitive
    /// itself fails. The still-running query retains its lease and may be
    /// cancelled again; a query that already settled leaves the actor idle.
    pub fn fail_cancel_dispatch(&mut self, owner: &QueryRunOwner) -> Result<(), ActorError> {
        if !owner_belongs_to(owner, &self.identity) {
            return Err(ActorError::OwnerMismatch);
        }
        match self.pending_cancel_dispatch.as_ref() {
            Some(pending) if pending == owner => {
                self.pending_cancel_dispatch = None;
                if let Some(active) = self
                    .active
                    .as_mut()
                    .filter(|active| active.lease.owner == *owner)
                {
                    active.cancel_requested = false;
                    active.connection_termination_required = false;
                }
                Ok(())
            }
            Some(_) => Err(ActorError::OwnerMismatch),
            None => Err(ActorError::NoActiveExecution),
        }
    }

    /// Marks release intent without pretending that the engine cursor settled.
    pub fn request_release(
        &mut self,
        lease: &ExecutionLease,
    ) -> Result<ReleaseRequest, ActorError> {
        let active = self.active.as_mut().ok_or(ActorError::NoActiveExecution)?;
        if active.lease != *lease {
            return Err(ActorError::StaleLease);
        }
        if active.release_requested {
            return Ok(ReleaseRequest::AlreadyRequested);
        }
        active.release_requested = true;
        Ok(ReleaseRequest::Requested)
    }

    fn request_result_release(
        &mut self,
        owner: &ResultSessionOwner,
    ) -> Result<ReleaseRequest, ActorError> {
        let active = self.active.as_mut().ok_or(ActorError::NoActiveExecution)?;
        if !result_owner_belongs_to_run(owner, active.lease.owner()) {
            return Err(ActorError::OwnerMismatch);
        }
        if active.release_requested {
            return Ok(ReleaseRequest::AlreadyRequested);
        }
        active.release_requested = true;
        Ok(ReleaseRequest::Requested)
    }

    /// Called only after EOF/error, driver cancellation settlement, or (for
    /// MSSQL fallback) connection close. Only this releases the single lease.
    pub fn settle_execution(&mut self, lease: &ExecutionLease) -> Result<Settlement, ActorError> {
        let active = self.active.as_ref().ok_or(ActorError::NoActiveExecution)?;
        if active.lease != *lease {
            return Err(ActorError::StaleLease);
        }
        let active = self.active.take().expect("active execution was validated");
        Ok(Settlement {
            cancel_requested: active.cancel_requested,
            release_requested: active.release_requested,
            connection_termination_required: active.connection_termination_required,
        })
    }
}

/// Production owner of the exact live identity, driver handle, single actor
/// lease and teardown state. Callers may borrow the engine only after acquiring
/// one of this actor's non-queueing leases.
pub struct ProductionConnectionActor {
    identity: ConnectionIdentity,
    core: Mutex<ConnectionActor>,
    handle: DbHandle,
    sqlite_cancel: Option<SqliteCancelResource>,
    postgres_cancel: Option<PostgresCancelResource>,
    mssql_cancel: Mutex<
        Option<(
            QueryRunOwner,
            tokio::sync::mpsc::UnboundedSender<QueryRunOwner>,
        )>,
    >,
    result_continuation: Mutex<Option<ActiveResultContinuation>>,
    settlement_changed: tokio::sync::Notify,
}

impl ProductionConnectionActor {
    pub fn new(identity: ConnectionIdentity, handle: DbHandle) -> Self {
        let sqlite_cancel = match &handle {
            DbHandle::Sqlite(connection) => connection
                .lock()
                .ok()
                .map(|connection| SqliteCancelResource::from_connection(&connection)),
            DbHandle::Postgres(_) | DbHandle::Mssql(_) => None,
        };
        let postgres_cancel = match &handle {
            DbHandle::Postgres(connection) => Some(connection.cancel_resource().clone()),
            DbHandle::Sqlite(_) | DbHandle::Mssql(_) => None,
        };
        Self {
            core: Mutex::new(ConnectionActor::new(identity.clone())),
            identity,
            handle,
            sqlite_cancel,
            postgres_cancel,
            mssql_cancel: Mutex::new(None),
            result_continuation: Mutex::new(None),
            settlement_changed: tokio::sync::Notify::new(),
        }
    }

    pub fn identity(&self) -> &ConnectionIdentity {
        &self.identity
    }

    pub(crate) fn handle(&self) -> &DbHandle {
        &self.handle
    }

    pub fn acquire_metadata(&self) -> Result<MetadataLease, ActorError> {
        self.core
            .lock()
            .map_err(|_| ActorError::Closed)?
            .acquire_metadata()
    }

    pub fn settle_metadata(&self, lease: &MetadataLease) -> Result<(), ActorError> {
        let result = self
            .core
            .lock()
            .map_err(|_| ActorError::Closed)?
            .settle_metadata(lease);
        if result.is_ok() {
            self.settlement_changed.notify_waiters();
        }
        result
    }

    pub fn acquire_execution(
        &self,
        owner: QueryRunOwner,
        capability: CancelCapability,
    ) -> Result<ExecutionLease, ActorError> {
        self.core
            .lock()
            .map_err(|_| ActorError::Closed)?
            .acquire_execution(owner, capability)
    }

    /// Installs the command sender for one exact row-producing statement.
    /// The actor stores only the six-field owner and sender; the engine worker
    /// remains the sole owner of its cursor/stream and borrowed driver state.
    pub fn install_result_continuation(
        &self,
        lease: &ExecutionLease,
        owner: ResultSessionOwner,
        sender: tokio::sync::mpsc::UnboundedSender<ResultContinuationCommand>,
    ) -> Result<(), ActorError> {
        {
            let core = self.core.lock().map_err(|_| ActorError::Closed)?;
            let active = core.validate_execution_lease(lease)?;
            if !result_owner_belongs_to_run(&owner, active.lease.owner()) {
                return Err(ActorError::OwnerMismatch);
            }
        }
        let mut continuation = self
            .result_continuation
            .lock()
            .map_err(|_| ActorError::Closed)?;
        match continuation.as_ref() {
            Some(active) if active.owner != owner => Err(ActorError::OwnerMismatch),
            Some(_) => Err(ActorError::ConnectionBusy),
            None => {
                *continuation = Some(ActiveResultContinuation { owner, sender });
                Ok(())
            }
        }
    }

    fn result_continuation_sender(
        &self,
        owner: &ResultSessionOwner,
    ) -> Result<tokio::sync::mpsc::UnboundedSender<ResultContinuationCommand>, ActorError> {
        self.core
            .lock()
            .map_err(|_| ActorError::Closed)?
            .validate_result_owner(owner)?;
        let continuation = self
            .result_continuation
            .lock()
            .map_err(|_| ActorError::Closed)?;
        match continuation.as_ref() {
            Some(active) if active.owner == *owner => Ok(active.sender.clone()),
            Some(_) => Err(ActorError::OwnerMismatch),
            None => Err(ActorError::NoActiveExecution),
        }
    }

    fn clear_result_continuation(&self, owner: Option<&ResultSessionOwner>) {
        if let Ok(mut continuation) = self.result_continuation.lock() {
            let should_clear = match (continuation.as_ref(), owner) {
                (Some(active), Some(owner)) => active.owner == *owner,
                (Some(_), None) => true,
                (None, _) => false,
            };
            if should_clear {
                *continuation = None;
            }
        }
        self.settlement_changed.notify_waiters();
    }

    fn clear_result_continuation_for_run(&self, owner: &QueryRunOwner) {
        if let Ok(mut continuation) = self.result_continuation.lock() {
            if continuation
                .as_ref()
                .is_some_and(|active| result_owner_belongs_to_run(&active.owner, owner))
            {
                *continuation = None;
            }
        }
    }

    fn signal_result_cancel(&self, owner: &QueryRunOwner) {
        if let Ok(continuation) = self.result_continuation.lock() {
            if let Some(active) = continuation
                .as_ref()
                .filter(|active| result_owner_belongs_to_run(&active.owner, owner))
            {
                let _ = active.sender.send(ResultContinuationCommand::Cancel);
            }
        }
    }

    async fn dispatch_result_continuation(
        &self,
        owner: &ResultSessionOwner,
        release: bool,
    ) -> Result<ResultContinuationAck, ActorError> {
        let sender = self.result_continuation_sender(owner)?;
        let (respond_to, response) = tokio::sync::oneshot::channel();
        let command = if release {
            ResultContinuationCommand::Release { respond_to }
        } else {
            ResultContinuationCommand::Next { respond_to }
        };
        if sender.send(command).is_err() {
            self.clear_result_continuation(Some(owner));
            return Err(ActorError::NoActiveExecution);
        }
        match response.await {
            Ok(ack) => Ok(ack),
            Err(_) => {
                self.clear_result_continuation(Some(owner));
                Err(ActorError::NoActiveExecution)
            }
        }
    }

    pub async fn request_result_next(
        &self,
        owner: &ResultSessionOwner,
    ) -> Result<ResultContinuationAck, ActorError> {
        self.dispatch_result_continuation(owner, false).await
    }

    /// Release is not complete when the worker merely acknowledges the
    /// command. It returns only after that worker drops/settles its lexical
    /// stream and the exact execution lease is idle.
    pub async fn request_result_release(
        &self,
        owner: &ResultSessionOwner,
    ) -> Result<ResultContinuationAck, ActorError> {
        // Validate all six owner fields against the installed channel before
        // mutating the query-run lease's release intent.
        self.result_continuation_sender(owner)?;
        self.core
            .lock()
            .map_err(|_| ActorError::Closed)?
            .request_result_release(owner)?;
        let run_owner = run_owner_for_result(owner);
        let ack = self.dispatch_result_continuation(owner, true).await?;
        if ack.outcome == ResultContinuationOutcome::Released {
            self.wait_for_owner_settlement(&run_owner).await?;
        }
        Ok(ack)
    }

    pub fn settle_execution(&self, lease: &ExecutionLease) -> Result<Settlement, ActorError> {
        let settlement = self
            .core
            .lock()
            .map_err(|_| ActorError::Closed)?
            .settle_execution(lease)?;
        self.clear_result_continuation_for_run(lease.owner());
        if let Ok(mut channel) = self.mssql_cancel.lock() {
            if channel
                .as_ref()
                .is_some_and(|(owner, _)| owner == lease.owner())
            {
                *channel = None;
            }
        }
        self.settlement_changed.notify_waiters();
        Ok(settlement)
    }

    pub fn install_mssql_cancel_channel(
        &self,
        lease: &ExecutionLease,
        sender: tokio::sync::mpsc::UnboundedSender<QueryRunOwner>,
    ) -> Result<(), ActorError> {
        self.core
            .lock()
            .map_err(|_| ActorError::Closed)?
            .validate_execution_lease(lease)?;
        let mut channel = self.mssql_cancel.lock().map_err(|_| ActorError::Closed)?;
        *channel = Some((lease.owner().clone(), sender));
        Ok(())
    }

    pub fn mark_connection_terminated(&self, lease: &ExecutionLease) -> Result<(), ActorError> {
        self.core
            .lock()
            .map_err(|_| ActorError::Closed)?
            .mark_connection_terminated(lease)
    }

    pub fn cancel_requested(&self, lease: &ExecutionLease) -> Result<bool, ActorError> {
        self.core
            .lock()
            .map_err(|_| ActorError::Closed)?
            .cancel_requested(lease)
    }

    async fn wait_for_owner_settlement(&self, owner: &QueryRunOwner) -> Result<(), ActorError> {
        loop {
            let notified = self.settlement_changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let busy = self
                .core
                .lock()
                .map_err(|_| ActorError::Closed)?
                .owner_is_busy(owner);
            if !busy {
                return Ok(());
            }
            notified.await;
        }
    }

    /// Waits until both the execution lease and metadata lease have settled.
    ///
    /// Lifecycle callers send their teardown signal before entering this wait.
    /// The actor mutex is held only for the instantaneous busy-state check and
    /// is never held across the notification await.
    pub async fn wait_for_settlement(&self) -> Result<(), ActorError> {
        loop {
            let notified = self.settlement_changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let busy = self.core.lock().map_err(|_| ActorError::Closed)?.is_busy();
            if !busy {
                return Ok(());
            }
            notified.await;
        }
    }

    /// Dispatches a production cancellation primitive only after exact-owner
    /// validation, then waits for both dispatch and query settlement.
    pub async fn request_cancel(&self, owner: &QueryRunOwner) -> Result<CancelRequest, ActorError> {
        let request = self
            .core
            .lock()
            .map_err(|_| ActorError::Closed)?
            .request_cancel(owner)?;
        let dispatch_result = match request {
            CancelRequest::AlreadyRequested => {
                self.wait_for_owner_settlement(owner).await?;
                return Ok(request);
            }
            CancelRequest::DriverCancellationRequired(DriverCancelPrimitive::SqliteInterrupt) => {
                match self.sqlite_cancel.as_ref() {
                    Some(cancel) => {
                        cancel.interrupt();
                        Ok(())
                    }
                    None => Err(ActorError::CancelFailed),
                }
            }
            CancelRequest::DriverCancellationRequired(
                DriverCancelPrimitive::PostgresCancelToken,
            ) => match self.postgres_cancel.as_ref() {
                Some(cancel) => cancel.cancel().await.map_err(|_| ActorError::CancelFailed),
                None => Err(ActorError::CancelFailed),
            },
            CancelRequest::ConnectionTerminationRequired => match self.mssql_cancel.lock() {
                Ok(channel) => match channel.as_ref() {
                    Some((active_owner, sender)) if active_owner == owner => sender
                        .send(owner.clone())
                        .map_err(|_| ActorError::CancelFailed),
                    _ => Err(ActorError::CancelFailed),
                },
                Err(_) => Err(ActorError::CancelFailed),
            },
        };
        {
            let mut core = self.core.lock().map_err(|_| ActorError::Closed)?;
            if dispatch_result.is_ok() {
                core.complete_cancel_dispatch(owner)?;
            } else {
                core.fail_cancel_dispatch(owner)?;
            }
        }
        self.settlement_changed.notify_waiters();
        dispatch_result?;
        self.signal_result_cancel(owner);
        self.wait_for_owner_settlement(owner).await?;
        Ok(request)
    }

    pub fn teardown_report(&self) -> TeardownReport {
        self.core
            .lock()
            .map(|core| core.teardown_report())
            .unwrap_or(TeardownReport {
                unreleased_execution: true,
                metadata_in_flight: true,
                unreleased_result_sessions: 0,
                closed: false,
            })
    }

    pub fn begin_teardown(&self) -> Result<TeardownReport, ActorError> {
        let report = self
            .core
            .lock()
            .map_err(|_| ActorError::Closed)?
            .begin_teardown()?;
        self.clear_result_continuation(None);
        Ok(report)
    }

    pub fn is_terminating(&self) -> bool {
        self.core.lock().map(|core| core.closed).unwrap_or(true)
    }

    pub fn request_lifecycle_teardown(&self) -> Result<LifecycleTeardownRequest, ActorError> {
        let state = self
            .core
            .lock()
            .map_err(|_| ActorError::Closed)?
            .request_lifecycle_teardown();
        self.clear_result_continuation(None);
        let driver_action = if state.busy {
            match &self.handle {
                DbHandle::Sqlite(_) => {
                    self.sqlite_cancel
                        .as_ref()
                        .ok_or(ActorError::Closed)?
                        .interrupt();
                    LifecycleDriverAction::SqliteInterrupt
                }
                DbHandle::Postgres(postgres) => {
                    postgres.abort_driver();
                    LifecycleDriverAction::PostgresTransportTermination
                }
                DbHandle::Mssql(_) => {
                    // Unlike SQLite and PostgreSQL, tiberius exposes no
                    // out-of-band transport primitive. Wake the exact
                    // lexical worker through the already-installed owner
                    // channel so teardown also interrupts an initial page
                    // that has not installed its result continuation yet.
                    if let Some(owner) = state.exact_owner.as_ref() {
                        if let Ok(channel) = self.mssql_cancel.lock() {
                            if let Some((active_owner, sender)) = channel.as_ref() {
                                if active_owner == owner {
                                    let _ = sender.send(owner.clone());
                                }
                            }
                        }
                    }
                    LifecycleDriverAction::MssqlConnectionTermination
                }
            }
        } else {
            LifecycleDriverAction::None
        };
        Ok(LifecycleTeardownRequest {
            exact_owner: state.exact_owner,
            cancel_requested: state.cancel_requested,
            release_requested: state.release_requested,
            connection_termination_required: state.connection_termination_required
                || matches!(
                    driver_action,
                    LifecycleDriverAction::PostgresTransportTermination
                        | LifecycleDriverAction::MssqlConnectionTermination
                ),
            busy: state.busy,
            driver_action,
        })
    }
}

impl Drop for ProductionConnectionActor {
    fn drop(&mut self) {
        if let DbHandle::Postgres(postgres) = &self.handle {
            postgres.abort_driver();
        }
    }
}

// ---------------------------------------------------------------------------
// Locked-driver cancellation and cursor-lifetime proofs (P1-T6)
// ---------------------------------------------------------------------------

/// Concrete rusqlite primitive. The handle is `Send + Sync` and may be invoked
/// while the connection-owning worker thread is inside a borrowed Rows scope.
#[derive(Clone)]
pub struct SqliteCancelResource(std::sync::Arc<rusqlite::InterruptHandle>);

impl SqliteCancelResource {
    pub fn from_connection(connection: &rusqlite::Connection) -> Self {
        Self(std::sync::Arc::new(connection.get_interrupt_handle()))
    }

    pub fn interrupt(&self) {
        self.0.interrupt();
    }
}

/// PostgreSQL cancellation needs the TLS connector used by the live client.
#[derive(Clone)]
pub enum PostgresCancelTls {
    NoTls,
    Rustls(tokio_postgres_rustls::MakeRustlsConnect),
}

#[derive(Clone)]
pub struct PostgresCancelResource {
    token: tokio_postgres::CancelToken,
    tls: PostgresCancelTls,
}

impl PostgresCancelResource {
    pub fn no_tls(client: &tokio_postgres::Client) -> Self {
        Self {
            token: client.cancel_token(),
            tls: PostgresCancelTls::NoTls,
        }
    }

    pub fn rustls(
        client: &tokio_postgres::Client,
        tls: tokio_postgres_rustls::MakeRustlsConnect,
    ) -> Self {
        Self {
            token: client.cancel_token(),
            tls: PostgresCancelTls::Rustls(tls),
        }
    }

    pub async fn cancel(&self) -> Result<(), tokio_postgres::Error> {
        match &self.tls {
            PostgresCancelTls::NoTls => self.token.cancel_query(tokio_postgres::NoTls).await,
            PostgresCancelTls::Rustls(tls) => self.token.cancel_query(tls.clone()).await,
        }
    }
}

/// tokio-postgres returns an owned RowStream, so a worker can own all of these
/// real types at once without a self-reference.
#[allow(dead_code)]
struct PostgresWorkerOwnershipProof {
    client: tokio_postgres::Client,
    active: Option<PostgresActiveExecutionProof>,
}

#[allow(dead_code)]
struct PostgresActiveExecutionProof {
    lease: ExecutionLease,
    stream: tokio_postgres::RowStream,
    cancel: PostgresCancelResource,
}

/// rusqlite Statement/Rows borrow lexically from Connection; the outer worker
/// owns `connection`, `lease`, and `cancel` while this function drives Rows.
#[allow(dead_code)]
fn sqlite_lexical_cursor_proof(
    connection: &rusqlite::Connection,
    lease: &ExecutionLease,
    _cancel: &SqliteCancelResource,
) -> rusqlite::Result<(u64, usize)> {
    let mut statement = connection.prepare("SELECT 1 UNION ALL SELECT 2")?;
    let mut rows = statement.query([])?;
    let mut count = 0;
    while rows.next()?.is_some() {
        count += 1;
    }
    Ok((lease.execution_id(), count))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MssqlExecutionExit {
    Completed,
    ConnectionTerminationRequired,
}

/// A real tiberius QueryStream borrows `&mut Client`; it therefore stays in
/// this lexical scope. An exact-owner cancel exits the scope rather than
/// pretending that dropping the stream sent TDS ATTENTION.
#[allow(dead_code)]
async fn drive_mssql_lexical_execution<S>(
    client: &mut tiberius::Client<S>,
    sql: &str,
    owner: &QueryRunOwner,
    cancel_rx: &mut tokio::sync::mpsc::UnboundedReceiver<QueryRunOwner>,
) -> Result<MssqlExecutionExit, tiberius::error::Error>
where
    S: futures_util::AsyncRead + futures_util::AsyncWrite + Unpin + Send,
{
    use futures_util::TryStreamExt;

    let mut stream = client.query(sql, &[]).await?;
    let mut cancellation_open = true;
    loop {
        tokio::select! {
            request = cancel_rx.recv(), if cancellation_open => {
                match request {
                    Some(request) if request == *owner => {
                        return Ok(MssqlExecutionExit::ConnectionTerminationRequired);
                    }
                    Some(_) => {}
                    None => cancellation_open = false,
                }
            }
            item = stream.try_next() => {
                if item?.is_none() {
                    return Ok(MssqlExecutionExit::Completed);
                }
            }
        }
    }
}

/// The outer worker owns the concrete client. Only after the borrowed
/// QueryStream scope returns may it consume/close the client. This is the Q2
/// connection-termination fallback for tiberius 0.12.3.
#[allow(dead_code)]
async fn mssql_worker_ownership_proof<S>(
    mut client: tiberius::Client<S>,
    sql: &str,
    owner: QueryRunOwner,
    mut cancel_rx: tokio::sync::mpsc::UnboundedReceiver<QueryRunOwner>,
) -> Result<MssqlExecutionExit, tiberius::error::Error>
where
    S: futures_util::AsyncRead + futures_util::AsyncWrite + Unpin + Send,
{
    let exit = drive_mssql_lexical_execution(&mut client, sql, &owner, &mut cancel_rx).await?;
    if exit == MssqlExecutionExit::ConnectionTerminationRequired {
        client.close().await?;
    }
    Ok(exit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_service::{
        ConnectionGeneration, ConnectionId, DescriptorId, QueryRunId, ResultSessionId,
        ResultSessionOwner, StatementExecutionId,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::net::TcpStream;
    use tokio_util::compat::Compat;

    fn connection(generation: &str) -> ConnectionIdentity {
        ConnectionIdentity {
            descriptor_id: DescriptorId("descriptor-a".to_string()),
            connection_id: ConnectionId("connection-a".to_string()),
            connection_generation: ConnectionGeneration(generation.to_string()),
        }
    }

    fn owner(generation: &str, query_run_id: &str) -> QueryRunOwner {
        QueryRunOwner {
            descriptor_id: DescriptorId("descriptor-a".to_string()),
            connection_id: ConnectionId("connection-a".to_string()),
            connection_generation: ConnectionGeneration(generation.to_string()),
            query_run_id: QueryRunId(query_run_id.to_string()),
        }
    }

    fn result_owner(
        generation: &str,
        query_run_id: &str,
        statement_id: &str,
        session_id: &str,
    ) -> ResultSessionOwner {
        ResultSessionOwner {
            descriptor_id: DescriptorId("descriptor-a".to_string()),
            connection_id: ConnectionId("connection-a".to_string()),
            connection_generation: ConnectionGeneration(generation.to_string()),
            query_run_id: QueryRunId(query_run_id.to_string()),
            statement_execution_id: StatementExecutionId(statement_id.to_string()),
            result_session_id: ResultSessionId(session_id.to_string()),
        }
    }

    #[test]
    fn cancel_dispatch_and_query_settlement_both_finish_before_run_b() {
        let mut actor = ConnectionActor::new(connection("generation-7"));
        let owner_a = owner("generation-7", "query-a");
        let lease_a = actor
            .acquire_execution(owner_a.clone(), CancelCapability::PostgresProtocolCancel)
            .unwrap();
        assert_eq!(
            actor.request_cancel(&owner_a),
            Ok(CancelRequest::DriverCancellationRequired(
                DriverCancelPrimitive::PostgresCancelToken
            ))
        );
        actor.settle_execution(&lease_a).unwrap();
        assert_eq!(
            actor.acquire_execution(
                owner("generation-7", "query-b"),
                CancelCapability::PostgresProtocolCancel,
            ),
            Err(ActorError::ConnectionBusy)
        );
        assert_eq!(
            actor.complete_cancel_dispatch(&owner("generation-stale", "query-a")),
            Err(ActorError::OwnerMismatch)
        );
        assert_eq!(actor.complete_cancel_dispatch(&owner_a), Ok(()));
        assert!(actor
            .acquire_execution(
                owner("generation-7", "query-b"),
                CancelCapability::PostgresProtocolCancel,
            )
            .is_ok());
    }

    #[test]
    fn query_settlement_still_blocks_run_b_when_cancel_dispatch_finishes_first() {
        let mut actor = ConnectionActor::new(connection("generation-7"));
        let owner_a = owner("generation-7", "query-a");
        let lease_a = actor
            .acquire_execution(owner_a.clone(), CancelCapability::PostgresProtocolCancel)
            .unwrap();
        actor.request_cancel(&owner_a).unwrap();
        actor.complete_cancel_dispatch(&owner_a).unwrap();
        assert_eq!(
            actor.acquire_execution(
                owner("generation-7", "query-b"),
                CancelCapability::PostgresProtocolCancel,
            ),
            Err(ActorError::ConnectionBusy)
        );
        actor.settle_execution(&lease_a).unwrap();
        assert!(actor
            .acquire_execution(
                owner("generation-7", "query-b"),
                CancelCapability::PostgresProtocolCancel,
            )
            .is_ok());
    }

    #[test]
    fn failed_cancel_dispatch_rolls_back_pending_state_and_allows_retry() {
        let mut actor = ConnectionActor::new(connection("generation-7"));
        let owner_a = owner("generation-7", "query-a");
        let lease = actor
            .acquire_execution(owner_a.clone(), CancelCapability::PostgresProtocolCancel)
            .unwrap();
        actor.request_cancel(&owner_a).unwrap();
        assert_eq!(actor.fail_cancel_dispatch(&owner_a), Ok(()));
        assert_eq!(
            actor.request_cancel(&owner_a),
            Ok(CancelRequest::DriverCancellationRequired(
                DriverCancelPrimitive::PostgresCancelToken
            ))
        );
        actor.complete_cancel_dispatch(&owner_a).unwrap();
        actor.settle_execution(&lease).unwrap();
        assert!(!actor.is_busy());
    }

    #[tokio::test]
    async fn concurrent_duplicate_cancel_waiters_observe_one_settlement_without_hanging() {
        let actor = Arc::new(ProductionConnectionActor::new(
            connection("generation-7"),
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        ));
        let owner_a = owner("generation-7", "query-a");
        let lease = actor
            .acquire_execution(owner_a.clone(), CancelCapability::SqliteInterrupt)
            .unwrap();
        let first_actor = actor.clone();
        let first_owner = owner_a.clone();
        let first = tokio::spawn(async move { first_actor.request_cancel(&first_owner).await });
        tokio::task::yield_now().await;
        let second_actor = actor.clone();
        let second_owner = owner_a.clone();
        let second = tokio::spawn(async move { second_actor.request_cancel(&second_owner).await });
        tokio::task::yield_now().await;
        actor.settle_execution(&lease).unwrap();

        let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            (
                first.await.unwrap().unwrap(),
                second.await.unwrap().unwrap(),
            )
        })
        .await
        .expect("cancel waiters lost the settlement notification");
        assert!(matches!(
            first,
            CancelRequest::DriverCancellationRequired(DriverCancelPrimitive::SqliteInterrupt)
        ));
        assert_eq!(second, CancelRequest::AlreadyRequested);
    }

    #[tokio::test]
    async fn mssql_exact_owner_channel_waits_for_close_mark_and_settlement() {
        let actor = Arc::new(ProductionConnectionActor::new(
            connection("generation-7"),
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        ));
        let owner_a = owner("generation-7", "query-a");
        let lease = actor
            .acquire_execution(
                owner_a.clone(),
                CancelCapability::MssqlConnectionTermination,
            )
            .unwrap();
        let (cancel_tx, mut cancel_rx) = tokio::sync::mpsc::unbounded_channel();
        actor
            .install_mssql_cancel_channel(&lease, cancel_tx)
            .unwrap();
        let cancel_actor = actor.clone();
        let cancel_owner = owner_a.clone();
        let cancellation =
            tokio::spawn(async move { cancel_actor.request_cancel(&cancel_owner).await });
        assert_eq!(cancel_rx.recv().await, Some(owner_a));
        actor.mark_connection_terminated(&lease).unwrap();
        actor.settle_execution(&lease).unwrap();
        assert_eq!(
            cancellation.await.unwrap().unwrap(),
            CancelRequest::ConnectionTerminationRequired
        );
        assert!(actor.is_terminating());
    }

    #[tokio::test]
    async fn exact_result_continuation_dispatches_next_and_keeps_metadata_busy() {
        let actor = Arc::new(ProductionConnectionActor::new(
            connection("generation-7"),
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        ));
        let query_owner = owner("generation-7", "query-a");
        let result_owner = result_owner("generation-7", "query-a", "statement-a", "session-a");
        let lease = actor
            .acquire_execution(query_owner, CancelCapability::SqliteInterrupt)
            .unwrap();
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        actor
            .install_result_continuation(&lease, result_owner.clone(), sender)
            .unwrap();

        let next_actor = actor.clone();
        let next_owner = result_owner.clone();
        let next = tokio::spawn(async move { next_actor.request_result_next(&next_owner).await });
        let command = receiver
            .recv()
            .await
            .expect("Next command was not dispatched");
        let ResultContinuationCommand::Next { respond_to } = command else {
            panic!("expected Next continuation command")
        };
        respond_to
            .send(ResultContinuationAck {
                outcome: ResultContinuationOutcome::PageReady,
            })
            .unwrap();
        assert_eq!(
            next.await.unwrap(),
            Ok(ResultContinuationAck {
                outcome: ResultContinuationOutcome::PageReady,
            })
        );
        assert_eq!(actor.acquire_metadata(), Err(ActorError::ConnectionBusy));
        actor.settle_execution(&lease).unwrap();
        assert!(receiver.recv().await.is_none());
    }

    #[tokio::test]
    async fn result_continuation_rejects_every_wrong_owner_component() {
        let actor = ProductionConnectionActor::new(
            connection("generation-7"),
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        );
        let lease = actor
            .acquire_execution(
                owner("generation-7", "query-a"),
                CancelCapability::SqliteInterrupt,
            )
            .unwrap();
        let exact = result_owner("generation-7", "query-a", "statement-a", "session-a");
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        actor
            .install_result_continuation(&lease, exact.clone(), sender)
            .unwrap();

        let mut wrong_descriptor = exact.clone();
        wrong_descriptor.descriptor_id = DescriptorId("descriptor-b".to_string());
        let mut wrong_connection = exact.clone();
        wrong_connection.connection_id = ConnectionId("connection-b".to_string());
        let mut wrong_generation = exact.clone();
        wrong_generation.connection_generation = ConnectionGeneration("generation-old".to_string());
        let mut wrong_run = exact.clone();
        wrong_run.query_run_id = QueryRunId("query-b".to_string());
        let mut wrong_statement = exact.clone();
        wrong_statement.statement_execution_id = StatementExecutionId("statement-b".to_string());
        let mut wrong_session = exact.clone();
        wrong_session.result_session_id = ResultSessionId("session-b".to_string());

        for wrong in [
            wrong_descriptor,
            wrong_connection,
            wrong_generation,
            wrong_run,
            wrong_statement.clone(),
            wrong_session,
        ] {
            assert_eq!(
                actor.request_result_next(&wrong).await,
                Err(ActorError::OwnerMismatch)
            );
            assert_eq!(
                actor.request_result_release(&wrong).await,
                Err(ActorError::OwnerMismatch)
            );
        }
        let (replacement, _) = tokio::sync::mpsc::unbounded_channel();
        assert_eq!(
            actor.install_result_continuation(&lease, wrong_statement, replacement),
            Err(ActorError::OwnerMismatch)
        );
        let (replacement, _) = tokio::sync::mpsc::unbounded_channel();
        let mut wrong_session = exact.clone();
        wrong_session.result_session_id = ResultSessionId("session-c".to_string());
        assert_eq!(
            actor.install_result_continuation(&lease, wrong_session, replacement),
            Err(ActorError::OwnerMismatch)
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
        actor.settle_execution(&lease).unwrap();
    }

    #[tokio::test]
    async fn result_release_waits_for_worker_settlement_before_lease_becomes_idle() {
        let actor = Arc::new(ProductionConnectionActor::new(
            connection("generation-7"),
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        ));
        let query_owner = owner("generation-7", "query-a");
        let result_owner = result_owner("generation-7", "query-a", "statement-a", "session-a");
        let lease = actor
            .acquire_execution(query_owner, CancelCapability::SqliteInterrupt)
            .unwrap();
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        actor
            .install_result_continuation(&lease, result_owner.clone(), sender)
            .unwrap();

        let release_actor = actor.clone();
        let release_owner = result_owner.clone();
        let release =
            tokio::spawn(async move { release_actor.request_result_release(&release_owner).await });
        let command = receiver
            .recv()
            .await
            .expect("Release command was not dispatched");
        let ResultContinuationCommand::Release { respond_to } = command else {
            panic!("expected Release continuation command")
        };
        respond_to
            .send(ResultContinuationAck {
                outcome: ResultContinuationOutcome::Released,
            })
            .unwrap();
        tokio::task::yield_now().await;
        assert!(
            !release.is_finished(),
            "Release returned before stream settlement"
        );
        assert_eq!(actor.acquire_metadata(), Err(ActorError::ConnectionBusy));
        assert!(actor.settle_execution(&lease).unwrap().release_requested);
        assert_eq!(
            release.await.unwrap(),
            Ok(ResultContinuationAck {
                outcome: ResultContinuationOutcome::Released,
            })
        );
        let metadata = actor.acquire_metadata().unwrap();
        actor.settle_metadata(&metadata).unwrap();
    }

    #[tokio::test]
    async fn settlement_channel_close_and_late_generation_clear_continuation() {
        let actor = ProductionConnectionActor::new(
            connection("generation-7"),
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        );
        let lease = actor
            .acquire_execution(
                owner("generation-7", "query-a"),
                CancelCapability::SqliteInterrupt,
            )
            .unwrap();
        let exact = result_owner("generation-7", "query-a", "statement-a", "session-a");
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        actor
            .install_result_continuation(&lease, exact.clone(), sender)
            .unwrap();
        actor.settle_execution(&lease).unwrap();
        assert!(receiver.recv().await.is_none());
        assert_eq!(
            actor.request_result_next(&exact).await,
            Err(ActorError::NoActiveExecution)
        );

        let new_actor = ProductionConnectionActor::new(
            connection("generation-8"),
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        );
        let new_lease = new_actor
            .acquire_execution(
                owner("generation-8", "query-b"),
                CancelCapability::SqliteInterrupt,
            )
            .unwrap();
        let new_owner = result_owner("generation-8", "query-b", "statement-b", "session-b");
        let (new_sender, mut new_receiver) = tokio::sync::mpsc::unbounded_channel();
        new_actor
            .install_result_continuation(&new_lease, new_owner, new_sender)
            .unwrap();
        assert_eq!(
            new_actor.request_result_next(&exact).await,
            Err(ActorError::OwnerMismatch)
        );
        assert!(matches!(
            new_receiver.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
        new_actor.settle_execution(&new_lease).unwrap();
    }

    #[tokio::test]
    async fn closed_worker_channel_is_cleared_and_can_be_reinstalled() {
        let actor = ProductionConnectionActor::new(
            connection("generation-7"),
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        );
        let lease = actor
            .acquire_execution(
                owner("generation-7", "query-a"),
                CancelCapability::SqliteInterrupt,
            )
            .unwrap();
        let exact = result_owner("generation-7", "query-a", "statement-a", "session-a");
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        actor
            .install_result_continuation(&lease, exact.clone(), sender)
            .unwrap();
        drop(receiver);
        assert_eq!(
            actor.request_result_next(&exact).await,
            Err(ActorError::NoActiveExecution)
        );

        let (replacement, _replacement_receiver) = tokio::sync::mpsc::unbounded_channel();
        actor
            .install_result_continuation(&lease, exact, replacement)
            .unwrap();
        actor.settle_execution(&lease).unwrap();
    }

    #[test]
    fn lifecycle_teardown_closes_the_result_continuation_channel() {
        let actor = ProductionConnectionActor::new(
            connection("generation-7"),
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        );
        let lease = actor
            .acquire_execution(
                owner("generation-7", "query-a"),
                CancelCapability::SqliteInterrupt,
            )
            .unwrap();
        let exact = result_owner("generation-7", "query-a", "statement-a", "session-a");
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        actor
            .install_result_continuation(&lease, exact, sender)
            .unwrap();

        assert!(
            actor
                .request_lifecycle_teardown()
                .unwrap()
                .release_requested
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
        ));
        actor.settle_execution(&lease).unwrap();
    }

    #[tokio::test]
    async fn lifecycle_teardown_wakes_stream_worker_and_waits_for_exact_settlement() {
        let actor = Arc::new(ProductionConnectionActor::new(
            connection("generation-shutdown"),
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        ));
        let lease = actor
            .acquire_execution(
                owner("generation-shutdown", "query-streaming"),
                CancelCapability::SqliteInterrupt,
            )
            .unwrap();
        let exact = result_owner(
            "generation-shutdown",
            "query-streaming",
            "statement-streaming",
            "session-streaming",
        );
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        actor
            .install_result_continuation(&lease, exact, sender)
            .unwrap();

        let worker_actor = Arc::clone(&actor);
        let worker_lease = lease.clone();
        let worker = tokio::spawn(async move {
            assert!(receiver.recv().await.is_none());
            worker_actor.settle_execution(&worker_lease).unwrap()
        });

        let request = actor.request_lifecycle_teardown().unwrap();
        assert_eq!(
            request.exact_owner,
            Some(owner("generation-shutdown", "query-streaming"))
        );
        assert!(request.cancel_requested);
        assert!(request.release_requested);
        tokio::time::timeout(Duration::from_secs(1), actor.wait_for_settlement())
            .await
            .expect("stream worker did not settle after lifecycle signal")
            .unwrap();
        assert_eq!(
            worker.await.unwrap(),
            Settlement {
                cancel_requested: true,
                release_requested: true,
                connection_termination_required: false,
            }
        );
        assert!(actor.begin_teardown().unwrap().closed);

        let repeated = actor.request_lifecycle_teardown().unwrap();
        assert!(!repeated.busy);
        actor.wait_for_settlement().await.unwrap();
        assert!(actor.begin_teardown().unwrap().closed);
    }

    #[tokio::test]
    async fn lifecycle_settlement_wait_includes_metadata_and_is_notified_on_settle() {
        let actor = ProductionConnectionActor::new(
            connection("generation-metadata"),
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        );
        let lease = actor.acquire_metadata().unwrap();

        let request = actor.request_lifecycle_teardown().unwrap();
        assert!(request.busy);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), actor.wait_for_settlement())
                .await
                .is_err(),
            "metadata still in flight must keep lifecycle settlement pending"
        );

        actor.settle_metadata(&lease).unwrap();
        tokio::time::timeout(Duration::from_secs(1), actor.wait_for_settlement())
            .await
            .expect("metadata settlement notification was lost")
            .unwrap();
        let report = actor.begin_teardown().unwrap();
        assert!(!report.metadata_in_flight);
        assert!(report.closed);
    }

    #[test]
    fn canonical_owner_and_single_lease_gate_are_enforced() {
        let mut actor = ConnectionActor::new(connection("generation-7"));
        assert_eq!(
            actor.acquire_execution(
                owner("generation-stale", "query-a"),
                CancelCapability::SqliteInterrupt,
            ),
            Err(ActorError::OwnerMismatch)
        );

        let lease = actor
            .acquire_execution(
                owner("generation-7", "query-a"),
                CancelCapability::SqliteInterrupt,
            )
            .unwrap();
        assert_eq!(lease.owner(), &owner("generation-7", "query-a"));
        assert_eq!(
            actor.acquire_execution(
                owner("generation-7", "query-b"),
                CancelCapability::PostgresProtocolCancel,
            ),
            Err(ActorError::ConnectionBusy)
        );
        actor.settle_execution(&lease).unwrap();
        assert!(!actor.is_busy());
    }

    #[test]
    fn stale_cancel_never_exposes_a_driver_action() {
        let mut actor = ConnectionActor::new(connection("generation-7"));
        let lease = actor
            .acquire_execution(
                owner("generation-7", "query-a"),
                CancelCapability::PostgresProtocolCancel,
            )
            .unwrap();

        assert_eq!(
            actor.request_cancel(&owner("generation-stale", "query-a")),
            Err(ActorError::OwnerMismatch)
        );
        assert_eq!(
            actor.request_cancel(&owner("generation-7", "query-b")),
            Err(ActorError::OwnerMismatch)
        );
        assert_eq!(
            actor.request_cancel(&owner("generation-7", "query-a")),
            Ok(CancelRequest::DriverCancellationRequired(
                DriverCancelPrimitive::PostgresCancelToken
            ))
        );
        assert_eq!(
            actor.request_cancel(&owner("generation-7", "query-a")),
            Ok(CancelRequest::AlreadyRequested)
        );
        assert!(actor.is_busy(), "cancel request must not release the lease");
        actor.settle_execution(&lease).unwrap();
    }

    #[test]
    fn mssql_cancel_requires_connection_close_and_stays_busy_until_settlement() {
        let mut actor = ConnectionActor::new(connection("generation-7"));
        let lease = actor
            .acquire_execution(
                owner("generation-7", "query-a"),
                CancelCapability::MssqlConnectionTermination,
            )
            .unwrap();
        assert_eq!(
            actor.request_cancel(&owner("generation-7", "query-a")),
            Ok(CancelRequest::ConnectionTerminationRequired)
        );
        assert_eq!(
            actor.acquire_execution(
                owner("generation-7", "query-b"),
                CancelCapability::MssqlConnectionTermination,
            ),
            Err(ActorError::ConnectionBusy)
        );
        assert_eq!(
            actor.settle_execution(&lease).unwrap(),
            Settlement {
                cancel_requested: true,
                release_requested: false,
                connection_termination_required: true,
            }
        );
    }

    #[test]
    fn mssql_lifecycle_request_marks_termination_and_never_reuses_the_actor() {
        let mut actor = ConnectionActor::new(connection("generation-7"));
        let lease = actor
            .acquire_execution(
                owner("generation-7", "query-lifecycle"),
                CancelCapability::MssqlConnectionTermination,
            )
            .unwrap();

        let request = actor.request_lifecycle_teardown();
        assert_eq!(
            request.exact_owner,
            Some(owner("generation-7", "query-lifecycle"))
        );
        assert!(request.cancel_requested);
        assert!(request.release_requested);
        assert!(request.connection_termination_required);
        assert_eq!(
            actor.acquire_execution(
                owner("generation-7", "query-after-termination"),
                CancelCapability::MssqlConnectionTermination,
            ),
            Err(ActorError::Closed)
        );
        assert_eq!(
            actor.settle_execution(&lease).unwrap(),
            Settlement {
                cancel_requested: true,
                release_requested: true,
                connection_termination_required: true,
            }
        );
    }

    #[test]
    fn mssql_lifecycle_wakes_the_exact_worker_before_result_continuation_install() {
        let actor = ProductionConnectionActor::new(
            connection("generation-7"),
            DbHandle::Mssql(tokio::sync::Mutex::new(None)),
        );
        let run_owner = owner("generation-7", "query-lifecycle");
        let lease = actor
            .acquire_execution(
                run_owner.clone(),
                CancelCapability::MssqlConnectionTermination,
            )
            .unwrap();
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        actor.install_mssql_cancel_channel(&lease, sender).unwrap();

        let request = actor.request_lifecycle_teardown().unwrap();
        assert_eq!(request.exact_owner, Some(run_owner.clone()));
        assert_eq!(receiver.try_recv(), Ok(run_owner));
        assert_eq!(
            actor.settle_execution(&lease).unwrap(),
            Settlement {
                cancel_requested: true,
                release_requested: true,
                connection_termination_required: true,
            }
        );
    }

    #[test]
    fn release_and_late_settlement_require_the_exact_lease() {
        let mut actor = ConnectionActor::new(connection("generation-7"));
        let lease_a = actor
            .acquire_execution(
                owner("generation-7", "query-a"),
                CancelCapability::SqliteInterrupt,
            )
            .unwrap();
        assert_eq!(
            actor.request_release(&lease_a),
            Ok(ReleaseRequest::Requested)
        );
        assert_eq!(
            actor.request_release(&lease_a),
            Ok(ReleaseRequest::AlreadyRequested)
        );
        actor.settle_execution(&lease_a).unwrap();

        let lease_b = actor
            .acquire_execution(
                owner("generation-7", "query-a"),
                CancelCapability::SqliteInterrupt,
            )
            .unwrap();
        assert_ne!(lease_a.execution_id(), lease_b.execution_id());
        assert_eq!(
            actor.settle_execution(&lease_a),
            Err(ActorError::StaleLease)
        );
        actor.settle_execution(&lease_b).unwrap();
    }

    #[test]
    fn metadata_fails_fast_while_result_lease_is_owned_and_another_actor_succeeds() {
        let actor_a = ProductionConnectionActor::new(
            connection("generation-7"),
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        );
        let actor_b = ProductionConnectionActor::new(
            ConnectionIdentity {
                descriptor_id: DescriptorId("descriptor-b".to_string()),
                connection_id: ConnectionId("connection-b".to_string()),
                connection_generation: ConnectionGeneration("generation-1".to_string()),
            },
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        );
        let lease = actor_a
            .acquire_execution(
                owner("generation-7", "query-a"),
                CancelCapability::SqliteInterrupt,
            )
            .unwrap();

        assert_eq!(actor_a.acquire_metadata(), Err(ActorError::ConnectionBusy));
        let metadata_b = actor_b.acquire_metadata().unwrap();
        actor_b.settle_metadata(&metadata_b).unwrap();
        actor_a.settle_execution(&lease).unwrap();
    }

    #[test]
    fn lifecycle_teardown_requests_exact_cancel_release_before_retry_close() {
        let actor = ProductionConnectionActor::new(
            connection("generation-7"),
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        );
        let lease = actor
            .acquire_execution(
                owner("generation-7", "query-a"),
                CancelCapability::SqliteInterrupt,
            )
            .unwrap();

        assert_eq!(
            actor.request_lifecycle_teardown().unwrap(),
            LifecycleTeardownRequest {
                exact_owner: Some(owner("generation-7", "query-a")),
                cancel_requested: true,
                release_requested: true,
                connection_termination_required: false,
                busy: true,
                driver_action: LifecycleDriverAction::SqliteInterrupt,
            }
        );
        assert_eq!(
            actor.teardown_report(),
            TeardownReport {
                unreleased_execution: true,
                metadata_in_flight: false,
                unreleased_result_sessions: 0,
                closed: true,
            }
        );
        assert_eq!(actor.acquire_metadata(), Err(ActorError::Closed));
        assert_eq!(
            actor.settle_execution(&lease).unwrap(),
            Settlement {
                cancel_requested: true,
                release_requested: true,
                connection_termination_required: false,
            }
        );
        assert_eq!(
            actor.begin_teardown().unwrap(),
            TeardownReport {
                unreleased_execution: false,
                metadata_in_flight: false,
                unreleased_result_sessions: 0,
                closed: true,
            }
        );
        assert_eq!(actor.acquire_metadata(), Err(ActorError::Closed));
    }

    #[test]
    fn sqlite_lexical_cursor_and_interrupt_use_real_locked_types() {
        let sqlite_connection = rusqlite::Connection::open_in_memory().unwrap();
        let cancel = SqliteCancelResource::from_connection(&sqlite_connection);
        let mut actor = ConnectionActor::new(connection("generation-7"));
        let lease = actor
            .acquire_execution(
                owner("generation-7", "query-a"),
                CancelCapability::SqliteInterrupt,
            )
            .unwrap();
        assert_eq!(
            sqlite_lexical_cursor_proof(&sqlite_connection, &lease, &cancel).unwrap(),
            (lease.execution_id(), 2)
        );
        actor.settle_execution(&lease).unwrap();
    }

    #[test]
    fn sqlite_interrupt_stops_a_live_query_on_the_connection_worker() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let cancel = SqliteCancelResource::from_connection(&connection);
        // This is a cleanup guard, not the proof mechanism. It is set only
        // after the InterruptHandle deadline has already failed, so it cannot
        // make the test pass while ensuring a regression never leaves a
        // billion-step worker running indefinitely.
        let cleanup_requested = Arc::new(AtomicBool::new(false));
        let worker_cleanup = Arc::clone(&cleanup_requested);
        connection
            .progress_handler(10_000, Some(move || worker_cleanup.load(Ordering::Acquire)))
            .unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let mut statement = connection
                .prepare(
                    "WITH RECURSIVE counter(value) AS (VALUES(0) UNION ALL SELECT value + 1 FROM counter WHERE value < 1000000000) SELECT sum(value) FROM counter",
                )
                .unwrap();
            started_tx.send(()).unwrap();
            let result = statement.query_row([], |row| row.get::<_, i64>(0));
            result_tx.send(result).unwrap();
        });

        started_rx.recv().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let result = loop {
            cancel.interrupt();
            let now = Instant::now();
            if now >= deadline {
                cleanup_requested.store(true, Ordering::Release);
                let cleanup = result_rx.recv_timeout(Duration::from_secs(2));
                if cleanup.is_ok() {
                    worker.join().expect("SQLite cleanup worker panicked");
                }
                panic!(
                    "rusqlite InterruptHandle did not settle the live query within the 3s deadline"
                );
            }
            let wait = (deadline - now).min(Duration::from_millis(10));
            match result_rx.recv_timeout(wait) {
                Ok(result) => break result,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("SQLite worker disconnected before reporting its result")
                }
            }
        };
        worker.join().expect("SQLite worker panicked");
        let error = result.expect_err("long-running SQLite query was not interrupted");
        assert_eq!(
            error.sqlite_error_code(),
            Some(rusqlite::ErrorCode::OperationInterrupted)
        );
    }

    #[test]
    fn production_lifecycle_request_interrupts_sqlite_and_settles_before_exact_close() {
        let actor = Arc::new(ProductionConnectionActor::new(
            connection("generation-lifecycle"),
            DbHandle::Sqlite(Mutex::new(rusqlite::Connection::open_in_memory().unwrap())),
        ));
        let lease = actor
            .acquire_execution(
                owner("generation-lifecycle", "query-lifecycle"),
                CancelCapability::SqliteInterrupt,
            )
            .unwrap();
        let cleanup_requested = Arc::new(AtomicBool::new(false));
        if let DbHandle::Sqlite(connection) = actor.handle() {
            let worker_cleanup = Arc::clone(&cleanup_requested);
            connection
                .lock()
                .unwrap()
                .progress_handler(10_000, Some(move || worker_cleanup.load(Ordering::Acquire)))
                .unwrap();
        }
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        let worker_actor = Arc::clone(&actor);
        let worker_lease = lease.clone();
        let worker = std::thread::spawn(move || {
            let result = match worker_actor.handle() {
                DbHandle::Sqlite(connection) => {
                    let connection = connection.lock().unwrap();
                    let mut statement = connection
                        .prepare(
                            "WITH RECURSIVE counter(value) AS (VALUES(0) UNION ALL SELECT value + 1 FROM counter WHERE value < 1000000000) SELECT sum(value) FROM counter",
                        )
                        .unwrap();
                    started_tx.send(()).unwrap();
                    statement.query_row([], |row| row.get::<_, i64>(0))
                }
                _ => unreachable!("test actor is SQLite"),
            };
            let settlement = worker_actor.settle_execution(&worker_lease).unwrap();
            result_tx.send((result, settlement)).unwrap();
        });

        started_rx.recv().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let (result, settlement) = loop {
            let request = actor.request_lifecycle_teardown().unwrap();
            assert_eq!(
                request.exact_owner,
                Some(owner("generation-lifecycle", "query-lifecycle"))
            );
            assert!(request.cancel_requested);
            assert!(request.release_requested);
            assert_eq!(
                request.driver_action,
                LifecycleDriverAction::SqliteInterrupt
            );
            match result_rx.recv_timeout(Duration::from_millis(10)) {
                Ok(result) => break result,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
                Err(_) => {
                    cleanup_requested.store(true, Ordering::Release);
                    let _ = result_rx.recv_timeout(Duration::from_secs(2));
                    panic!("lifecycle SQLite interrupt did not settle within deadline")
                }
            }
        };
        worker.join().unwrap();
        assert_eq!(
            result.unwrap_err().sqlite_error_code(),
            Some(rusqlite::ErrorCode::OperationInterrupted)
        );
        assert_eq!(
            settlement,
            Settlement {
                cancel_requested: true,
                release_requested: true,
                connection_termination_required: false,
            }
        );
        assert!(actor.begin_teardown().unwrap().closed);
    }

    #[test]
    fn postgres_and_mssql_real_driver_shapes_compile() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<rusqlite::InterruptHandle>();
        assert_send_sync::<tokio_postgres::CancelToken>();

        let _pg_owned_shape: fn(
            tokio_postgres::Client,
            Option<PostgresActiveExecutionProof>,
        ) -> PostgresWorkerOwnershipProof =
            |client, active| PostgresWorkerOwnershipProof { client, active };
        let _pg_cancel = PostgresCancelResource::cancel;

        type MssqlWire = Compat<TcpStream>;
        let _mssql_borrowed_stream_worker = mssql_worker_ownership_proof::<MssqlWire>;
    }
}
