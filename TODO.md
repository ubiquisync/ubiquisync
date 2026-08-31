# TODO

Known gaps and pending decisions, mostly surfaced while porting from the source
codebase. Items here should graduate to GitHub issues (or get fixed) rather
than accumulate.

## Protocol design pass (July 2026) — put things in the right places

File fixes:

- [ ] `init.rs`: add `version: u16` as first field
- [ ] `header.rs`: fix `key_fingeprint` typo; add missing imports
- [ ] `ctl/op.rs`: doc-comment `CommitInfo.id` dual use (container id vs device
  id); `ObservePeers` refs are ctl-log heads only
- [ ] `ctl/op.rs`: `RemoveDevice` drops `ctl_cut`/`hlc_cut` (witnessed-validity
  design); self-targeted case carries a final-heads manifest (seal); the
  "own devices only" comment is default-profile CEL, not protocol
- [ ] Subtype layout: critical bit (0x80); all ops critical except
  `SetDeviceName`; non-critical requires proof skipping can't affect fold output

Spec rules to write down (exist only in conversation):

- [ ] AAD = canonical `LogicalHeader` encoding, reconstructed from path on disk
- [ ] Nonces: deterministic coordinate-derived for inner per-entry ciphertext
  (safe because expunge is tombstone-only, never rewrite-in-place); random
  for the outer batch/segment armor (`EncryptionInfo.nonce`)
- [ ] Compression inside encryption; genesis byte-stability (hash raw bytes,
  never re-serialize); `MerkleRoot` length pinned per version (v1 = 32, flat hash)
- [ ] Signature acceptance: Ed25519 strict (RFC 8032), ECDSA low-s only
- [ ] Total order = (HLC, origin, hash); ctl-op causal context = own-log prefix
  + latest `ObservePeers` closure; fail-frozen-never-divergent on unknowns
- [ ] Validity minimization: app-op validity = membership + cuts only, never caps
- [ ] Removal semantics — see `docs/removal-and-cuts.md` (witnessed-validity
  rule: no cut points, no unwinding of acknowledged history, monotone
  verdicts; self-seal with final-heads manifest is the only hard boundary;
  mass-removal handled by default-profile ratification gate)
- [ ] Binding: `Join`/`AdmitDevice` user ids must match, write-once; lazy
  binding incl. root self-join
- [ ] Root `encryption_key.is_some()` = workspace encryption flag; joiners must
  match; `server: true` ⇒ plaintext workspace
- [ ] Graft rules: consent-follows-root; membership edges + data import,
  grants/policies never (no privilege smuggling); cross-join tiebreak
- [ ] Metadata-cleartext disclosure list; E2EE = no forward secrecy by design;
  recovery codes are a product requirement

Profile v1:

- [ ] Hook ↔ op mapping table + default CEL (admin cap, root device axiom)
- [ ] CEL environment pin + context contract (actor device/user, caps, op
  fields, is-server) — biggest unwritten surface
- [ ] Size-bound constants (name, cel, cipher, app_magic)

Conformance vectors (divergence = the cardinal sin):

- [ ] Codec framing incl. unknown non-critical op (skipped, hashed, chained)
  and unknown critical op (fold frozen at exact position)
- [ ] Signature edge cases (non-canonical Ed25519 points, high-s ECDSA)
- [ ] Fold evaluation: worked example incl. concurrent removal + mercy rule

Crates: `chacha20poly1305`, `ed25519-dalek` (not `ed25519`), `p256`,
`x25519-dalek`, `hpke`, `zeroize`, `getrandom`; signing behind a trait for
enclave impls.

## Decided — implement during the engine port

- [ ] **LWW tiebreak = value bytes** (decided June 2026). HLC timestamps carry
  no peer-id component, so two offline peers can mint identical
  `(wall_ms, counter)` timestamps; the source reducers compare strictly (`>`),
  so tied writes resolve first-applied-wins and peers can diverge. The decided
  rule (now in the log-entries doc): a deletion beats a write; between two
  writes the greater value bytes win, with NULL below every value. Rationale:
  the tie is broken by data both peers already hold — no per-cell author
  tracking for an exceedingly rare case. Implementation: extend the reducers'
  `ts >` comparisons to lexicographic `(ts, value)`; SQLite compares
  BLOB/TEXT bytewise already, Postgres TEXT needs `COLLATE "C"` (see below).

## Engine port: fix, don't port

Behaviors in the source implementation that the docs intentionally do NOT
describe — the spec text is the target, these are the deltas:

- [x] **Skew bound was only enforced on some write paths.** In the source, a
  store could observe remote timestamps into the shared HLC without the 60s
  skew check, so a far-future entry could poison the clock. Fixed in the HLC
  port: `Hlc::observe` / `HlcService::observe` take the local wall clock and
  reject beyond-skew timestamps themselves, so no store can skip the check.
- [ ] **Unknown column is a hard error.** The source reducer fails with
  `ColumnNotFound` for an unknown column on a known table, which breaks
  version-skewed peers (the IDs are self-describing precisely so this can
  work). Likely fix: materialize the column on demand (ALTER TABLE ADD COLUMN
  surrogate).

## Reducer port: SQL dialect

The `SqlDialect` trait currently covers type names only. The remaining
divergences to add when the SQL builders port (~5 total):

- [ ] Placeholder style: `?N` (SQLite) vs `$N` (Postgres).
- [ ] Upsert conflict syntax: `INSERT OR IGNORE` vs `ON CONFLICT DO NOTHING`.
- [ ] Max function: SQLite scalar `MAX(a, b)` vs Postgres `GREATEST(a, b)`.
  Keep the `COALESCE` wrapping around the arguments — SQLite's multi-arg
  `MAX` returns NULL if **any** argument is NULL while `GREATEST` ignores
  NULLs; the COALESCE is what makes both backends behave identically. Comment
  it so nobody simplifies it away.
- [ ] `COLLATE "C"` on Postgres text columns wherever byte ordering matters
  (the LWW value-byte tiebreak, pull-sync cursor iteration) so comparisons
  are bytewise like SQLite's BINARY collation.
- [ ] Boolean handling in `RETURNING` clauses (SQLite returns 0/1 integers,
  Postgres returns real booleans).

## JS / WASM wrapper

A browser/JS wrapper is a future deliverable (the npm namespace is already
reserved). Decided not to pursue `no_std` for it — the targets we ship
(Tauri desktop, UniFFI → Swift/Kotlin) all have std, and WASM runs std fine.
What WASM actually needs, to scope when we build the wrapper:

- [ ] **Injectable clock.** `hlc::wall_ms()` calls `SystemTime::now()`, which
  panics on `wasm32-unknown-unknown`. The codec and protocol types are
  otherwise wasm-safe (they only touch `std::io` over in-memory buffers). Make
  the clock source injectable so WASM can supply `Date.now()`.
- [ ] **Storage backend, not `fs_log`.** The filesystem sync layer (`std::fs`)
  won't run in the browser and isn't shipped there; the browser needs a
  different backend (OPFS / IndexedDB) driven from JS.
- [ ] Optional: a `wasm32-unknown-unknown` build check in CI to keep the
  protocol/codec layer browser-portable.

## Docs

- [ ] Byte-level segment/codec encoding page (varints, delta timestamps, UUID
  dictionary compression, blake3 trailer, expungement markers) — write
  alongside the codec port.
