# Architecture: layering & the control layer

Working design document (July 2026). Captures the converged protocol
architecture from the design pass, especially the control layer, so that
sub-topics can be split into separate work threads with full context. This is
a design doc, not a spec — normative text graduates to `www/` docs.

## 1. Layering model

Each layer is **additive**. Lower layers are complete and useful without the
layers above them.

**L0 — Logs & identity (base layer).** Per-device, per-container append-only
logs; canonical entry hashing; hash chains; checkpoint signatures; per-container
encryption; genesis (`InitEntry`); expunge/sign records. Complete on its own:
this is the trusted-cloud-folder mode, where the folder ACL *is* the authz and
all logs present are trusted.

**L1 — Replication.** Oplog table, peer cursor table, sync traits
(`HasCursors`/`LogSource`/`WorkspaceSource`), snapshot sync. Storage/transport
agnostic; a "remote" is anything implementing the source traits.

**L2 — Control layer (optional).** One more container with a well-known id
carrying `CtlOp`s. Its materialized state is consulted by the L1 synchronizer
to accept/reject peer logs. Needed only when trust leaves the transport:
dumb relays, multi-user workspaces, membership, capabilities, key
distribution.

**L3 — Profile / policy.** CEL rules stored in-band (`SetPolicy`), default
profile v1 (admin cap, root-device axiom). Workspace-owned data, never wire
rules.

The L1↔L2 interface is a sync-policy predicate (allow/deny/defer per
(peer, container) + cut filters). When L2 is absent, the policy is TrustAll
(folder mode).

## 2. Base layer (L0)

### Genesis (`InitEntry`)

One signed, byte-stable entry per device per workspace — independent of any
container and of the control layer. Fields: format version (first), HLC
timestamp, app magic, device name, signing pubkey, optional encryption pubkey,
optional workspace id (None = root), server flag.

- Device id = truncated hash of raw genesis bytes (opaque 128-bit id, **no**
  RFC-4122 version bits). Hash the raw bytes; never re-serialize.
- Workspace id = root device's id. Fresh keypair per genesis (convention).
- Root's `encryption_key.is_some()` ⇒ workspace provisions key machinery.
  Whether a given remote stores ciphertext is per-remote/per-container.

### Containers

A container is an independent log space: per-device logs, chains, checkpoints,
cursors. Identified by structured 16 bytes: `[0]` protocol flags (incl.
encrypted bit), `[1]` app type byte, `[2..]` random or derived. Well-known
ids for fixed containers: derived from `H(app_magic ‖ name)` with flag/type
bytes stamped after hashing. Creation is implicit at first entry (no init
race — ids are intrinsic). Per-device chain seeded `H(genesis ‖ container_id)`
(cross-container transplant resistance).

### Encryption (per container; a transport/at-rest-encoding property)

**Members hold keys and plaintext; replication containers hold ciphertext.**
Admitted servers are members (receive key wraps; hold plaintext replicas).
Epoch symmetric keys; no forward secrecy by design (new devices must read
history); HPKE wraps distributed as ctl ops (`ShareKey`); epochs implicit
(born at first `ShareKey` mentioning a fingerprint); rotation on removal
protects future entries only.

Encrypted containers use **per-entry encryption; the canonical hash pre-image
is the per-entry ciphertext**. Entries are never rewritten at the same
coordinates (expunge deletes/tombstones, never replaces), so **deterministic
nonces** (derived from epoch, container, origin, index) are safe. Consequences:
ciphertext is canonical and reproducible by any key-holder; relays can verify
chains and execute expunges without keys; plaintext replicas re-derive
canonical bytes on demand. No multi-entry AEAD blocks. Compression: zstd packs
for plaintext containers; optional app-shipped static dictionaries per-entry
for encrypted ones. The ctl container is always plaintext in every transport.
Cleartext per segment: container, origin, index range, epoch fingerprint,
framing. HLC timestamps and `server_user_id` live inside the ciphertext.

AAD rule: AEAD associated data = canonical logical envelope (container,
origin, index, header fields), reconstructed from path/coordinates at read
time — never stored. Moving/renaming a blob breaks authentication.

