# Removal: Witnessed Validity and Seals

Normative semantics for `RemoveDevice` and `RemoveUser`. This revision
**replaces** the previous cut-point + contested-cut design in this file, and
supersedes the freshness/cut sections of `architecture-control-layer.md`
where they conflict. §7 records the alternatives considered and why each
lost, so the design history survives compaction.

Companion reading: `architecture-control-layer.md` (fold, causal contexts,
ObservePeers/CommitContainers, MVCC registers).

---

## 1. Ground rules

1. **Ingestion is verdict-free.** Chained, signed bytes always flow and are
   stored; whether a replica fetches or stores is local resource policy.
   What held bytes *mean* is never a local decision, and no verdict ever
   destroys bytes. Only privacy expungement destroys bytes.
2. **Verdicts are positional.** An entry's status is a function of facts in
   the log set (declared causal contexts, chain positions) — never of when
   any replica received anything. Same log set ⇒ same verdicts everywhere.
3. **Actor-frame validity (mercy).** An op is valid iff its actor held the
   required authority in the fold of its own declared causal past (own-log
   prefix + ObservePeers closure). Authority changes bind prospectively, on
   observation. There is no single global permission state; validity is a
   per-frame fold over the raw log set, and records are never destroyed
   precisely so those frames stay evaluable.
   **Validity is evaluated in exactly this one frame — never additionally
   against any replica's current state.** Witnessing (§2) is a separate,
   monotone materialization filter, not a second validity check. (Matrix
   learned this the hard way: its dual evaluation — declared auth_events
   plus current-state "soft fail" — was a chronic source of bugs and
   moderation confusion. Do not add a current-state check "for safety.")
4. **Acknowledged history is never unwound.** No mechanism — removal,
   governance, or otherwise — invalidates an entry that a valid member
   acknowledged before observing the removal. Entry validity is
   **monotone**: verdicts move only from pending/invalid to valid as new
   logs surface, never the reverse. The only rollback in the system is a
   replica discarding its own *unacknowledged* transients (§4), and the
   only hard boundary is the self-seal (§3.2), where no honest
   post-boundary write can exist.

Rule 4 replaces the previous design's "all retroactivity is explicit,
human-issued, single-target." Retroactivity is now **eliminated** for
remote removals, not gated.

## 2. The witnessed-validity rule

Definitions:

- An op **acknowledges** history H if H is in its declared causal context
  (ctl: ObservePeers closure; the declaration channel for *data* history
  is the §9 core open item — see §4 for the commitment/visibility
  distinction that constrains it).
- Two ops are **concurrent** if neither is in the other's context.
- A member's op is itself valid per rule 3, with author validity computed
  recursively down the admission tree (grounded at genesis; contexts are
  hash-chained and acyclic, so the recursion terminates).

> **The rule.** Let C be a removal op naming device R (directly, or as one
> of a removed user's devices). An entry e authored by R is **valid** iff
> either (a) e is in C's own causal context — the issuer acknowledged it by
> issuing — or (b) some valid op W acknowledges e, where W's author is a
> valid member other than R and **C is not in W's context**.
> Everything else R authored is invalid, permanently pending: it can
> become valid if a qualifying W surfaces from an unsynced log, and can
> never be invalidated once valid.

Plainly: **what somebody saw before the removal, happened; what nobody saw,
never happened.** The removal's effective boundary is not a declared index
or timestamp but C's *causal frontier* — the union of what the issuer and
every concurrently-acting member actually observed of R.

Consequences:

- **Granularity is exact.** If T acknowledged e1…e5 but not e6…e10, then
  e1…e5 are valid and e6…e10 are not. Nothing is shielded or voided by
  proximity; exactly the observed prefix survives.
- **No unwinding, ever.** A replica that materialized entries after
  acknowledging them never rolls them back: its own acknowledgment is a
  qualifying W. Late-surfacing removals (drawer, §6.3) therefore cannot
  touch anything — a year of acknowledged work is a year of witnesses.
- **No backdating surface.** The previous design's HLC boundary for
  `RemoveUser` was forgeable by the target (backdate into the window).
  Claimed timestamps are now irrelevant to removal verdicts: validity
  depends on *other people's* already-written contexts, which an adversary
  cannot retroactively enter.
- **Resurrection is ordinary late delivery.** A pending entry turning valid
  when its witness's log surfaces is handled by normal CRDT merge — no
  repair machinery, no state machine, no "restored pending review" UX.
- **Anti-self-shielding is emergent.** A puppet device admitted by R inside
  the unwitnessed tail is itself invalid (its admission was never
  acknowledged), so its acknowledgments validate nothing. Puppets admitted
  inside the *witnessed* window are real members until removed (§5).

Convergence: all inputs to the rule are positional facts, so any two
replicas holding the same log set compute identical verdicts, and as logs
propagate all replicas converge — monotonically.

**Two classes of invalidity** (and acknowledgments are powerless over one
of them):

- **Pending** — a removed device's unwitnessed tail. Revivable by design:
  a qualifying witness surfacing from an unsynced log makes it valid.
  This is the *only* revivable class.
- **Absolute** — post-seal extensions (§3.2), `UseKey` with no justifying
  `ShareKey` in context (§4), frame-invalid ops, malformed records.
  Unrevivable, ever. An acknowledgment naming absolutely-invalid history
  has **no validating force** — though the acknowledging op itself
  remains valid; there is no contagion through acknowledgment. (Matrix's
  soft-fail revival bug — later events referencing suppressed events
  pulling them back into effect — is what this rule forecloses.)

**Client obligation**: pending entries are first-class, enumerable state
— surfaced for review, spam cleanup, and the import affordance for a
removed device's unwitnessed work — never silently hidden bytes. (Matrix
hid its soft-failed events; moderation tooling suffered for years.)

