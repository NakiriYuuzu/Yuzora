"""uv run --with cryptography spikes/companion-direct/lan/make-ca.py [name]
Throwaway keys only. Never install this CA on the desktop trust store.
"""
import argparse
import datetime as dt
import ipaddress
import os
from pathlib import Path
import plistlib
import re
import subprocess
import sys
import uuid

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import ExtendedKeyUsageOID, NameOID

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("name", nargs="?")
parser.add_argument("--counterexample-ip", help="Explicit RFC1918 IPv4 for a phone IP-SAN negative test")
args = parser.parse_args()
phone_ip = None
if args.counterexample_ip:
    try:
        phone_ip = ipaddress.IPv4Address(args.counterexample_ip)
    except ipaddress.AddressValueError:
        parser.error("--counterexample-ip must be an RFC1918 IPv4 address")
    if not any(phone_ip in ipaddress.IPv4Network(net) for net in ("10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16")):
        parser.error("--counterexample-ip must be an RFC1918 IPv4 address")
name = args.name
if name is None:
    if sys.platform != "darwin":
        parser.error("Pass the local mDNS host label on non-macOS")
    name = subprocess.check_output(["scutil", "--get", "LocalHostName"], text=True).strip()
if not re.fullmatch(r"[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?", name):
    parser.error("Expected a single DNS host label (without .local)")
host = name.lower() + ".local"
if host == "other.local":
    parser.error("other.local is reserved for the counterexample")
os.umask(0o077)
out = Path(__file__).resolve().parent / "out"
out.mkdir(exist_ok=True, mode=0o700)
if any(out.iterdir()):
    parser.error("out/ is not empty; archive/remove prior spike keys explicitly first")
now = dt.datetime.now(dt.timezone.utc)
key = ec.generate_private_key(ec.SECP256R1())
subject = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, f"Yuzora spike CA ({host})")])
ca = (x509.CertificateBuilder().subject_name(subject).issuer_name(subject)
      .public_key(key.public_key()).serial_number(x509.random_serial_number())
      .not_valid_before(now - dt.timedelta(minutes=5)).not_valid_after(now + dt.timedelta(days=730))
      .add_extension(x509.BasicConstraints(ca=True, path_length=0), critical=True)
      .add_extension(x509.KeyUsage(False, False, False, False, False, True, True, None, None), critical=True)
      .add_extension(x509.NameConstraints(permitted_subtrees=[x509.DNSName(host)], excluded_subtrees=[
          x509.IPAddress(ipaddress.IPv4Network("0.0.0.0/0")),
          x509.IPAddress(ipaddress.IPv6Network("::/0")),
      ]), critical=True)
      .add_extension(x509.SubjectKeyIdentifier.from_public_key(key.public_key()), critical=False)
      .sign(key, hashes.SHA256()))

def write_cert(label, cert, private_key=None):
    (out / f"{label}.pem").write_bytes(cert.public_bytes(serialization.Encoding.PEM))
    if private_key is not None:
        (out / f"{label}.key").write_bytes(private_key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
    print(f"{label} SHA-256: {cert.fingerprint(hashes.SHA256()).hex(':').upper()}")

# The CA signing key stays in memory and is never serialized.
write_cert("ca", ca)
leaves = [("leaf", x509.DNSName(host)), ("other", x509.DNSName("other.local")),
          ("ip", x509.IPAddress(ipaddress.IPv4Address("127.0.0.1")))]
if phone_ip is not None:
    leaves.append(("phone-ip", x509.IPAddress(phone_ip)))
for label, san in leaves:
    leaf_key = ec.generate_private_key(ec.SECP256R1())
    cert = (x509.CertificateBuilder().subject_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, str(san.value))]))
            .issuer_name(subject).public_key(leaf_key.public_key()).serial_number(x509.random_serial_number())
            .not_valid_before(now - dt.timedelta(minutes=5)).not_valid_after(now + dt.timedelta(days=90))
            .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
            .add_extension(x509.SubjectAlternativeName([san]), critical=False)
            .add_extension(x509.ExtendedKeyUsage([ExtendedKeyUsageOID.SERVER_AUTH]), critical=False)
            .add_extension(x509.KeyUsage(True, False, False, False, False, False, False, None, None), critical=True)
            .add_extension(x509.AuthorityKeyIdentifier.from_issuer_public_key(key.public_key()), critical=False)
            .sign(key, hashes.SHA256()))
    write_cert(label, cert, leaf_key)
der = ca.public_bytes(serialization.Encoding.DER)
(out / "ca.crt").write_bytes(der)
payload = {"PayloadType": "com.apple.security.root", "PayloadVersion": 1,
           "PayloadIdentifier": "dev.yuzora.spike.ca", "PayloadUUID": str(uuid.uuid4()),
           "PayloadDisplayName": f"Yuzora spike CA ({host})", "PayloadContent": der}
profile = {"PayloadType": "Configuration", "PayloadVersion": 1,
           "PayloadIdentifier": "dev.yuzora.spike", "PayloadUUID": str(uuid.uuid4()),
           "PayloadDisplayName": f"Yuzora Companion spike ({host})", "PayloadContent": [payload]}
(out / "ca.mobileconfig").write_bytes(plistlib.dumps(profile))
(out / "hostname.txt").write_text(host + "\n")
print(f"HTTPS / RP ID: {host}; output: {out}")
print("DNS nameConstraints is a subtree: also permits subdomains; all IPv4/IPv6 SANs excluded.")
print("ca.key is never created; the CA private key stays in memory only. Removing out/ and re-running generates a NEW CA and fingerprint.")
