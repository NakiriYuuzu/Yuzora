//! Source endpoint and an owned route supplied by the desktop.
use std::net::SocketAddr;

pub trait DatabaseRoute: Send + Sync {
    fn address(&self) -> SocketAddr;
    fn is_current(&self) -> bool;
}

/// Only the private DB worker pipe carries this endpoint. The frontend selects
/// a stable host ID; Rust owns the tunnel and its loopback address.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DriverEndpoint {
    pub host: String,
    pub port: u16,
    pub connect_address: Option<SocketAddr>,
}
impl DriverEndpoint {
    pub fn direct(host: String, port: u16) -> Self {
        Self {
            host,
            port,
            connect_address: None,
        }
    }
    pub fn postgres_config(&self) -> tokio_postgres::Config {
        let mut config = tokio_postgres::Config::new();
        config.host(&self.host);
        if let Some(address) = self.connect_address {
            config.hostaddr(address.ip()).port(address.port());
        } else {
            config.port(self.port);
        }
        config
    }
    pub async fn connect_tcp(&self) -> std::io::Result<tokio::net::TcpStream> {
        match self.connect_address {
            Some(address) => tokio::net::TcpStream::connect(address).await,
            None => tokio::net::TcpStream::connect((self.host.as_str(), self.port)).await,
        }
    }
}
