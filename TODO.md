# TODO

Known gaps and pending decisions, mostly surfaced while porting from the source
codebase. Items here should graduate to GitHub issues (or get fixed) rather
than accumulate.

## Decided — implement during the engine port

- [ ] **LWW tiebreak = value bytes**: HLC timestamps carry
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

## Docs

- [ ] Byte-level segment/codec encoding page (varints, delta timestamps, UUID
  dictionary compression, blake3 trailer, expungement markers) — write
  alongside the codec port.