## 3. The ops

### 3.1 RemoveDevice { device_id } — remote

Issued from a device other than the target (default profile: only by the
same user's devices; org profiles may extend to admins via CEL — the
restriction is policy, not protocol). Carries **no cut parameters**; the
previous `ctl_cut`/`hlc_cut` fields are removed.

- **Prospective effect**: R's membership terminates, binding each observer
  as C enters its frame. Terminal; removed ids are never readmitted.
- **Validity of R's entries**: the witnessed-validity rule (§2).
- Acknowledgment by the target never matters. A hostile or vanished R
  changes nothing: once C is in every live member's context, no honest
  member can mint a qualifying W (their acknowledgments contain C), so R's
  tail past the frontier is dead regardless of what R signs or claims.

### 3.2 Self-removal: the seal

`RemoveDevice` targeting the author's own device, written in the device's
own ctl log — logout. Categorically different and *stronger* than any
remote removal:

- The op occupies a position in the device's own hash chain, so the
  boundary is an **index, for free, with perfect granularity and no
  forgery surface**: entries chained after the seal are provably authored
  after logout.
- The seal op carries a **final-heads manifest**: a terminal
  `CommitContainers` naming the head of every data log the device manages
  (the device stops its writers, then seals — one atomic fact; per-log
  seal records were rejected because a crash mid-sealing would leave an
  ambiguous half-logged-out state). Any data entry past a manifest head is
  past the seal.
- **The witness rule does not apply to seals.** Post-seal extensions are
  invalid absolutely, acknowledged or not, because no honest post-seal
  write can exist — the drawer risk that motivates witnessed validity for
  remote removals (honest concurrent work the issuer hasn't seen) is
  impossible here by construction. This is the system's only hard cut.
- A peer consuming the device's data logs before its ctl log sees no seal
  yet; that is transient materialization skew, identical to every other
  authority fact (data streams never carry membership information), and
  terminal verdicts are unaffected (rule 2).
- A keyless relay can refuse post-seal appends with verdict-grade
  confidence — the violation is positional and visible in the ctl facts
  relays already read. This is the one ingestion refusal that is more than
  resource policy.

### 3.3 RemoveUser { user_id }

Removal of a user: binding terminal, no new devices admissible, and every
device of the user is treated as a removal target with this op as its C.
Per-device validity is the witnessed-validity rule; there is no HLC
boundary and no `before_hlc` parameter (the previous design's deep-cut
deferral is obsolete — see §5 for how bad history is handled instead).

## 4. Ack-gated materialization

Two mechanisms must not be conflated here:

- **Commitment is log-local.** A log's own signature records (signed chain
  checkpoints) are what make its entries attested and materializable.
  Peers materialize what a log's signatures cover; `CommitContainers` is
  not required for this and is not a commitment mechanism.
