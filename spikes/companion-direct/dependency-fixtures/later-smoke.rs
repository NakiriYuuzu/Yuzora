use rcgen::{CertificateParams, IsCa, BasicConstraints, KeyUsagePurpose, NameConstraints, GeneralSubtree, KeyPair};
fn main() {
 let mut ca=CertificateParams::default();
 ca.is_ca=IsCa::Ca(BasicConstraints::Constrained(0));
 ca.key_usages=vec![KeyUsagePurpose::KeyCertSign,KeyUsagePurpose::CrlSign];
 ca.name_constraints=Some(NameConstraints{permitted_subtrees:vec![GeneralSubtree::DnsName("yuzora-spike.local".into())],excluded_subtrees:vec![]});
 let key=KeyPair::generate().unwrap();
 let cert=ca.self_signed(&key).unwrap();
 std::fs::write("/tmp/yzc-deps/rcgen-ca.der",cert.der()).unwrap();
 let svg=qrcode::QrCode::new(b"https://example.invalid/spike").unwrap().render::<qrcode::render::svg::Color>().build();
 assert!(svg.contains("<svg"));
 let vapid=web_push_native::jwt_simple::algorithms::ES256KeyPair::from_bytes(&[7u8;32]).unwrap();
 let peer=web_push_native::p256::SecretKey::from_slice(&[8u8;32]).unwrap().public_key();
 let request=web_push_native::WebPushBuilder::new("https://example.invalid/push".parse().unwrap(),peer,web_push_native::Auth::clone_from_slice(&[9u8;16])).with_vapid(&vapid,"mailto:spike@example.invalid").build("spike").unwrap();
 assert_eq!(request.headers()["content-encoding"],"aes128gcm");
 assert!(request.headers()["authorization"].to_str().unwrap().starts_with("vapid "));
 println!("PASS: rcgen CA generated; qrcode SVG; Web Push aes128gcm + VAPID request built offline (not sent)");
}
