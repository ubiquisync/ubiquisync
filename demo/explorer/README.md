# Ubiquisync Explorer

A desktop demo of the Ubiquisync sync engine. **It is a demo, not an app: do not put real
data in it.** Keys are stored in plain files next to the databases, and the reset button
deletes everything.

One window runs two devices, A and B. Each is a real `StoreImpl` with its own SQLite
database, and both are attached to the same local sync folder through the plain
file-system provider. The data is a todo list on one synced table (`todos`: id, title,
done, created_at). Everything else on screen shows what the engine is doing:

- **Two devices**: both todo lists, the sync folder's files between them, and a
  "Sync on / Paused" button on each side. Pause one side, edit the same item on both,
  resume, and watch the two edits merge. Click a title to rename it; "history" opens the
  item's history in the Logs view.
- **Status**: the `status()` snapshot of the device chosen under "Viewing as": the
  one-line level, what is left to send and apply, each device's log, the sync folder,
  the background tasks, and an activity feed built from `watch()` events.
- **Logs**: every log entry the chosen device holds, merged by time or per device, each
  decoded without a schema (`decode_any`) and named from the demo's schema. Selecting an
  entry shows the item's history: every write to each field, from which device, and
  which one is current.

## Run it

From this directory:

```sh
cargo run
```

It is a plain Cargo project with its own `[workspace]`, so it is not part of the main
workspace build. The `dx` CLI is not needed. Desktop only (macOS, Windows, Linux with
WebKitGTK); the phone layout from the mockups is not built.

To check the engine side without opening a window (adds an item on A and waits until B
has it):

```sh
UBIQUISYNC_EXPLORER_SMOKE=1 cargo run
```

## Where it keeps its files

In `ubiquisync-explorer-demo` under the system temp directory, or in
`UBIQUISYNC_DEMO_DIR` if set:

- `device-A.db`, `device-B.db`: each device's SQLite database
- `device-A.keys`, `device-B.keys`: each device's keys, in plain text
- `sync-folder/`: the shared folder (`peers/` holds each device's init file, `_/<device>/`
  its pack files)

"Reset demo data" shuts both devices down, deletes this directory, and starts again.
Only one copy of the explorer can use a directory at a time.
