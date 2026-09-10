//! Desktop IPC and profile lifecycle around the shared database core.
use crate::db_result_session::ResultSessionState;
pub use yuzora_host::db_service::*;

/// Completes the cross-registry cleanup for an MSSQL Cancel termination.
///
/// Both the run and Cancel commands can observe driver settlement first. The
/// exact profile tombstone makes either ordering idempotent without allowing a
/// late finalizer to remove a newer connection generation.
pub(crate) fn finalize_terminated_connection(
    state: &DbState,
    sessions: &ResultSessionState,
    profiles: &crate::db_profiles::DatabaseProfileState,
    identity: &ConnectionIdentity,
) -> Result<(), DatabaseOperationalError> {
    match close_exact_in_state(state, identity) {
        Ok(_) => {}
        Err(error)
            if matches!(
                error.code,
                DatabaseOperationalErrorCode::StaleConnection
                    | DatabaseOperationalErrorCode::ServerDisconnected
            ) =>
        {
            let actors = state
                .0
                .lock()
                .map_err(|_| DatabaseOperationalError::connection_failed())?;
            if actors
                .get(&identity.connection_id.0)
                .is_some_and(|actor| actor.identity() != identity)
            {
                return Err(DatabaseOperationalError::new(
                    DatabaseOperationalErrorCode::StaleConnection,
                    "database connection identity is stale",
                ));
            }
        }
        Err(error) => return Err(error),
    }

    profiles
        .mark_exact_connection_offline(identity)
        .map_err(|error| {
            let code = if error.code == crate::db_profiles::ProfileErrorCode::StaleConnection {
                DatabaseOperationalErrorCode::StaleConnection
            } else {
                DatabaseOperationalErrorCode::ConnectionFailed
            };
            DatabaseOperationalError::new(code, "terminated database connection cleanup failed")
        })?;
    sessions
        .lock()
        .map_err(|_| {
            DatabaseOperationalError::new(
                DatabaseOperationalErrorCode::ConnectionFailed,
                "result session registry is unavailable",
            )
        })?
        .release_connection(identity)
        .map_err(|_| {
            DatabaseOperationalError::new(
                DatabaseOperationalErrorCode::ConnectionFailed,
                "result session cleanup failed",
            )
        })?;
    Ok(())
}

#[tauri::command]
pub async fn db_list_tables(
    state: tauri::State<'_, DbState>,
    identity: ConnectionIdentity,
) -> Result<Vec<TableInfo>, DatabaseOperationalError> {
    list_tables_in_state(&state, identity).await
}

#[tauri::command]
pub async fn db_table_columns(
    state: tauri::State<'_, DbState>,
    identity: ConnectionIdentity,
    table: TableInfo,
) -> Result<Vec<ColumnInfo>, DatabaseOperationalError> {
    table_columns_in_state(&state, identity, table).await
}

#[tauri::command]
pub async fn db_query_run(
    state: tauri::State<'_, DbState>,
    sessions: tauri::State<'_, ResultSessionState>,
    profiles: tauri::State<'_, crate::db_profiles::DatabaseProfileState>,
    request: QueryRunRequest,
) -> Result<QueryRun, DatabaseOperationalError> {
    let identity = ConnectionIdentity {
        descriptor_id: request.descriptor_id.clone(),
        connection_id: request.connection_id.clone(),
        connection_generation: request.connection_generation.clone(),
    };
    let run = query_run_in_state(&state, &sessions, request).await?;
    if run.connection_terminated {
        finalize_terminated_connection(&state, &sessions, &profiles, &identity)?;
    }
    Ok(run)
}

#[tauri::command]
pub async fn db_query_cancel(
    state: tauri::State<'_, DbState>,
    sessions: tauri::State<'_, ResultSessionState>,
    profiles: tauri::State<'_, crate::db_profiles::DatabaseProfileState>,
    owner: QueryRunOwner,
) -> Result<QueryCancelResult, DatabaseOperationalError> {
    let identity = ConnectionIdentity {
        descriptor_id: owner.descriptor_id.clone(),
        connection_id: owner.connection_id.clone(),
        connection_generation: owner.connection_generation.clone(),
    };
    let result = query_cancel_in_state(&state, owner).await?;
    if result.outcome == QueryCancelOutcome::CancelledConnectionTerminated {
        finalize_terminated_connection(&state, &sessions, &profiles, &identity)?;
    }
    Ok(result)
}

#[tauri::command]
pub async fn db_result_page(
    state: tauri::State<'_, DbState>,
    sessions: tauri::State<'_, ResultSessionState>,
    request: ResultPageRequest,
) -> Result<ResultPage, DatabaseOperationalError> {
    result_page_in_state(&state, &sessions, request).await
}

#[tauri::command]
pub async fn db_result_session_release(
    state: tauri::State<'_, DbState>,
    sessions: tauri::State<'_, ResultSessionState>,
    owner: ResultSessionOwner,
) -> Result<ResultPage, DatabaseOperationalError> {
    result_session_release_in_state(&state, &sessions, owner).await
}