- **`CommitContainers` forces transitive visibility.** By placing the
  totality of a device's container heads into its ctl log, it makes
  acknowledgment non-cherry-pickable: an observer that acknowledges the
  ctl head cannot feign ignorance of the data heads that head references
  while claiming knowledge of ctl. Its role in the witness rule is to fix
  what an acknowledgment *necessarily covers* — not to enable data to be
  witnessed in the first place.

**Key use is acknowledgment.** A declared `UseKey(fp)` implicitly
acknowledges the `ShareKey` op that delivered fp to the writing device,
together with that op's **full causal context**. When rotation follows a
removal, the ShareKey carrying the new epoch has the removal in its
context — so any device that writes with the new epoch has acknowledged
the removal and can never again witness the removed device's tail. A
device may participate in a post-removal epoch or keep witnessing the
removed device — never both. A `UseKey` whose fingerprint has no
justifying `ShareKey` in the device's declared context is malformed:
out-of-band key possession cannot be prevented, but it cannot be
laundered into valid protocol-visible use. Corollary: prompt rotation on
removal converts crypto hygiene into witness-set closure for encrypted
containers.

This is provable from three facts. (1) A valid `UseKey(fp)` requires a
justifying `ShareKey(fp)` in its declared context (normative, above).
(2) Declared contexts are causally closed: including an op includes that
op's entire causal past (own-log prefix + ObservePeers closure,
transitively). (3) A device's context grows monotonically along its own
log — the own-log prefix is always included, so nothing acknowledged can
later be un-acknowledged. Therefore, if the ShareKey followed a removal
in its issuer's frame, the removal is in the ShareKey's causal past (2),
hence in the declared context of any valid `UseKey` of that epoch (1),
hence in the context of every subsequent op the using device ever writes
(3) — none of which can be concurrent with the removal, so the device
can never again produce a qualifying witness for the removed device's
tail. ∎

Because acknowledgment is now the validity mechanism, it acquires a
normative role in the local apply path:

> A replica materializes what log-local signatures attest, but state not
> yet covered by its own emitted acknowledgments is transient: it may be
> discarded without ceremony if a removal arrives first — the only
> rollback in the system, equivalent to aborting an uncommitted
> transaction. Durability of *meaning* attaches at acknowledgment, not
> at receipt.

**The data-ack closure** — what "W acknowledges data entry e" means —
needs no new vocabulary. W's `ObservePeers` names the target's ctl head;
the target's ctl log up to that head contains its `CommitContainers`
brackets; e is acknowledged iff a bracket at or below the named head
covers it. `ObservePeers` (ctl heads) and the targets' own brackets
compose into the full closure. The residue is the target's **open
bracket**: data synced ahead of its author's next `CommitContainers` is
materializable (its own log's signatures attest it) but not yet
witness-protected — a transient at every replica, matching the inclusion
filter's bounded open-bracket optimism. Brackets are routine (written
with each atomic write batch), so the unprotected tail is small and
closes at the author's next bracket.

## 5. Aftermath of a removal

Removal terminates authorship going forward. Everything else is forward
repair, with a clean division of labor:

- **Content** that is wrong (spam, vandalism, mistakes) — **app-level
  undo/edit**. Valid history is fixed by writing, never by re-judging.
- **Authority** that shouldn't persist — explicit prospective ops:
  `Revoke` for grants, `RemoveDevice`/`RemoveUser` for puppets admitted in
  the witnessed window. The fold can enumerate every entity and grant
  whose authority traces through the removed device (provenance tree);
  the client presents this at removal time as the blast radius,
  bulk-issuable in one gesture. Recursion lives in tooling, not verdicts.
- **Bytes** that must not exist (privacy/legal) — expungement. Never used
  as fixup.
- **Keys** the target learned — unrecoverable in any design; rotation
  (new epoch + ShareKey) protects the future, nothing protects the past.

Puppet whack-a-mole terminates: each puppet costs one removal op; a
removed puppet's own unacknowledged tail validates nothing (honest members
hold its removal and cannot witness for it — §2), and puppets minted in
anyone's unwitnessed tail were never members at all. Depth is bounded by
what was actually witnessed.

## 6. Normative walkthroughs

### 6.1 Three devices (race)

M (observer), R (removed device), T (third device). C = RemoveDevice(R);
R wrote e1…e10 after C's issuer last saw it; T acknowledged e1…e5 and
built on them before observing C.

1. M holds only C: R removed prospectively; nothing known past C's context.
2. R's e1…e10 reach M: unacknowledged by anyone M knows of → pending, not
   materialized.
