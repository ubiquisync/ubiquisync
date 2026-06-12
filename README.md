<p align="center">
  <img src="www/src/assets/logo.svg" width="160" alt="Ubiquisync logo">
</p>

# Ubiquisync

**WORK IN PROGRESS: NOT READY FOR PRODUCTION**

Conflict-free sync over commodity cloud storage or server.

Ubiquisync solves the problem of syncing user workspace data between devices without
merge conflicts and without the need to stand up any sync server infrastructure.

It will allow you to sync both structured data (stored in SQLite or Postgres) and collaborative rich-text documents over commodity cloud storage such as Google Drive, iCloud Drive, Dropbox or a dedicated sync server.

## Features

Ubiquisync might be a good fit if your app could benefit from these features:
- SQLite or Postgres data storage and querying
- user-defined schemas (a la Airtable, Notion)
- conflict-free merging of rich document content (a la Google Docs)
- local-first, offline data
- sync over Google Drive, iCloud Drive, Dropbox, etc. OR a dedicated sync server for
  real-time collaboration and user management
- reactive updates
- full revision history and attribution

## How it works

Each device keeps an append-only log of the changes it makes, stamped with a hybrid
logical clock (HLC). Syncing is just a matter of copying those per-device logs between
peers — over a shared cloud folder or a relay server — and replaying them. Each device
writes only to its own log and never touches another device's files, so there is
nothing for the storage provider to conflict on: no "conflicted copy" duplicates, no
file-level merges. And because every change lives in the log, peers always converge on
the same state no matter what order updates arrive in.

Merges are conflict-free by construction, with the strategy depending on the data:
- **Structured rows** merge last-writer-wins by HLC timestamp, plus a max-wins
  column type for monotonic values that must only move forward.
- **Rich-text documents** merge as CRDTs (via [yrs](https://github.com/y-crdt/y-crdt),
  the Rust port of Yjs), so concurrent edits to the same document combine without
  losing anyone's work.

Schema changes propagate conflict-free in exactly the same way. Tables and columns are
addressed by stable IDs and travel in the log alongside the data, so adding a table or
column is just another change that flows to every peer — there's no lock-step migration
step to coordinate. A device running older code can still receive and store data for
tables it doesn't recognize yet, and adopt the proper schema once its code catches up.

No server is required: as long as the log directory syncs (iCloud / Drive / Dropbox),
devices stay in sync, offline-first. An optional server can additionally relay updates
in real time and serve web clients that have no local storage of their own.

## Caveats

Ubiquisync might not be a good fit if any of these things apply:
- needs fine grained read or write permissions
- produces a very high volume of tiny, frequent changes, where the per-change log overhead can outweigh the data itself
- needs unique constraints on tables
- needs to use SQL DDL or DML directly — Ubiquisync supports SQL queries, but requires using its own data definition and manipulation primitives
