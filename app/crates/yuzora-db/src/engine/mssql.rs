//! STUB: replaced by the engine implementation.

use super::{ConnectSpec, RowSink};
use crate::error::{DbError, DbResult};
use crate::model::{DbObject, ObjectRef, RowEdit, TableInfo};

pub struct MsConn;

#[derive(Clone)]
pub struct MsCanceller;

impl MsCanceller {
    pub async fn cancel(&self) {}
}

impl MsConn {
    pub async fn connect(_spec: &ConnectSpec) -> DbResult<Self> {
        Err(DbError::Connect("not implemented".into()))
    }
    pub fn server_version(&self) -> String {
        String::new()
    }
    pub fn is_closed(&self) -> bool {
        false
    }
    pub fn canceller(&self) -> MsCanceller {
        MsCanceller
    }
    pub async fn databases(&mut self) -> DbResult<Vec<String>> {
        Ok(Vec::new())
    }
    pub async fn objects(&mut self) -> DbResult<Vec<DbObject>> {
        Ok(Vec::new())
    }
    pub async fn describe(&mut self, _object: &ObjectRef) -> DbResult<TableInfo> {
        Ok(TableInfo::default())
    }
    pub async fn execute(&mut self, _sql: &str, _sink: &mut dyn RowSink) -> DbResult<()> {
        Ok(())
    }
    pub async fn apply_edits(&mut self, _table: &ObjectRef, _edits: &[RowEdit]) -> DbResult<u64> {
        Ok(0)
    }
}
