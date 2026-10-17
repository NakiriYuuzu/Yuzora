//! Test-only software authenticator. Builds fixtures; never verifies credentials.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use ciborium::value::Value as C;
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use rand08::SeedableRng;
use rsa::{pkcs8::EncodePublicKey, signature::SignatureEncoding, traits::PublicKeyParts};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
const HOST: &str = "desktop.tailnet.ts.net";
const ORIGIN: &str = "https://desktop.tailnet.ts.net:8443";
fn cbor(value: C) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&value, &mut bytes).unwrap();
    bytes
}
fn int(n: i64) -> C {
    C::Integer(n.into())
}
enum Key {
    Es(SigningKey),
    Ed(ed25519_dalek::SigningKey),
    Rsa(Box<rsa::RsaPrivateKey>),
}
pub struct Authenticator {
    key: Key,
    pub id: String,
    be: bool,
}
impl Authenticator {
    pub fn new(algorithm: &str, byte: u8, be: bool) -> Self {
        let key = match algorithm {
            "ES256" => Key::Es(SigningKey::from_bytes((&[byte; 32]).into()).unwrap()),
            "EdDSA" => Key::Ed(ed25519_dalek::SigningKey::from_bytes(&[byte; 32])),
            _ => Key::Rsa(Box::new(
                rsa::RsaPrivateKey::new(&mut rand08::rngs::StdRng::from_seed([byte; 32]), 2048)
                    .unwrap(),
            )),
        };
        Self {
            key,
            id: B64.encode([byte; 16]),
            be,
        }
    }
    fn cose(&self) -> Vec<u8> {
        let pairs = match &self.key {
            Key::Es(key) => {
                let p = key.verifying_key().to_encoded_point(false);
                vec![
                    (int(1), int(2)),
                    (int(3), int(-7)),
                    (int(-1), int(1)),
                    (int(-2), C::Bytes(p.x().unwrap().to_vec())),
                    (int(-3), C::Bytes(p.y().unwrap().to_vec())),
                ]
            }
            Key::Ed(key) => vec![
                (int(1), int(1)),
                (int(3), int(-8)),
                (int(-1), int(6)),
                (int(-2), C::Bytes(key.verifying_key().to_bytes().to_vec())),
            ],
            Key::Rsa(key) => vec![
                (int(1), int(3)),
                (int(3), int(-257)),
                (int(-1), C::Bytes(key.n().to_bytes_be())),
                (int(-2), C::Bytes(key.e().to_bytes_be())),
            ],
        };
        cbor(C::Map(pairs))
    }
    pub fn response(&self, options: &Value, register: bool, count: u32, case: &str) -> Value {
        let typ = if register {
            "webauthn.create"
        } else {
            "webauthn.get"
        };
        let mut client = json!({"type": if case=="type" {if register {"webauthn.get"} else {"webauthn.create"}} else {typ}, "challenge":if case=="challenge" {json!(B64.encode([0;16]))} else {options["challenge"].clone()}, "origin":if case=="origin" {"https://desktop.tailnet.ts.net:9443"} else {ORIGIN},"crossOrigin":case=="cross"});
        if case == "top" {
            client["topOrigin"] = json!("https://other.ts.net");
        }
        let client = serde_json::to_vec(&client).unwrap();
        let mut flags = 5 | if register { 64 } else { 0 } | if self.be { 24 } else { 0 };
        if self.be && case == "eligible" {
            flags &= !16;
        }
        if case == "uv" {
            flags &= !4;
        }
        if case == "up" {
            flags &= !1;
        }
        if case == "bs" {
            flags = 21 | if register { 64 } else { 0 };
        }
        let mut ad = Sha256::digest(if case == "rp" {
            b"other.ts.net".as_slice()
        } else {
            HOST.as_bytes()
        })
        .to_vec();
        if case == "extension" {
            flags |= 128;
        }
        ad.push(flags);
        ad.extend(count.to_be_bytes());
        if case == "extension" {
            ad.extend(cbor(C::Map(vec![(
                C::Text("hmac-secret".into()),
                C::Bytes(vec![0; 48]),
            )])));
        }
        let response = if register {
            ad.extend([0; 16]);
            ad.extend(16u16.to_be_bytes());
            ad.extend(B64.decode(&self.id).unwrap());
            ad.extend(self.cose());
            let att = cbor(C::Map(vec![
                (C::Text("fmt".into()), C::Text("none".into())),
                (C::Text("attStmt".into()), C::Map(vec![])),
                (C::Text("authData".into()), C::Bytes(ad.clone())),
            ]));
            let (algorithm, public_key) = match &self.key {
                Key::Es(key) => (
                    -7,
                    key.verifying_key()
                        .to_public_key_der()
                        .unwrap()
                        .as_bytes()
                        .to_vec(),
                ),
                Key::Ed(key) => {
                    // RFC 8410 Ed25519 SubjectPublicKeyInfo fixture, not a verifier.
                    let mut der = vec![
                        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0,
                    ];
                    der.extend(key.verifying_key().to_bytes());
                    (-8, der)
                }
                Key::Rsa(key) => (
                    -257,
                    key.to_public_key()
                        .to_public_key_der()
                        .unwrap()
                        .as_bytes()
                        .to_vec(),
                ),
            };
            json!({"clientDataJSON":B64.encode(client),"attestationObject":B64.encode(att),"authenticatorData":B64.encode(ad),"publicKey":B64.encode(public_key),"publicKeyAlgorithm":algorithm,"transports":["usb","nfc","ble","internal","hybrid","smart-card"]})
        } else {
            let mut signed = ad.clone();
            signed.extend(Sha256::digest(&client));
            if case == "signature" {
                signed.push(0);
            }
            let sig = match &self.key {
                Key::Es(k) => {
                    let sig: Signature = k.sign(&signed);
                    sig.to_der().as_bytes().to_vec()
                }
                Key::Ed(k) => k.sign(&signed).to_bytes().to_vec(),
                Key::Rsa(k) => rsa::pkcs1v15::SigningKey::<rsa::sha2::Sha256>::new(*k.clone())
                    .sign(&signed)
                    .to_vec(),
            };
            json!({"clientDataJSON":B64.encode(client),"authenticatorData":B64.encode(ad),"signature":B64.encode(sig),"userHandle":options["user"]["id"]})
        };
        json!({"id":self.id,"rawId":self.id,"type":"public-key","authenticatorAttachment":"cross-platform","clientExtensionResults":{},"response":response})
    }
}
