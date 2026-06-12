---
title: Log entries
description: The log model — per-peer append-only logs, the entry envelope, and hybrid logical clock timestamps.
---

Every peer appends each change it makes to its own append-only log. Syncing is copying those logs between devices — over a shared cloud folder or a relay server — and replaying them. No peer ever writes to another peer's log, so there are no write conflicts at the transport level; convergence is entirely the job of the [merge semantics](/protocol/sys-tables/#merge-semantics) applied during replay.

This page specifies the unit of that log: the entry envelope and its timestamp. The operations the envelope carries are specified per data domain — [system tables](/protocol/sys-tables/), [user-defined tables](/protocol/usr-tables/), and [documents](/protocol/documents/). The byte-level segment file encoding is an implementation concern, documented separately.

## The envelope

A log entry is one operation plus metadata:

| Field | Contents |
|---|---|
| `op` | The state mutation — one operation from the domain's op vocabulary. |
| `timestamp` | HLC timestamp, monotonically increasing within a peer's stream. |
| `user_id` | Optional user attribution (see below). |

There are two log domains, distinguished by what `op` is: the **state log** carries system and user-defined table operations, and the **document log** carries document operations. Both use this same envelope, and both draw timestamps from a single shared clock per peer, so timestamps are causally comparable across the two logs.

Each entry is individually integrity-hashed (blake3) and is the unit of **expungement**: an entry can be redacted from a segment after the fact without invalidating the entries around it. Append-only storage still has to honor permanent removal — leaked secrets, data-deletion requests — and rewriting history is not an option when other peers replay logs by position.

## Timestamps

Every entry carries a hybrid logical clock (HLC) timestamp packed into a single `u64`:

```text
[  wall_ms: 48 bits  ][  counter: 16 bits  ]
```

The top 48 bits are Unix epoch milliseconds (enough until roughly year 10895); the low 16 bits are a counter that disambiguates writes within the same millisecond. Because the wall clock occupies the high bits, **plain integer comparison preserves causal order** — every LWW merge decision in the protocol is a single `u64` comparison, with no timestamp parsing anywhere in the merge path.

The clock guarantees:

- **Monotonic within a peer.** Each timestamp a peer generates is strictly greater than its last, even if the wall clock stalls or steps backward (the counter absorbs it).
- **Causal across peers.** When a peer observes a remote entry, its clock advances to at least that timestamp, so everything it writes afterward is strictly newer. "I changed it after seeing your change" therefore always wins LWW, regardless of whose wall clock is ahead.
- **Bounded skew.** A remote entry whose wall-clock component is more than 60 seconds ahead of the local clock is rejected. Without this bound, a single device with a badly wrong clock would drag every peer's clock years ahead of wall time, and its writes would outrank honestly-timestamped data until real time caught up. Past timestamps are always accepted — they lose LWW merges harmlessly, which is the correct outcome for stale data.

## Attribution

The `user_id` field exists because logs are written in two modes:

- **Device mode** — the log belongs to a device, and attribution is implicit: every entry in a device's log was authored by that device's user. `user_id` is absent.
- **Server mode** — a server writes a log on behalf of clients that have no durable storage of their own (e.g. web clients). Entries from different users interleave in one log, so each entry carries its author's `user_id` explicitly.

Attribution is metadata, not merge input: two entries merge identically whether or not they are attributed.
