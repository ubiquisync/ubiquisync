# TODO

Known gaps and pending decisions, mostly surfaced while porting from the source
codebase. Items here should graduate to GitHub issues (or get fixed) rather
than accumulate.

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

- [ ] **Doc store: stale `DeleteDoc` beats newer updates.** The source read
  filter is `deleted_ts IS NULL` only, so a tombstone older than the newest
  update still deletes on peers that receive it late. Spec: deleted while
  tombstone >= newest update. Implement visibility as
  `deleted_ts IS NULL OR updated_at > deleted_ts`.
- [ ] **Skew bound only enforced on the state log.** The source doc store
  observes remote timestamps into the shared HLC without the 60s skew check,
  so a far-future document entry can still poison the clock. Enforce at the
  shared clock layer.
- [ ] **NULL writes to not-yet-materialized usr columns are dropped.** The
  NULL's timestamp is never recorded, so an older non-NULL write arriving
  later resurrects the cell on that peer only. Record cleared-cell timestamps
  even when no physical column exists yet.
- [ ] **Unknown sys column is a hard error.** The source reducer fails with
  `SysColumnNotFound` for an unknown column on a known table, which breaks
  version-skewed peers (the IDs are self-describing precisely so this can
  work). Likely fix: materialize the column on demand (ALTER TABLE ADD COLUMN
  surrogate).

## Codec port

- [ ] **Enforce the Text rules at decode time.** Strict UTF-8 already holds in
  the source (`String::from_utf8(...)?` makes invalid UTF-8 a protocol error
  before it reaches storage) — keep it. The **embedded-NUL check is not
  enforced anywhere yet**: the docs forbid `\0` in Text because SQLite
  tolerates it while Postgres rejects it, so an unchecked NUL is a stored
  value one backend physically cannot hold — divergence. Validate on decode
  (and reject on encode) for Text PKs, sys Text columns, and usr text values.

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

## Docs

- [ ] Byte-level segment/codec encoding page (varints, delta timestamps, UUID
  dictionary compression, blake3 trailer, expungement markers) — write
  alongside the codec port.
