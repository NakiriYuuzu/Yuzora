# Yuzora tiberius fork

- Upstream crate: `tiberius` 0.12.3 from crates.io.
- Upstream checksum: `a1446cb4198848d1562301a3340424b4f425ef79f35ef9ee034769a9dd92c10d`.
- Scope authority: database reliability plan Amendment A3 / P4-T5.
- Mechanical baseline: `Cargo.toml`, `Cargo.toml.orig`, licenses, README,
  CHANGELOG, SECURITY, and `src/**` copied byte-for-byte from the locked crate.
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

Yuzora drains each MSSQL query stream to EOF, including after row caps and
value/shape errors. It materializes one coherent driver result index; additional
result sets fail closed only after the drain completes. Driver/transport errors
during that drain still return immediately. Its scalar affected-row value is the
checked aggregate of the retained server-reported counts. Trigger/procedure DONE
counts may contribute; this is not claimed to be an outer-DML-only count.