3. T's log reaches M: T's acknowledgment predates T's observation of C →
   e1…e5 valid at M (and everywhere), materialized; T's own edits were
   never in question. e6…e10 remain pending — nobody saw them — and die
   at the frontier. No review state, no re-issue, no flip risk: T's
   witness can never be un-happened.

Arrival order irrelevant: had M received T's log first, e1…e5 would have
materialized earlier; terminal state identical.

### 6.2 Thief

Phone stolen Monday; owner notices Wednesday and issues `RemoveDevice`.

- Entries the thief wrote that peers acknowledged before observing C —
  the Monday–Wednesday window — are **valid, permanently**. This is the
  named price of this design (§7, alternative C). Cleanup is §5: undo the
  content, revoke the grants, remove any puppets — each visible in the
  provenance tree at removal time.
- Entries the thief pushed that nobody acknowledged (e.g. staged on a
  relay and pulled by members only after C propagated) are dead on
  arrival: every potential witness already holds C.
- Once C reaches all live members, the stolen device is inert: nothing it
  signs can ever become valid. No index cut needed; no backdating helps.

### 6.3 Drawer (stale removal)

A device issues a removal, then stays offline for a year while the target
works normally and everyone builds on that work. When the drawer log
surfaces: every affected entry has a year of qualifying witnesses → all
valid, nothing moves anywhere. The removal binds prospectively from
observation. No contest state, no review queue, no re-issue decision —
the case that motivated three prior design iterations is now a non-event
by construction.

### 6.4 Drawer, prospective variant (mass removal)

The drawer log contains `RemoveUser` for every member: frame-valid under
mercy, and prospective effects need no witnesses — decapitation, not
rewind. Defenses, unchanged from the previous revision:

1. If the drawer device was itself removed in the interim, its spree is
   invalid at every replica that folds its removal first — and its spree
   ops are unacknowledgeable by honest members thereafter.
2. **Default-profile gate** (profile v1): removals — at minimum removals
   of admins, active when ≥ 2 admins exist — are effective only once
   ratified by a second current admin. Deterministic, frame-evaluable.
   The spree surfaces as pending proposals no live admin co-signs.
3. Backstop: governance can be captured, data cannot. All members hold
   bytes and keys; recovery is fork-and-graft, and grants/policies
   deliberately do not survive grafts.

### 6.5 Collusion (engineered ignorance)

Member T deliberately avoids syncing C and keeps acknowledging R's new
output, extending R's validity frontier. Bounded facts: T must be a
member; the pattern is visibly anomalous (a device that syncs everyone
except the log that binds it); in encrypted containers the choice is
forced cryptographically — writing with the rotated epoch acknowledges
the removal (§4), while clinging to the retired epoch *is* the visible
anomaly; the remedy is removing T — after which T's
further acknowledgments are themselves unwitnessable by honest members
and validate nothing. And T could achieve strictly more, unflagged, by
re-emitting R's content under its own key: member content laundering is
unpreventable in any protocol, so the honest-attribution path is not the
thing to fortify. The defense against a bad member is membership
termination, here as everywhere.

## 7. Alternatives considered

Recorded so this ground isn't re-litigated from scratch. Verdict format:
what it buys / where it breaks.

