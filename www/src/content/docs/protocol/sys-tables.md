---
title: System tables
description: The system table protocol — what system tables are, how their type-encoded IDs work, and why the model was chosen.
---

The Ubiquisync protocol syncs three kinds of data, reflected directly in its operation types: **system tables** (fixed, compile-time schema), **[user-defined tables](/protocol/usr-tables/)** (runtime schema, for applications that let users define their own tables), and **[collaborative rich-text documents](/protocol/documents/)**. This page specifies system tables.

## What system tables are

System tables hold application and framework state with a schema known at compile time: settings, device registrations, sync metadata, key material. The application declares them in code, and every row merges deterministically — any two peers that have seen the same set of operations hold identical rows, regardless of the order the operations arrived in.

The defining constraint is **version skew**. Peers run different versions of the same application — sometimes years apart — and a peer must be able to store and forward data written by a newer peer without understanding it. The schema is advisory; the wire protocol is self-describing.

## Self-describing IDs

Ubiquisync achieves this by embedding type information directly in every table and column ID. The ID alone determines how to parse a value off the wire — no schema lookup, no out-of-band negotiation. A peer that encounters an unknown table or column can decode it, store it faithfully, merge it correctly, and serve it back to peers that do understand it.

### Table IDs

A system table ID is a `u16`. The top 2 bits encode the number of primary key columns (1–4), each PK column type takes 2 bits below that, and the remaining low bits are an arbitrary table index. The layout width varies with the PK count, but parsing is always self-describing: the count is in a fixed position and determines the rest.

```text
1 PK:  ┌ count:2 ┬ t1:2 ┬─────── index:12 ───────┐   4096 indices
2 PKs: ┌ count:2 ┬ t1:2 ┬ t2:2 ┬── index:10 ─────┐   1024 indices
3 PKs: ┌ count:2 ┬ t1:2 ┬ t2:2 ┬ t3:2 ┬ index:8 ─┐    256 indices
4 PKs: ┌ count:2 ┬ t1:2 ┬ t2:2 ┬ t3:2 ┬ t4:2 ┬ index:6 ┐  64 indices
```

Every `u16` bit pattern is a valid table ID: the count field and the 2-bit type fields are total, so table IDs have no protocol-error path.

Primary keys are row identity — they are compared, never merged — so every PK type is deterministic by construction:

| Value | Type | Wire encoding | SQLite | Postgres |
|---|---|---|---|---|
| 0 | Bytes | length-prefixed | `BLOB` | `BYTEA` |
| 1 | Uuid | fixed 16 bytes | `BLOB` | `BYTEA` |
| 2 | Text | length-prefixed | `TEXT` | `TEXT` |
| 3 | I64 | zigzag varint | `INTEGER` | `BIGINT` |

### Column IDs

A system column ID is a `u8`: the high 3 bits encode the column's wire type, and the low 5 bits are an arbitrary column index within the table.

```text
┌─────────────┬───────────────┐
│    type     │ column index  │
│   (3 bits)  │   (5 bits)    │
└─────────────┴───────────────┘
```

| Value | Type | Wire encoding | Merge | SQLite | Postgres |
|---|---|---|---|---|---|
| 0 | Bytes | length-prefixed | LWW | `BLOB` | `BYTEA` |
| 1 | Text | length-prefixed | LWW | `TEXT` | `TEXT` |
| 2 | I64 | zigzag varint | LWW | `INTEGER` | `BIGINT` |
| 3 | Uuid | fixed 16 bytes | LWW | `BLOB` | `BYTEA` |
| 4 | MaxI64 | zigzag varint | max-wins | `INTEGER` | `BIGINT` |
| 5–7 | — | invalid | — | — | — |

Type values 5–7 are **invalid, not reserved**. A peer encountering one treats the entry as a protocol error. This doubles as corruption detection: a bit-flipped ID fails loudly instead of silently misparsing a column.

