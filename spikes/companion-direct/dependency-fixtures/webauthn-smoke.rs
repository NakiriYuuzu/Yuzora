// Throwaway software authenticator: constructs/signs fixtures; RP verification is
// exclusively webauthn_rp. Fixed test-only key, never use in production.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use ciborium::value::Value as C;
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::num::NonZeroU32;
use webauthn_rp::{
    AuthenticatedCredential, DiscoverableAuthentication64, DiscoverableCredentialRequestOptions,
    PublicKeyCredentialCreationOptions, Registration,
    request::{AsciiDomain, RpId, register::{UserHandle64, RegistrationVerificationOptions, ResidentKeyRequirement}, auth::AuthenticationVerificationOptions},
    response::register::{CompressedPubKey, CompressedP256PubKey, StaticState, UncompressedPubKey},
};
const HOST: &str = "yuzora-spike.local";
const ORIGIN: &str = "https://yuzora-spike.local:47443";
fn cbor(value: C) -> Vec<u8> { let mut v = vec![]; ciborium::ser::into_writer(&value, &mut v).unwrap(); v }
fn int(n: i64) -> C { C::Integer(n.into()) }
fn client(typ: &str, challenge: &Value, origin: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"type":typ,"challenge":challenge,"origin":origin,"crossOrigin":false})).unwrap()
}
fn auth_data(host: &str, flags: u8, counter: u32) -> Vec<u8> {
    let mut a = Sha256::digest(host.as_bytes()).to_vec(); a.push(flags); a.extend(counter.to_be_bytes()); a
}
fn parse_registration(bytes: &[u8]) -> Result<Registration, serde_json::Error> {
 serde_json::from_slice::<webauthn_rp::response::register::ser_relaxed::RegistrationRelaxed>(bytes).map(|r|r.0)
}
fn parse_authentication(bytes: &[u8]) -> Result<DiscoverableAuthentication64, serde_json::Error> {
 serde_json::from_slice::<webauthn_rp::response::auth::ser_relaxed::DiscoverableAuthenticationRelaxed64>(bytes).map(|r|r.0)
}
fn main() {
    rustls::crypto::ring::default_provider().install_default().unwrap();
    let key = SigningKey::from_bytes((&[7u8;32]).into()).unwrap();
    let point = key.verifying_key().to_encoded_point(false);
    let cose = cbor(C::Map(vec![(int(1),int(2)), (int(3),int(-7)), (int(-1),int(1)), (int(-2),C::Bytes(point.x().unwrap().to_vec())), (int(-3),C::Bytes(point.y().unwrap().to_vec()))]));
    let rp = RpId::Domain(AsciiDomain::try_from(HOST.to_owned()).unwrap());
    let user = UserHandle64::new();
    // Registration positives + negative flags and bindings.
    for case in ["valid", "wrong-origin", "wrong-rp", "no-uv", "no-up", "wrong-challenge", "bs-without-be"] {
        let mut opts = PublicKeyCredentialCreationOptions::first_passkey_with_blank_user_info(&rp, &user);
        opts.timeout = NonZeroU32::new(120_000).unwrap();
        opts.authenticator_selection.resident_key = ResidentKeyRequirement::Preferred;
        opts.extensions = Default::default();
        let (server, request) = opts.start_ceremony().unwrap();
        let request = serde_json::to_value(request).unwrap();
        let challenge = if case == "wrong-challenge" { json!(B64.encode([0u8;16])) } else { request["challenge"].clone() };
        let cd = client("webauthn.create", &challenge, if case == "wrong-origin" { "https://evil.local:47443" } else { ORIGIN });
        let flags = match case { "no-uv" => 0x41, "no-up" => 0x44, "bs-without-be" => 0x55, _ => 0x45 };
        let mut ad = auth_data(if case == "wrong-rp" { "other.local" } else { HOST }, flags, 10);
        ad.extend([0u8;16]); ad.extend(16u16.to_be_bytes()); ad.extend([9u8;16]); ad.extend(&cose);
        let att = cbor(C::Map(vec![(C::Text("fmt".into()),C::Text("none".into())), (C::Text("attStmt".into()),C::Map(vec![])), (C::Text("authData".into()),C::Bytes(ad))]));
        let response = json!({"id": B64.encode([9u8;16]), "rawId": B64.encode([9u8;16]), "type":"public-key", "authenticatorAttachment":"platform", "clientExtensionResults":{}, "response":{"clientDataJSON":B64.encode(&cd),"attestationObject":B64.encode(att),"transports":["internal"]}});
        let parsed = parse_registration(&serde_json::to_vec(&response).unwrap());
        let verification = RegistrationVerificationOptions::<&str,&str> { allowed_origins: &[ORIGIN], ..Default::default() };
        let reg = match parsed { Ok(ref r) => server.verify(&rp, r, &verification).map(|_| ()), Err(e) => { println!("registration {case}: rejected parse {e:?}"); assert_ne!(case,"valid"); continue; } };
        println!("registration {case}: {reg:?}");
        assert_eq!(reg.is_ok(), case == "valid");
        if case != "valid" { continue; }
        // Fresh successful registration for credential-backed assertion tests.
    }
    let mut opts = PublicKeyCredentialCreationOptions::first_passkey_with_blank_user_info(&rp, &user);
    opts.extensions = Default::default();
    let (server, request) = opts.start_ceremony().unwrap();
    let req = serde_json::to_value(request).unwrap();
    let cd = client("webauthn.create", &req["challenge"], ORIGIN);
    let mut ad = auth_data(HOST, 0x45, 10); ad.extend([0u8;16]); ad.extend(16u16.to_be_bytes()); ad.extend([9u8;16]); ad.extend(&cose);
    let att = cbor(C::Map(vec![(C::Text("fmt".into()),C::Text("none".into())), (C::Text("attStmt".into()),C::Map(vec![])), (C::Text("authData".into()),C::Bytes(ad))]));
    let r = parse_registration(&serde_json::to_vec(&json!({"id":B64.encode([9u8;16]),"rawId":B64.encode([9u8;16]),"type":"public-key","authenticatorAttachment":"platform","clientExtensionResults":{},"response":{"clientDataJSON":B64.encode(cd),"attestationObject":B64.encode(att),"transports":["internal"]}})).unwrap()).unwrap();
    let registered = server.verify(&rp, &r, &RegistrationVerificationOptions::<&str,&str>{allowed_origins:&[ORIGIN],..Default::default()}).unwrap();
    println!("registered flags/state: {:?}", registered.dynamic_state());
    for case in ["valid", "wrong-origin", "wrong-rp", "no-uv", "no-up", "wrong-challenge", "counter-rollback", "bad-signature", "be-change", "bs-without-be"] {
        let mut opts = DiscoverableCredentialRequestOptions::passkey(&rp);
        opts.0.timeout = NonZeroU32::new(120_000).unwrap();
        let (server, request) = opts.start_ceremony().unwrap();
        let req = serde_json::to_value(request).unwrap();
        let challenge = if case=="wrong-challenge" {json!(B64.encode([0u8;16]))} else {req["challenge"].clone()};
        let cd = client("webauthn.get", &challenge, if case=="wrong-origin" {"https://evil.local:47443"} else {ORIGIN});
        let flags = match case {"no-uv"=>1,"no-up"=>4,"be-change"=>13,"bs-without-be"=>21,_=>5};
        let ad = auth_data(if case=="wrong-rp" {"other.local"} else {HOST}, flags, if case=="counter-rollback" {9} else {11});
        let mut signed = ad.clone(); signed.extend(Sha256::digest(&cd));
        if case=="bad-signature" { signed.push(0); }
        let sig: Signature = key.sign(&signed);
        let response = json!({"id":B64.encode([9u8;16]),"rawId":B64.encode([9u8;16]),"type":"public-key","authenticatorAttachment":"platform","clientExtensionResults":{},"response":{"clientDataJSON":B64.encode(cd),"authenticatorData":B64.encode(ad),"signature":B64.encode(sig.to_der()),"userHandle":serde_json::to_value(&user).unwrap()}});
        let parsed = parse_authentication(&serde_json::to_vec(&response).unwrap());
        let s = registered.static_state();
        let UncompressedPubKey::P256(k) = s.credential_public_key else {panic!()};
        let pk: CompressedPubKey<&[u8],&[u8],&[u8],&[u8]> = CompressedPubKey::P256(CompressedP256PubKey::try_from((k.x(),k.y_is_odd())).unwrap());
        let mut cred = AuthenticatedCredential::new(registered.id(), registered.user_id(), StaticState{credential_public_key:pk,extensions:s.extensions}, registered.dynamic_state()).unwrap();
        let verified = match parsed {Ok(ref a)=>server.verify(&rp,a,&mut cred,&AuthenticationVerificationOptions::<&str,&str>{allowed_origins:&[ORIGIN],..Default::default()}),Err(e)=>{assert_ne!(case,"valid");println!("authentication {case}: rejected parse {e:?}");continue;}};
        println!("authentication {case}: {verified:?}");
        assert_eq!(verified.is_ok(),case=="valid");
    }
    println!("PASS: ES256 software registration/authentication and negative checks; ring provider installed");
}
