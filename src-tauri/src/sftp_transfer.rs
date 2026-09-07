//! Exact-session transfer ownership. Reserve before exposing Cancel in the UI.
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;

struct Entry {
    session: String,
    cancel: watch::Sender<bool>,
    started: bool,
    expires: Instant,
}

#[derive(Clone, Default)]
pub(crate) struct Transfers(Arc<Mutex<HashMap<String, Entry>>>);

impl Transfers {
    pub(crate) fn observer(
        &self,
        session: &str,
        id: &str,
    ) -> Result<watch::Receiver<bool>, String> {
        let entries = self.0.lock().map_err(|_| "sftp-transfer-lock")?;
        let entry = entries
            .get(id)
            .filter(|entry| {
                entry.session == session && !entry.started && entry.expires > Instant::now()
            })
            .ok_or("sftp-transfer-expired")?;
        Ok(entry.cancel.subscribe())
    }
    pub(crate) fn reserve(&self, session: &str) -> Result<String, String> {
        let mut entries = self.0.lock().map_err(|_| "sftp-transfer-lock")?;
        entries.retain(|_, entry| {
            entry.started || (entry.expires > Instant::now() && !*entry.cancel.borrow())
        });
        if entries.len() >= 32 || entries.values().filter(|e| e.session == session).count() >= 4 {
            return Err("sftp-transfer-limit".into());
        }
        let id = format!("xfer-{}", uuid::Uuid::new_v4());
        entries.insert(
            id.clone(),
            Entry {
                session: session.into(),
                cancel: watch::channel(false).0,
                started: false,
                expires: Instant::now() + Duration::from_secs(300),
            },
        );
        Ok(id)
    }

    pub(crate) fn start(&self, session: &str, id: &str) -> Result<Transfer, String> {
        let mut entries = self.0.lock().map_err(|_| "sftp-transfer-lock")?;
        let entry = entries
            .get_mut(id)
            .filter(|e| e.session == session && !e.started && e.expires > Instant::now())
            .ok_or("sftp-transfer-expired")?;
        entry.started = true;
        Ok(Transfer {
            registry: self.clone(),
            id: id.into(),
            cancelled: entry.cancel.subscribe(),
        })
    }

    pub(crate) fn cancel(&self, session: &str, id: &str) -> Result<(), String> {
        let entries = self.0.lock().map_err(|_| "sftp-transfer-lock")?;
        let entry = entries
            .get(id)
            .filter(|e| e.session == session)
            .ok_or("sftp-transfer-expired")?;
        entry.cancel.send_replace(true);
        Ok(())
    }

    pub(crate) fn disconnect(&self, session: Option<&str>) {
        if let Ok(mut entries) = self.0.lock() {
            entries.retain(|_, entry| {
                if session.is_none_or(|session| session == entry.session) {
                    entry.cancel.send_replace(true);
                    false
                } else {
                    true
                }
            });
        }
    }
}

pub(crate) struct Transfer {
    registry: Transfers,
    id: String,
    cancelled: watch::Receiver<bool>,
}

impl Transfer {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn check(&self) -> Result<(), String> {
        if *self.cancelled.borrow() {
            Err("sftp-transfer-cancelled".into())
        } else {
            Ok(())
        }
    }

    pub(crate) async fn run<T>(
        &mut self,
        operation: impl Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        until_cancelled(&mut self.cancelled, operation).await
    }
}

pub(crate) async fn until_cancelled<T>(
    cancelled: &mut watch::Receiver<bool>,
    operation: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    if *cancelled.borrow() {
        return Err("sftp-transfer-cancelled".into());
    }
    tokio::select! {
        biased;
        _ = cancelled.changed() => Err("sftp-transfer-cancelled".into()),
        result = operation => result,
    }
}

impl Drop for Transfer {
    fn drop(&mut self) {
        if let Ok(mut entries) = self.registry.0.lock() {
            entries.remove(&self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn early_cancel_is_latched_and_cannot_cross_sessions_or_replay() {
        let transfers = Transfers::default();
        let id = transfers.reserve("old-session").unwrap();
        let mut checking = transfers.observer("old-session", &id).unwrap();
        assert!(transfers.observer("new-session", &id).is_err());
        assert!(transfers.cancel("new-session", &id).is_err());
        assert!(transfers.start("new-session", &id).is_err());
        transfers.cancel("old-session", &id).unwrap();
        assert_eq!(
            until_cancelled(&mut checking, async { Ok(()) })
                .await
                .unwrap_err(),
            "sftp-transfer-cancelled"
        );
        let mut transfer = transfers.start("old-session", &id).unwrap();
        assert!(transfers.start("old-session", &id).is_err());
        let mut ran = false;
        assert!(transfer
            .run(async {
                ran = true;
                Ok(())
            })
            .await
            .is_err());
        assert!(!ran);
        drop(transfer);
        assert!(transfers.start("old-session", &id).is_err());
    }

    #[tokio::test]
    async fn cancellation_unblocks_waiting_io_and_disconnect_preserves_other_host() {
        let transfers = Transfers::default();
        let a = transfers.reserve("a").unwrap();
        let b = transfers.reserve("b").unwrap();
        let mut running = transfers.start("a", &a).unwrap();
        let other = transfers.start("b", &b).unwrap();
        let finished = tokio::spawn(async move {
            running
                .run(std::future::pending::<Result<(), String>>())
                .await
        });
        transfers.disconnect(Some("a"));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), finished)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err(),
            "sftp-transfer-cancelled"
        );
        assert!(other.check().is_ok());
        transfers.disconnect(None);
        assert!(other.check().is_err());
    }

    #[test]
    fn reservations_are_bounded_and_release_the_slot() {
        let transfers = Transfers::default();
        let ids: Vec<_> = (0..4).map(|_| transfers.reserve("a").unwrap()).collect();
        assert!(transfers.reserve("a").is_err());
        drop(transfers.start("a", &ids[0]).unwrap());
        assert!(transfers.reserve("a").is_ok());
        transfers
            .0
            .lock()
            .unwrap()
            .values_mut()
            .for_each(|e| e.expires = Instant::now());
        assert!(transfers.start("a", &ids[1]).is_err());
        assert!(transfers.reserve("a").is_ok());
    }
}
