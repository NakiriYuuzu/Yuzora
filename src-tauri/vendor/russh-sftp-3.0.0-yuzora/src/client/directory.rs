use std::sync::Arc;

use super::{error::Error, rawsession::SftpResult, runtime, RawSftpSession};
use crate::protocol::Status;

/// Owns an acquired directory until its one close request has been scheduled.
pub(super) struct DirectoryHandle {
    session: Arc<RawSftpSession>,
    handle: Option<String>,
}

impl DirectoryHandle {
    pub(super) async fn open(session: Arc<RawSftpSession>, path: String) -> SftpResult<Self> {
        // Keep the OPEN receiver alive if the listing is cancelled before its reply.
        // An abandoned task result drops this owner and schedules CLOSE.
        runtime::spawn(async move {
            let handle = session.opendir(path).await?.handle;
            Ok(Self {
                session,
                handle: Some(handle),
            })
        })
        .await
        .map_err(|error| Error::UnexpectedBehavior(error.to_string()))?
    }

    pub(super) fn as_str(&self) -> &str {
        self.handle.as_deref().expect("directory is still owned")
    }

    fn schedule_close(&mut self) -> Option<runtime::JoinHandle<SftpResult<Status>>> {
        self.handle.take().map(|handle| {
            let session = Arc::clone(&self.session);
            // Finish the acknowledgement and local handle accounting even if the
            // caller drops its close future. The option prevents duplicate CLOSE.
            runtime::spawn(async move { session.close(handle).await })
        })
    }

    pub(super) async fn close(mut self) -> SftpResult<()> {
        self.schedule_close()
            .expect("directory is still owned")
            .await
            .map_err(|error| Error::UnexpectedBehavior(error.to_string()))??;
        Ok(())
    }
}

impl Drop for DirectoryHandle {
    fn drop(&mut self) {
        let _ = self.schedule_close();
    }
}