All non-PK columns are implicitly nullable at the protocol level. There is no NOT NULL: a non-null constraint is a cross-row, cross-peer invariant, and enforcing one during merge would require either rejecting writes (divergence) or inventing default values (data corruption). Applications that need required fields enforce them at the write path, where there is a single author who can be told "no".

## Merge semantics

Every column type was admitted or rejected against one test: **do all peers converge to the same state regardless of the order they receive operations in?**

**LWW (last-writer-wins)** columns carry a hybrid logical clock timestamp; the write with the later timestamp wins. This is the default for all four value types.

**Max-wins** (`MaxI64`) merges by taking the larger value, needs no timestamp, and can only increase. It is the one non-LWW merge in the protocol, and it is the *most* deterministic operation available: `max` is commutative, associative, and idempotent, so the result is independent of delivery order, duplication, and timing. It exists because monotonic values cannot be built safely on LWW — consider a `revoked_at` column: with LWW, a peer holding stale data but a later clock can *un-revoke*; with max-wins it cannot, by construction. Use `MaxI64` for anything that must only move forward: revocation times, high-water marks, migration versions. For min semantics, negate the value at the application layer.

### Why there is no float type

Floating point breaks determinism at the edges: NaN has many bit representations and compares unequal to itself, signed zeros compare equal but differ in bits, and any merge doing arithmetic faces non-associative rounding. None of this is worth solving when the alternative is simple: store decimals as `Text` (or fixed-point scaled integers as `I64`) and interpret them at the application layer.

### Why there is no counter type

A counter column — merge by applying increments — was considered and rejected, and the failure is structural rather than fixable:

- A plain wrapping counter requires exactly-once, ordered delivery, which a peer-to-peer log cannot guarantee. Re-applied or reordered increments silently corrupt the total.
- The "safer" saturating variant is worse in a subtler way: saturating addition is **not associative** — `(a ⊕ b) ⊕ c ≠ a ⊕ (b ⊕ c)` near the limits — so peers that apply the same increments in different orders converge to *different* totals. The safety mechanism is itself a source of divergence.

No single-cell increment scheme survives at-least-once, out-of-order delivery. The correct construction is the classic G-counter, supported as a pattern rather than a type: key a table by `(counter_id, peer_id)` and give it a `MaxI64` value column. Each peer only ever raises its own row, max-wins makes re-delivery and reordering harmless, and readers sum the rows. For decrement support, add a second `MaxI64` column for the negative side (a PN-counter).

## Text rules

`Text` values (PK and non-PK) follow strict rules so that every storage backend behaves identically:

- **Must be valid UTF-8.** Validated when decoding the wire format; invalid UTF-8 is a protocol error. (SQLite would tolerate invalid UTF-8; Postgres would reject it — validating at the protocol layer keeps peers on different backends convergent.)
- **No embedded NUL bytes.** Postgres `TEXT` cannot store `\0`; forbidding it at the protocol level keeps the data portable.
- **Compared as raw bytes.** No Unicode normalization, no case folding, no locale collation. `"café"` in NFC and NFD are different values — and different row keys. Applications that need normalized keys must normalize before writing.

## The type vocabulary is frozen

The type sets above are fixed at protocol v1 and cannot grow compatibly: a peer that does not recognize a type value cannot even skip its bytes on the wire — the width is unknown — so any new type value is a hard protocol fork. Rather than pretend spare bits buy extensibility, the spare column-type values are defined as permanently invalid, which turns them into free corruption detection.

Real extensibility comes from the types themselves: `Bytes` and `Text` are universal transports. A future revision that needs a richer scalar type (a decimal, a structured value, a new semantic) declares those columns as `Bytes` or `Text` at the wire level and applies the richer interpretation in the schema layer of peers that understand it. Older peers parse, store, and re-sync such values as opaque LWW data — exactly the correct behavior for data they do not understand.
