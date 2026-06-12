---
title: User-defined tables
description: The user-defined table protocol — the entity model, operations, and why cell values are only text and UUID.
---

User-defined tables hold schemas that *users of the application* create at runtime — the Airtable/Notion model, where a user adds a "Projects" table with whatever columns they like, on whatever device they happen to be using, possibly offline. Where [system tables](/protocol/sys-tables/) are declared by the application developer at compile time, user tables are themselves user data, and the protocol has to sync the schema with the same guarantees as the rows.

## Everything is a UUID

Tables, columns, rows, and join tables are all addressed by 16-byte UUIDs. There is no central registry and no allocation step: a peer mints a UUID offline and the new table or column is immediately valid protocol-wide, collision-free by construction. This is the deliberate opposite of system table IDs, which are compact and type-encoded — that design needs a compile-time author to assign indices, which is exactly what user tables don't have.

## The entity model

Rows are **entities**. An entity is a global UUID whose existence is independent of any table; what table it belongs to is a property — its *affinity* — set by an operation, not implied by where its values were written:

- `UsrSetTable(entity_id, table_id)` points an entity at a table, LWW by timestamp. Setting affinity is what creates a row; re-pointing it is how a row **moves between tables** as one O(1) operation, keeping its identity (and so its inbound references and join edges).
- `UsrDelete(entity_id)` soft-deletes an entity by advancing a delete tombstone. An entity is deleted while its tombstone is newer than its affinity — a later `UsrSetTable` revives it. Restore-from-trash is the same operation as creation.

## Operations

| Operation | Effect | Merge |
|---|---|---|
| `UsrUpsert` | Write cell values for a row | Per-column LWW |
| `UsrSetTable` | Set an entity's table affinity | LWW |
| `UsrDelete` | Soft-delete an entity | Tombstone vs. affinity timestamp |
| `UsrUpdateJoin` | Set or remove a join-table edge | LWW per edge |

`UsrUpsert` carries the table UUID, the row's entity UUID, and a list of `(column_uuid, value)` pairs, where a value of NULL clears the cell (all user columns are nullable, for the same reason [system table columns are](/protocol/sys-tables/#column-ids)). Each column merges independently: concurrent edits to *different* columns of the same row both survive; concurrent edits to the *same* column resolve LWW.

`UsrUpdateJoin` is the many-to-many primitive — row relations, multi-select membership. An edge is keyed `(left_row_id, right_row_id)` within a join table UUID, and set/remove resolve LWW per edge, so concurrent membership changes to different pairs never conflict.

## Cell values: only text and UUID

A user-table cell holds one of exactly two value shapes, plus NULL:

| Value | Carries | Why |
|---|---|---|
| `Text` | Every user-facing scalar: strings, numbers, dates, checkboxes, URLs | Scalar column types must be trivially changeable |
| `Uuid` | References: row links, select and multi-select option IDs | Points at synced objects; not meaningfully retypeable |

This is a deliberate design position, not a missing feature. In a user-defined schema system, a column's "type" — number, date, currency, checkbox — is **view-time formatting plus lightweight validation**, not a storage property. When a user changes a Notion-style number column to plain text, or a text column to a date, nothing about the stored data should need to change: no row rewrites, and — critically for a distributed system — no schema migration to coordinate across peers that may be offline for months. Storing every scalar as text is what makes retyping a metadata-only edit.

The exceptions prove the rule: selects, multi-selects, and references are *not* scalars — they point at other objects (option rows, other entities) — so they get the one non-text shape, `Uuid`.

Two consequences worth stating plainly:

- **The type travels with the value, not the column.** Each value on the wire is tagged text/UUID/NULL. A column has no protocol-level type to violate, so a retyped column's old rows are simply still valid.
- **The store doesn't sort numerically.** Since numbers are text at the storage layer, ordering and aggregation by numeric value are application concerns, applied at view time alongside the formatting that makes the column "a number" in the first place.

## What the protocol doesn't define

Column display names, column order, view configuration, validation rules — all of that is schema *metadata*, and to the protocol it is just more application data, synced through ordinary tables. The protocol needs none of it to merge user-table operations: every op above is self-sufficient given only its UUIDs, values, and timestamp.
