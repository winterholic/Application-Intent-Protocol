# aip-crypto

Product encryption building blocks for WBS 8.18.1. This crate does not connect encryption to product reads,
writes, migrations, journals, CLI configuration or Infisical. Those remain separate gates in
[the design](../../docs/design/13-field-encryption.md).

- `cell`: AES-256-GCM v2 frames with length-prefixed AAD binding schema, resource, field, row, scope and
  immutable DEK identity/version. Random nonces come from the AEAD library's OS random source. Values are
  limited to 1 MiB; context and frame bounds are checked before decryption. Normal reads accept only v2.
  `parse_header` checks structure, including the scope tag and canonical version; it does not authenticate
  a stored frame or establish that external data is safe to adopt. Use AEAD open with the full trusted context.
- `provider`: async `KekProvider` interface and local env/file KEK rings. HKDF derives wrapping keys;
  rotating the wrapping KEK preserves the DEK and existing data frames. Files must be owned regular files,
  0400/0600, at most 8192 bytes, with no symlink in any component. Unix paths are walked by file descriptors;
  other platforms return unavailable. Remote KMS and hot reload are not implemented.
- `fingerprint`: purpose-separated HKDF/HMAC for idempotency, audit and blind indexes. Replay verifies with
  the stored key ID using the library's constant-time verification. The caller validates/canonicalizes
  scalars and supplies a separate blind-index root ring. These APIs do not implement SQL uniqueness or rotation.
- `cache`: an LRU of at most 4096 DEKs with 1–300 second TTL. A Tokio runtime with time enabled is required.
  Reads enforce expiry immediately; an owned one-second task also evicts idle entries and aborts on drop.
  Eviction releases the cache reference; active borrowers retain material until their final `Arc` is dropped.
- Optional `legacy-import`: explicit offline v1 authentication and resealing to a destination v2 context.
  Enable only for an import tool, never as a server fallback. The operator supplies source field/row and
  destination schema/scope; the v1 format cannot prove the source schema/scope. No plaintext-returning API.

Keys and temporary successful plaintext buffers use `Zeroizing`. This reduces retention and does not prove
all compiler/library copies are wiped or protect a compromised host. Distributed per-DEK invocation limits,
writer fencing, metadata transactions, startup key checks and operational rotation/backup policy belong to
8.18.3–8.18.5 and must pass before this library is used for production storage.
`aip-execution::encryption::DekStore` now supplies the metadata transaction foundation and migration-role
provisioning. Its writer lookup selects ACTIVE and holds shared scope/key locks in the caller's
transaction, but resource writes, usage budgets and operational rekey are not connected. This metadata
primitive does not remove the product's encryption execution gate.

Validation: `cargo test -p aip-crypto --all-features --locked` and
`cargo clippy -p aip-crypto --all-targets --all-features --locked -- -D warnings`. Tests include a NIST
AES-256-GCM vector, RFC 5869 vector, AAD/frame tampering, KEK rewrap, retained fingerprint keys, file
permissions/symlinks, cache eviction/expiry/borrowers and explicit v1 import. Passing tests are not a security audit.
