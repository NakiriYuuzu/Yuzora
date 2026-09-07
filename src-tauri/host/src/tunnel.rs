//! A dedicated raw byte lane after one versioned, owned stdio handshake.
//! Used by WSL, which does not need an SSH daemon or a TCP control service.
use crate::protocol::ConnectionOwner;
#[cfg(unix)]
use crate::protocol::{Outcome, Response, PROTOCOL_VERSION};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::time::Duration;
#[cfg(unix)]
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}
impl Endpoint {
    pub fn validate(&self) -> Result<(), String> {
        if self.port == 0
            || self.host.is_empty()
            || self.host.len() > 253
            || !self
                .host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".-:".contains(&byte))
        {
            return Err("invalid-tunnel-endpoint".into());
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TunnelRequest {
    pub version: u32,
    pub id: String,
    pub owner: ConnectionOwner,
    pub endpoint: Endpoint,
}

#[cfg(unix)]
pub async fn serve<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    input: R,
    mut output: W,
) -> Result<(), String> {
    let mut input = BufReader::new(input);
    let bytes = tokio::time::timeout(Duration::from_secs(10), crate::wire::read_frame(&mut input))
        .await
        .map_err(|_| "tunnel-handshake-timeout")??
        .ok_or("tunnel-closed")?;
    let request: TunnelRequest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if request.version != PROTOCOL_VERSION
        || request.id.is_empty()
        || request.id.len() > 128
        || request.owner.host_id.is_empty()
        || request.owner.host_id.len() > 256
    {
        return Err("invalid-tunnel-handshake".into());
    }
    request.endpoint.validate()?;
    let remote = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::TcpStream::connect((request.endpoint.host.as_str(), request.endpoint.port)),
    )
    .await
    .map_err(|_| "tunnel-connect-timeout")?
    .map_err(|e| e.to_string());
    let outcome = match &remote {
        Ok(_) => Outcome::Ok {
            value: serde_json::Value::Null,
        },
        Err(message) => Outcome::Error {
            code: "tunnel-connect-failed".into(),
            message: message.clone(),
        },
    };
    let mut response = serde_json::to_vec(&Response {
        version: PROTOCOL_VERSION,
        id: request.id,
        owner: request.owner,
        outcome,
    })
    .map_err(|e| e.to_string())?;
    response.push(b'\n');
    tokio::time::timeout(Duration::from_secs(10), output.write_all(&response))
        .await
        .map_err(|_| "tunnel-write-timeout")?
        .map_err(|e| e.to_string())?;
    output.flush().await.map_err(|e| e.to_string())?;
    let mut remote = remote?;
    let (mut read, mut write) = remote.split();
    // Tokio's copy uses fixed-size buffers, and preserves TCP half-close.
    let upstream = async {
        tokio::io::copy(&mut input, &mut write).await?;
        write.shutdown().await
    };
    let downstream = async {
        tokio::io::copy(&mut read, &mut output).await?;
        output.shutdown().await
    };
    tokio::try_join!(upstream, downstream).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[tokio::test]
    async fn handshake_preserves_binary_payload_and_half_close() {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Endpoint {
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
        };
        let peer = tokio::spawn(async move {
            let (mut peer, _) = listener.accept().await.unwrap();
            peer.write_all(b"banner\0").await.unwrap();
            let mut body = Vec::new();
            peer.read_to_end(&mut body).await.unwrap();
            peer.write_all(&body).await.unwrap();
        });
        let (mut input, reader) = tokio::io::duplex(1024);
        let (writer, output) = tokio::io::duplex(1024);
        let helper = tokio::spawn(serve(reader, writer));
        let owner = ConnectionOwner {
            host_id: "wsl-fixture".into(),
            generation: 42,
        };
        let request = TunnelRequest {
            version: PROTOCOL_VERSION,
            id: "raw".into(),
            owner: owner.clone(),
            endpoint,
        };
        let payload = b"request\0\xff\nnot-json";
        let mut bytes = serde_json::to_vec(&request).unwrap();
        bytes.push(b'\n');
        bytes.extend(payload);
        input.write_all(&bytes).await.unwrap();
        input.shutdown().await.unwrap();
        let mut output = BufReader::new(output);
        let bytes = crate::wire::read_frame(&mut output).await.unwrap().unwrap();
        let response: Response = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(response.owner, owner);
        assert_eq!(response.id, "raw");
        assert!(matches!(response.outcome, Outcome::Ok { .. }));
        let mut body = Vec::new();
        tokio::time::timeout(Duration::from_secs(3), output.read_to_end(&mut body))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(body, [b"banner\0".as_slice(), payload.as_slice()].concat());
        helper.await.unwrap().unwrap();
        peer.await.unwrap();
    }
    #[test]
    fn endpoints_accept_dns_ipv4_ipv6_and_reject_non_host_input() {
        for host in ["localhost", "127.0.0.1", "::1", "db.internal"] {
            Endpoint {
                host: host.into(),
                port: 5432,
            }
            .validate()
            .unwrap();
        }
        for host in [
            "",
            "db:5432/path",
            "x\0y",
            "a b",
            "-o ProxyCommand=x",
            "[::1]",
        ] {
            assert!(Endpoint {
                host: host.into(),
                port: 5432
            }
            .validate()
            .is_err());
        }
        assert!(Endpoint {
            host: "localhost".into(),
            port: 0
        }
        .validate()
        .is_err());
    }
}