### Log-level records (needed without the control layer)

- **Sign** (non-indexed meta record): checkpoint signature over
  (height, chain hash). Prunable; batch-seal checkpoints make sealed packs
  standalone-verifiable.
- **Expunge** (indexed): replaces an entry with its retained canonical hash.
  Chain survives. Purpose: data-privacy/legal redaction only (GDPR, leaked
  secrets, abuse). Propagation to replicas/relays requires a signed
  instruction — in control-layer workspaces this is the `ExpungeEntry` ctl op;
  the base layer needs an equivalent signed marker (open item).
- **Range expunge** (future): efficiency variant for bulk redaction. NOT a
  compaction mechanism.

### Compaction = snapshot sync (not expungement)

Snapshot = materialized state + cursor/frontier vector
`(peer, container, idx, hash)` at-or-before that state + producer signature +
HLC. A lightweight replica = snapshot + logs after the cursors. Full-history
replicas are unaffected. Tombstone GC falls out: deletions are baked into the
snapshot; snapshot consumers never see pre-snapshot tombstones. Trust in the
producer is policy (folder mode: trusted; control mode: could require a cap).
Folder pruning of old segments becomes safe once all live replicas are past
the snapshot cursors (operational decision, out of protocol).

## 3. Replication layer (L1): SQL surface & traits

### Tables

- **oplog** `(container_id, peer_id, entry_idx, server_user_id, ts, tag,
  index_key, index_value)` PK `(container, peer, idx)`. The merge of all logs
  from all containers: itself a full replica, a `LogSource`, and the queryable
  indexed history. App-designed index keys may span containers
  (entity-centric queries: docs + table rows as one entity). Rows should
  tolerate opaque payloads (store raw bytes for unknown tags) so ingestion can
  run ahead of what this software version can interpret.
- **peer_cursors** `(peer, container, idx, hash)`. Two duties: (a) the
  *reduction frontier* — what has actually been reduced into state tables
  (oplog may run ahead; supports in-memory accumulation/debounced flush, e.g.
  ydoc batching; crash ⇒ replay from oplog or transport-as-WAL); (b) the
  *chain-verification resume state* — `hash` is the chain head at the cursor,
  required for incremental signature verification. Exists with or without an
  oplog table; a lightweight client is peer_cursors without oplog, reducing
  directly from transport, keeping local state small.
- **genesis** table: device id → raw init bytes (+ parsed pubkeys). Lets any
  replica serve InitEntries to peers. Base layer.
- **keyring** table: epoch fingerprint → decrypted key (member-local,
  sensitive). Wrap records themselves arrive as ctl ops. Base layer (needed
  whenever any container is encrypted).

Control layer adds materialized *caches* (see §5): devices, users,
capabilities-at-current-context. Caches, never sources of truth — the oplog
rows are the truth.

### Traits

- `HasCursors`: query at three granularities — whole workspace, one
  container, one (container, peer) — and watch **one** event stream covering
  all containers (`ContainerAppeared`, `Advanced{container, peer, height}`).
- `LogSource`: read entries for any (container, peer) from a cursor.
- `WorkspaceSource`: enumerates containers; hands out per-container sources;
  ctl container syncs eagerly/first; data containers pull lazily per a
  subscription policy (selective sync; cold containers cost nothing).

## 4. Control layer (L2): the ctl container

One container, well-known id, plaintext everywhere, always fully retained on
every replica including lightweight clients (it is small — human-rate ops).
Vocabulary: `CtlOp` (`crates/ubiquisync-core/src/ctl/op.rs`): CommitContainers,
ObservePeers, SetDeviceName, Join, AdmitDevice, AdmitServer, Grant, Revoke,
RemoveDevice, RemoveUser, RemoveServer, SetPolicy, ShareKey (+ ExpungeEntry,
pending). Grant/Revoke use bounded opaque bytes for capability and value
(≤32/≤64; interpretation is profile/app-level; entity-scoped ACL overloading
is legitimate).