- **A. Freshness rules** (op void if a sufficiently-old unobserved
  authority op exists). Buys: automatic staleness hygiene. Breaks:
  compares *claimed* HLCs, so adversaries backdate into the mercy window
  while honest-but-offline actors get voided — it punishes exactly the
  wrong party, and its blast radius (drawer phone voiding a year of
  everyone's work) is catastrophic. Deleted; do not revive.
- **B. Transitive cuts** (voiding a device recursively voids everything
  downstream: admits, grants, dependent writes). Buys: one-op cleanup.
  Breaks: large-scale retroactivity no human aimed, rewriting third
  parties' lived history; drawer case turns it into a workspace bomb.
  Rejected in favor of prospective cascades + provenance tooling.
- **C. Declared cut points + contested-cut rule + re-issue** (the previous
  revision of this document: index/HLC boundaries; retroactive void
  executes iff unwitnessed; witnessed → contested → human re-issues to
  erase). The only serious contender. Buys, relative to witnessed
  validity: (1) backdated compromise cuts — the Monday–Wednesday thief
  window can be *erased* rather than undone; (2) governance can remove
  witnessed garbage from the valid set rather than compensating over it.
  Breaks: those two powers are precisely what force the contested state
  machine — bidirectional verdict flips, materialization repair,
  contested-review UX, cut-point parameters and their anchoring problem,
  and a re-issue protocol — because any mechanism that can unwind
  acknowledged history must be guarded against stale issuers or it
  reintroduces A/B's bombs. And the end state it buys is reachable
  anyway: undo produces the same materialized content, prospective ops
  produce the same authority state; only log aesthetics differ. Verdict:
  strictly more machinery for equivalent reachable outcomes; superseded.
- **D. Purely per-observer mercy, no witness rule** ("each replica keeps
  whatever it got before it saw C"). Buys: nothing over §2; feels simple.
  Breaks: verdicts become functions of local arrival order — replicas
  that received different subsets diverge *permanently*, and a target
  that never acknowledges its removal writes frame-valid entries forever.
  Witnessed validity is exactly this idea made positional: "what someone
  saw" is read from declared contexts in the logs, not from local
  delivery accidents.
- **E. Quorum/BFT removal votes.** Buys: adversary-resistant agreement.
  Breaks: wrong trust model — this is collaborative consensus among
  parties who mostly trust each other, not Byzantine agreement; quorum
  liveness fails exactly in the offline/partitioned operation the
  protocol exists to serve. The lightweight cousin (second-admin
  ratification for mass removals) is adopted as a profile gate (§6.4).
- **F. Cryptographic revocation / key rotation as removal.** Buys: real
  confidentiality going forward. Breaks: nothing — but it answers the
  read side only (who can decrypt the future), not write validity. Kept,
  as the existing rotation design; complement, not alternative.
- **G. Global state resolution (Matrix-style).** One algorithm
  re-resolves authoritative state over the whole event DAG whenever forks
  merge. Buys: no pending class, no witness rule — one function answers
  everything. Breaks: the resolution is deterministic but **nonmonotone**
  — an old fork surfacing can flip anyone's standing retroactively
  ("state resets": users un-banned, admins dropped, a decade of
  production pain). It is the drawer bomb institutionalized as the
  design's core operation. Witnessed validity is the monotone repair of
  exactly this failure mode.
- **H. Server-ack hard authority** (authority ops effective only when
  acknowledged by a designated server). Buys: genuine recency —
  revocation takes effect at a gate, the concurrency that creates
  removal races never exists, offboarding is instant. This is the one
  alternative that *is* stronger where its precondition (a coordinator)
  holds. Breaks: requires that coordinator; as a base-protocol rule it
  forfeits offline-first authority operations. Adopted as an **opt-in
  profile policy** for server-mediated workspaces; the data plane stays
  offline-first either way.

## 8. Consistency model (summary)

Kept, unconditionally: deterministic convergence (same logs ⇒ same
verdicts); unforgeable attribution; no verdict destroys bytes; and — new
in this revision — **monotone validity**: acknowledged history is never
unwound by any mechanism.

Given up, provably necessarily (no coordinator + offline writes): recency.
There is no instant with one global answer to "is X a member?" — only
frontier-relative answers converging with propagation. Revocation latency
is human-time; read access is rotated, not revoked; insider misuse is
answered by removal, not prevention. Workspaces that want recency opt into
the server-ack profile (§7.H).

## 9. Open items

- Bracket cadence: normative guidance that `CommitContainers` accompanies
  every atomic write batch — both the data-ack closure (§4) and the
  inclusion filter assume routine brackets; specify client behavior when
  a peer's data runs far ahead of its brackets (widening transient).
- Ack-gated materialization: spec the apply/acknowledge ordering contract
  and the permitted transient-rollback window (§4).
- Seal manifest encoding: `RemoveDevice(self)` carrying final data-log
  heads; interaction with logout-while-offline (seal locally, sync later).
- `ctl/op.rs` shape change: drop `ctl_cut`/`hlc_cut` from `RemoveDevice`;
  self-target case carries the manifest.
- Default-profile ratification gate: exact CEL, admin-count activation
  threshold, single-admin degradation.
- CRDT semantics for late-validation (§2 resurrection = late delivery —
  confirm per reducer that this is literally ordinary merge, no special
  casing; tables via indexed oplog, ydoc via normal re-reduction).
- Conformance vectors: §6 walkthroughs as fixtures, including at least
  one pending→valid late-witness delivery and a fixture asserting the
  absence of valid→invalid transitions across any arrival order.
