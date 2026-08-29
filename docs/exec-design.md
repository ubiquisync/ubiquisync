## Execution Flow (local originated ops)

NOTE: single threaded executor for local ops, could concurrently
ingest and _maybe_ execute remote ops as long as HLC ordering is correct

- before this all, call reducer's prepare fn with op
- take single op, convert into wire bytes + container ID
- NOTE: for ctl containers we'll expand into multiple ops in a batch (observe peers, wrap keys, etc.)
- find live local stream id, end idx & end hash for this container
- advance HLC and capture timestamp
- package into single entry segment OpBatch with end idx (start-end idx is half open interval)
- compute entry hash and new chain hash
- sign new chain hash
- package into compressed segment
- add to sql batch: insert into segments
- pass op + mut sql batch to reducer's apply method with timestmap, don't need to pass container id because its implicit,
- but do we need to pass peer id?
- add to sql batch: update to streams table advancing committed idx (what do we do with ready idx for local ops)?
- execute sql batch
- pass results to reducer post apply, dispatch events