Subtype layout: one reserved outer tag; one-byte subtype; top bit = critical.
Unknown non-critical op ⇒ skip (bytes still hashed/chained). Unknown critical
op ⇒ freeze fold interpretation at that position, keep replicating, surface
upgrade prompt ("fail-frozen, never fail-divergent"). All current ops critical
except SetDeviceName.

### Membership model

- Rooted admission: any member admits (`AdmitDevice`), subject consents
  (`Join` or genesis workspace field). Log valid in W iff consent + admission
  path from root. One device ↔ one user, write-once binding: `Join.user_id`
  and `AdmitDevice.user_id` must match; first matching pair wins.
- Lazy identity: no user id until first binding-bearing fact (root self-binds
  via self-`Join` at first need). Authority axiom = the **root device**, not a
  user.
- Removal: `RemoveDevice{ctl_cut, hlc_cut}` (index pin on target's ctl log;
  HLC cut across data containers via the device's single clock);
  `RemoveServer` same; `RemoveUser` implicit cut at removal position. Removed
  ids are terminal (rejoin = new device).
- Workspace merge (graft): joining root writes `Join`; a host member admits
  it. Consent-follows-root. Membership edges and data import; **grants and
  policies never do** (no privilege smuggling). Ctl facts are scoped to the
  workspace their author was consented to at that position.

### Convergence semantics (case enumeration)

Total order for tiebreaks: (HLC, origin, hash). Causal context of a ctl op =
own-log prefix + closure of the latest preceding `ObservePeers` refs
(ctl heads only, full 32-byte hash anchors).

- **A. Additive facts** (admits, joins), any arrival order: membership grows.
  No reversal.
- **B. Register writes, causally ordered** (Grant/Revoke/SetPolicy/name):
  later wins. No reversal.
- **C. Register writes, concurrent**: deterministic tiebreak; a late-arriving
  concurrent op may flip the *current* register value (convergence, not
  reversal). Cannot cascade: every op's validity was sealed against its own
  causal past.
- **D. Concurrent revocation vs. use of authority**: the op whose context
  lacks the revoke is valid, permanently. Principle: authority is evaluated
  against the actor's causal knowledge. Only cuts override knowledge.
- **E. Cuts**: the sole retroactive mechanism (deliberate; anti-compromise).
  Voids the target's tail beyond the cut, recursively: `void(op)` iff author
  beyond author's cut OR author's membership at the op's causal position
  derives only through voided admits. Handled by refold-on-cut (the ctl fold
  is tiny; refold from genesis is cheap). The layer is *reversal-only-on-cuts*,
  not reversal-free.

### Freshness (revocation with teeth, without Byzantine machinery)

Deflation problem: a device can refuse to reference a revocation and keep its
ops "valid" under rule D. Countermeasure — the freshness rule:

> A cap-gated ctl op X is **void** iff there exists a valid fold-mutating ctl
> op E with `HLC(E) + T_cap ≤ HLC(X)` and E ∉ ctx(X).

Plain HLC clock; per-cap window `T_cap` (profile knob; ∞ disables). To wield
privileges you must have seen everything older than T — operationally, "have
synced ctl through any remote within T," since ctl flows through every remote.
Selective observation fails (judged against what existed, not effort). Data
ops are never freshness-gated (validity-minimization: app-op validity =
membership + cuts only). Consequence, stated as principle: **the freshness
window is the maximum offline duration for exercising privileges**; plain
editing offline is unlimited.

Accepted edge (collaborative-consensus compromise): a validly-written,
never-pushed fold-mutating op surfacing much later retro-voids interim
cap-gated ops of actors who couldn't have seen it. Rare, bounded to
privileged ops, corrected by governance re-grants; nothing is destroyed.
Non-normative UX rule: surface late-arriving old fold-mutating ops as review
events, never silently fold. (A "first-witness" clock was considered and
rejected: the first-witness estimate only ratchets earlier as more `Observed`
entries surface, flipping verdicts void-ward — it recreates retroactive
instability with extra machinery.)

### Evaluation state: MVCC registers

Validity = f(op, its causal context). Implementation is versioned registers,
not replay and not branch checkout:

- Rows: `(key, value, writer_peer, writer_height, hlc, op_hash)`; multiple
  versions per key.
- Visibility of a row to op X: `(writer_peer, writer_height)` covered by
  VV(X) — version-vector dominance.
- VV(X) computed incrementally per entry (max of previous own entry's VV and
  referenced heads' VVs + the refs); cached once per inter-`ObservePeers`
  span (contexts change only at Observed boundaries).
- GC: rows dominated by a newer row whose writer position is below the
  stability frontier (min over admitted origins of last-seen HLC) can never be
  consulted again — squash. Steady-state depth 1; >1 only across live
  concurrency windows.
- SQL realization: capabilities are queryable **from the oplog alone** —
  index_key = (user_id, cap) yields all versions; visibility filtering is a
  join against a small VV relation (`writer_height ≤ vv[writer_peer]`). The
  "active capabilities" table is an optional cache of the current-context
  resolution. Precondition: full ctl history retained on every replica
  (including lightweight ones).

### Cross-container binding

`CommitContainers` (in the author's ctl log): delta list of
`(container, height, chain_hash)` for containers advanced since the author's
last commitment; optional root = versioned flat hash of the full frontier map
(v1: plain blake3 of canonical map encoding; RFC-6962 tree reserved as v2).
Commitment hashes are canonical-bytes chain hashes (per-entry ciphertext in
encrypted containers — so relays can check them there too). Duties: makes
per-container checkpoint signatures prunable once committed (they remain as
real-time tail accelerators and batch-seal marks); makes cross-container
equivocation publicly visible (self-contradiction = proof; forked container
audiences collide via the commitment); staleness of commitments is an audit
signal. `ObservePeers` duties: causal context, freshness evidence, liveness
for the stability frontier.

## 5. Dumb relays (consequence of the layers)

Relays evaluate **monotone facts + their own resource policy**; opinions live
with owners. Ctl is plaintext: relays compute membership (with owner-chosen
predicate over facts), enforce quotas per origin/workspace, verify ctl chains
and commitments. Data containers: with per-entry canonical ciphertext, relays
verify data chains against commitments and execute `ExpungeEntry`
autonomously (swap blob → tombstone). Relays never judge app-data validity;
over-approximation (storing too much) is safe, under-approximation is
forbidden; GC conservative. Read access: transport ACL (owner account) +
revoke facts under owner predicate; true confidentiality against the relay is
the encryption layer. Billing account = the only traditional-auth object;
devices authenticate by device-key request signing checked against
ctl-derived membership.

## 6. Open items (candidates for separate threads)

1. **CEL contract**: environment version pin, feature subset, the exact
   context schema (actor device/user, caps map via MVCC lookup, op fields,
   is-server), hook ↔ op mapping table, default policies (admin cap, root
   device axiom, freshness window defaults per cap). Largest unwritten
   surface.
2. **ExpungeEntry op + base-layer equivalent**: signed expunge instruction
   shape; Expunge policy hook; relay execution semantics per transport;
   verify-across-tombstones conformance vectors.
3. **Snapshot sync**: snapshot format, producer signature, trust policy per
   layer, interaction with encrypted containers (snapshots are key-holder
   artifacts; encrypted in transport), folder-pruning guidance.
4. **Conformance vectors**: codec framing (incl. unknown critical /
   non-critical ops), signature acceptance (Ed25519 strict, ECDSA low-s),
   fold evaluation (worked example: deflecting device, partitioned honest
   device, late-surfacing revocation, cut cascade), commitment/equivocation
   scenarios, deterministic-nonce encrypted container vectors.
5. **Crypto assembly**: chacha20poly1305 (XChaCha), ed25519-dalek (strict
   verify), p256, x25519-dalek, hpke, zeroize, getrandom; signing behind a
   trait (enclave impls); deterministic nonce derivation spec for encrypted
   containers.
6. **Profile v1 document**: everything in L3 as a versioned artifact.
