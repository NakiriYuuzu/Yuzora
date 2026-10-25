/// Bytes 1..=32 as unpadded base64url. Must equal `proxySecret` in
/// `tests/fixtures/init.json`; requests send it as `Yzc-Proxy-Secret`.
pub const PROXY_SECRET: &str = "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA";
