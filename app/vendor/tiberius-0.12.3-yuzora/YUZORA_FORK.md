# Yuzora tiberius fork

- Upstream crate: `tiberius` 0.12.3 from crates.io.
- Upstream checksum: `a1446cb4198848d1562301a3340424b4f425ef79f35ef9ee034769a9dd92c10d`.
- Scope authority: database reliability plan Amendment A3 / P4-T5.
- Mechanical baseline: `Cargo.toml`, `Cargo.toml.orig`, licenses, README,
  CHANGELOG, SECURITY, and `src/**` copied byte-for-byte from the locked crate,
  except the hand-modified files listed below.
- Product contract: retain valid TDS DONE-family row counts in wire order while
  draining a query stream. A DONE token without `DONE_COUNT` means no reported
  count; it is not a confirmed zero. MONEY and SMALLMONEY decode from their
  signed scaled integers directly into `Numeric(scale = 4)`, never through a
  floating-point intermediary.

Hand-modified upstream files:

- `src/tds/codec/token/token_done.rs`: expose a crate-private COUNT-aware value.
- `src/tds/stream/query.rs`: retain valid counts across every token-consuming
  path and expose them after EOF without adding a public `QueryItem` variant.
- `src/result.rs`: ignore invalid/unreported DONE counts in `ExecuteResult`.
- `src/tds/codec/column_data/money.rs`: preserve MONEY/SMALLMONEY signed
  fixed-point values exactly, including boundaries and trailing scale.
- `src/tds/codec/token/token_col_metadata.rs`: keep every MONEY/SMALLMONEY null
  metadata path in the same exact `Numeric` representation.
- `Cargo.toml` (`Cargo.toml.orig` left as upstream; cargo does not read it): the
  `rustls` feature moves from tokio-rustls 0.24 / rustls 0.21 / rustls-pemfile 1 /
  rustls-native-certs 0.6 to tokio-rustls 0.26 (`ring` + `tls12`, default features
  off, so no aws-lc) / rustls-pki-types / rustls-native-certs 0.8. This removes
  the rustls 0.21, rustls-webpki 0.101, sct and rustls-pemfile 1.0.4 advisory chain.
- `src/client/tls_stream/rustls_tls_stream.rs`: port to the rustls 0.23 API pinned
  to the ring provider (`builder_with_provider`, no process-default provider).
  Trust-all mode skips chain validation and handshake-signature verification:
  webpki-based verification rejects the X.509 v1 self-signed certificates SQL
  Server generates. rustls 0.21 skipped TLS 1.2 signature checks here too but
  still verified TLS 1.3 ones; skipping both is acceptable because the
  certificate is unverified in this mode and SQL Server (TDS 7.x) negotiates at
  most TLS 1.2. A platform trust store that yields no usable roots now returns
  a TLS error carrying the load errors instead of panicking. PEM parsing uses rustls-pki-types; native roots use the 0.8 API.

Yuzora drains each MSSQL query stream to EOF, including after row caps and
value/shape errors. It materializes one coherent driver result index; additional
result sets fail closed only after the drain completes. Driver/transport errors
during that drain still return immediately. Its scalar affected-row value is the
checked aggregate of the retained server-reported counts. Trigger/procedure DONE
counts may contribute; this is not claimed to be an outer-DML-only count.
