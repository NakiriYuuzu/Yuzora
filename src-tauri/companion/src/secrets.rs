//! Fixed-width CSPRNG secrets. No Debug and no logging of their encoded forms.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::TryRng;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

pub fn random<const N: usize>() -> Result<Zeroizing<[u8; N]>, &'static str> {
    let mut bytes = Zeroizing::new([0; N]);
    rand::rngs::SysRng
        .try_fill_bytes(bytes.as_mut())
        .map_err(|_| "entropy-unavailable")?;
    Ok(bytes)
}
pub fn encode(value: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(value)
}
pub fn hash(value: &[u8]) -> [u8; 32] {
    Sha256::digest(value).into()
}
pub fn matches(expected: &[u8; 32], supplied: &[u8]) -> bool {
    bool::from(expected.ct_eq(&hash(supplied)))
}
