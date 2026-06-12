---
title: Documents
description: The document protocol — CRDT updates, namespaces, and delete semantics for collaborative rich text.
sidebar:
  order: 4
---

Documents are the protocol's third data domain: collaborative content — primarily rich text — where last-writer-wins is the wrong merge. For a table cell, "the newer edit replaces the older one" is what users expect. For a paragraph two people edited offline, it means one of them loses their work. Documents therefore merge as **CRDTs**: concurrent edits to the same document combine, down to individual characters, with no edit discarded.

Ubiquisync uses [yrs](https://github.com/y-crdt/y-crdt) (the Rust implementation of Yjs) as its CRDT engine; document update payloads are Yjs-compatible v1 updates.

## Operations

The document log carries exactly two operations:

| Operation | Effect | Merge |
|---|---|---|
| `UpdateDoc` | Apply a CRDT update to a document | CRDT integration |
| `DeleteDoc` | Soft-delete a document | LWW tombstone |

Both identify their target by a `(ns, id)` pair of UUIDs. `id` is the document; `ns` is an application-defined **namespace** that partitions the document space — one namespace per collection, per document field, per whatever grouping the application needs. The protocol treats the pair as an opaque composite key.

## Opaque updates

The protocol never looks inside an `UpdateDoc` payload. It stores the blob, forwards it, and hands it to the CRDT engine at apply time. This buys two properties:

**Delivery can be duplicated and reordered.** Every operation inside a CRDT update carries its own `(client, clock)` identity, so integration is commutative and idempotent: updates can arrive out of order, and applying one twice is a no-op. That is exactly the delivery model of logs replayed over cloud-folder sync — at-least-once, in whatever order files appear — so document sync needs no acknowledgements, no deduplication layer, and no merge timestamps of its own.

**Version skew is free.** Because peers relay payloads without interpreting them, an older peer stores and forwards documents written by newer application versions with richer document schemas — the same store-and-forward obligation [system tables](/protocol/sys-tables/) meet with self-describing IDs, met here by never needing to describe the content at all.

The entry's HLC timestamp is still present on every update — not for merging content, but for the one LWW decision documents do have: deletion.

## Delete semantics

`DeleteDoc` is an LWW tombstone on the entry timestamp, exactly like a table row delete: the document is deleted while the tombstone is newer than every update, and a newer-timestamped `UpdateDoc` revives it.

After a delete, a peer may drop the document's content from local storage, but the tombstone itself stays in the log. Peers that have not yet seen the delete will still replay it, and peers that see a stale update for a deleted document resolve it by timestamp like any other LWW race.

## Why CRDTs here and not everywhere

CRDT merging is strictly more capable than LWW, so why isn't the whole protocol built on it? Cost: CRDTs pay for their merge quality with per-operation identity metadata and tombstone growth inside every document. For collaborative text, that price buys the only acceptable semantics. For scalar cells and rows, LWW already *is* the semantics users expect — the newer value wins — and it costs one timestamp comparison against a single stored value. The protocol spends CRDT complexity exactly where replacement semantics would lose user work, and nowhere else.
