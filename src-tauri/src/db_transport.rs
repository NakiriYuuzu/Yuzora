//! Preserve the source DB endpoint for TLS and policy; change only TCP routing.
use crate::host_service::{HostConnection, HostManager};
use crate::host_tunnels;
use crate::ssh_service::SshManager;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
#[cfg(test)]
use yuzora_host::db_endpoint::DriverEndpoint;
use yuzora_host::tunnel::Endpoint;

pub(crate) struct DatabaseTunnel {
    connection: Arc<HostConnection>,
    resource_owner: String,
    tunnel_id: String,
    address: SocketAddr,
}
impl DatabaseTunnel {
    pub(crate) async fn open(
        hosts: &HostManager,
        ssh: Arc<SshManager>,
        host_id: &str,
        endpoint: Endpoint,
    ) -> Result<Self, String> {
        let connection = hosts.connection_for_host(host_id)?;
        let resource_owner = format!("db:{}", uuid::Uuid::new_v4());
        let opened =
            host_tunnels::open(connection.clone(), ssh, resource_owner.clone(), endpoint).await?;
        let tunnel = Self {
            connection,
            resource_owner,
            tunnel_id: opened.tunnel_id,
            address: (Ipv4Addr::LOCALHOST, opened.local_port).into(),
        };
        hosts.connection(&tunnel.connection.owner)?;
        Ok(tunnel)
    }
    pub(crate) fn address(&self) -> SocketAddr {
        self.address
    }
    pub(crate) fn is_current(&self) -> bool {
        !*self.connection.cancelled.borrow()
    }
}
impl yuzora_host::db_endpoint::DatabaseRoute for DatabaseTunnel {
    fn address(&self) -> SocketAddr {
        self.address()
    }
    fn is_current(&self) -> bool {
        self.is_current()
    }
}
impl Drop for DatabaseTunnel {
    fn drop(&mut self) {
        let _ = host_tunnels::close(&self.connection, &self.resource_owner, &self.tunnel_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires database fixtures and YUZORA_HOST_TEST_BINARY"]
    async fn real_database_workers_use_owned_host_tunnels_and_release_on_drop() {
        use crate::db_query_worker::{NetworkQueryStart, NetworkRow};
        use crate::db_service::{
            open_unregistered_via, DbHandle, DbOpenConfig, DbValue, PostgresTransportMode,
        };
        use crate::host_service::HostTarget;
        let hosts = HostManager::default();
        let ssh = Arc::new(SshManager::for_test());
        let helper = std::env::var("YUZORA_HOST_TEST_BINARY").expect("test helper path");
        let password = std::env::var("YUZORA_P8_DATABASE_PASSWORD").expect("fixture password");
        for (host_id, port) in [("db-tunnel-pg", 55432), ("db-tunnel-mssql", 51433)] {
            let connected = hosts
                .connect(host_id.into(), HostTarget::Local, helper.clone(), &ssh)
                .await
                .unwrap();
            let connection = hosts.connection(&connected.owner).unwrap();
            let config = || {
                if port == 55432 {
                    DbOpenConfig::Postgres {
                        via_host: Some(host_id.into()),
                        host: "127.0.0.1".into(),
                        port,
                        database: "yuzora_p8".into(),
                        user: "yuzora_full".into(),
                        password: password.clone().into(),
                        transport_mode: PostgresTransportMode::EncryptedTrustServerCert,
                        insecure_exception: None,
                        trust_server_cert_acknowledged: true,
                    }
                } else {
                    DbOpenConfig::Mssql {
                        via_host: Some(host_id.into()),
                        host: "127.0.0.1".into(),
                        port,
                        database: "yuzora_p8".into(),
                        user: "yuzora_full".into(),
                        password: password.clone().into(),
                        trust_cert: true,
                    }
                }
            };
            let tunnel = DatabaseTunnel::open(
                &hosts,
                ssh.clone(),
                host_id,
                Endpoint {
                    host: "127.0.0.1".into(),
                    port,
                },
            )
            .await
            .unwrap();
            let handle = open_unregistered_via(config(), Some(Box::new(tunnel)))
                .await
                .unwrap();
            let worker = match &handle {
                DbHandle::Postgres(pg) => pg.worker(),
                DbHandle::Mssql(worker) => worker,
                _ => unreachable!(),
            };
            assert!(matches!(
                worker.start_query("SELECT 1 AS value").await.unwrap(),
                NetworkQueryStart::Rows { .. }
            ));
            assert!(
                matches!(worker.next_row().await.unwrap(), NetworkRow::Value(values) if values == vec![DbValue::Integer { value: "1".into() }])
            );
            assert!(matches!(
                worker.next_row().await.unwrap(),
                NetworkRow::End { .. }
            ));
            assert_eq!(connection.tunnels.lock().unwrap().len(), 1);
            worker.close().await.unwrap();
            drop(handle);
            assert!(connection.tunnels.lock().unwrap().is_empty());

            let tunnel = DatabaseTunnel::open(
                &hosts,
                ssh.clone(),
                host_id,
                Endpoint {
                    host: "127.0.0.1".into(),
                    port,
                },
            )
            .await
            .unwrap();
            let handle = open_unregistered_via(config(), Some(Box::new(tunnel)))
                .await
                .unwrap();
            hosts.disconnect(&connected.owner).await.unwrap();
            let worker = match &handle {
                DbHandle::Postgres(pg) => pg.worker(),
                DbHandle::Mssql(worker) => worker,
                _ => unreachable!(),
            };
            assert!(
                worker.is_closed(),
                "a host disconnect invalidates the DB connection"
            );
            assert!(connection.tunnels.lock().unwrap().is_empty());
            drop(handle);
        }
    }

    #[tokio::test]
    async fn tunnel_address_avoids_local_dns_and_preserves_tls_hostname() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = DriverEndpoint {
            host: "database.only-on-source.invalid".into(),
            port: 5432,
            connect_address: Some(listener.local_addr().unwrap()),
        };
        let config = endpoint.postgres_config();
        assert_eq!(
            config.get_hosts(),
            &[tokio_postgres::config::Host::Tcp(endpoint.host.clone())]
        );
        assert_eq!(
            config.get_hostaddrs(),
            &[listener.local_addr().unwrap().ip()]
        );
        assert_eq!(config.get_ports(), &[listener.local_addr().unwrap().port()]);
        assert_eq!(endpoint.port, 5432, "policy retains the source port");
        let socket = endpoint.connect_tcp().await.unwrap();
        assert_eq!(socket.peer_addr().unwrap(), listener.local_addr().unwrap());
    }
}
