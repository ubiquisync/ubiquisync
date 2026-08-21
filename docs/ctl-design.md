## Ops/Effects

Head: (mmr root hash, peer id, log idx)

head_acks: reference containers and heads from peer logs or my own logs

- RemoveUser
  - cap_ref: remove_user capability head in ctl log
  - head_acks: all containers from all peers where i have observed this user (includes server heads), this
    defines a witness boundary beyond which i cannot observe any of these containers unless i observe
    a witness who saw ack'ed beyond this boundary without having ack'ed my removal first
  - key_wraps: for any containers where the user was a member
- RemoveDevice (user's own device):
  - cap_ref: no cap needed, but there could be policies which applie
  - head_acks: container heads for removed device
  - key_wraps: for any containers device was a member of
- RemoveDevice (other user's):
  - do we even allow this???
- RemoveDeviceSelf:
  - this should basically be the last op in the ctl log
    of a device which is _logging out_ and should
    include heads for all its containers which force
    witness boundaries
- RemoveUserSelf:
  - should include heads of all containers on all known devices
  - should we allow this???
- SetContainerPermission (Write -> None):
  - member: a user or group
  - scope: a container
  - cap_ref: container_admin(container)
  - head_acks: all heads for this container for removed peers
    directly or based on group membership, setting witness boundary.
    my write head for this container (so i am forced to rotate my key)
  - key_wraps
- SetContainerPermission (Write -> Read):
  - same as for Write -> None except:
  - head_acks: all heads for removed peers, but not my write head (i don't need to rotate key)
  - key_wraps: not needed
- SetContainerPermission (Read -> None):
  - same as for Write -> None except:
  - head_acks: just my write head because i need to rotate keys,
    but the witness boundary should _already_ be sealed for these peers -
    if they _never_ had write access, there should have been a standing
    proactive anti-witness boundary (TODO)
  - key_wraps: same as Write -> None
- SetContainerPermission (None -> Read):
  - key_wraps needed for added peers but no rotation needed
- SetContainerPermission (None -> Write):
  - key_wraps needed for added peers
  - no acks needed, but any standing anti-witness boundary on these peers must 
    be lifted (TODO), this sort of permission based boundary
    should probably be an implicit check rather than a
    per stream status field (non-empty witness boundaries
    are set per-stream because the boundary was set per-stream and could ove)
- SetContainerPermission (Read -> Write):
  - no key wraps or acks needed, just basically lifting the anti-witness flag
- AdmitDevice:
  - key wraps needed for the new device but no rotation needed
- RemoveRecoveryKey:
  - head_acks: do recovery keys have _write_ permission at all??
    i think maybe they can only be used to write to the ctl log to
    add a new device, so we need a ctl ack for the recover key
    but it shouldn't be permitted to write to other containers
  - key_wraps: basically the same as remove device since since recovery keys
    should get all valid key wraps for the user
  - should recovery keys be able to self remove?? (then they wouldn't do key wraps)
  - TODO: i guess recovery keys need genesis and are a _type_ of peer??
  - OR: recovery keys aren't peers at all, a newly added device can simply
    can simply include a recovery key signature in its genesis and start
    writing with the key wraps the recovery key shared with it and
    if it wants, the new device can take this as an opportunity to
    rotate the recovery key. BUT does this introduce the possibility of
    a race condition between recovery key removal and device genesis
    that doesn't match our other witness rules??? or maybe its
    fine - if the newly minted ctl log was witnessed by some honest peer
    before it witnessed the key removal maybe it's valid??
  - NOTE: we might need to be careful about how many devices/recovery keys
    we let users add in encrypted workspaces - it can cause lots of key
    wrap churn...
- Grant(cap):
  cap_ref: admin(cap)
- Revoke(cap):
  cap_ref: admin(cap)
- AdmitUser:
  cap_ref: add_user
  acks: doesn't need any
  key_wraps: only needed for containers user is added to by default,
  no rotation needed, but if the user doesn't have a device, what
  are we wrapping for, maybe admit user needs to reference either a
  device or ack some server custodial knowledge of the user
  (servers would usually be the one admitting the user in this case)
- Policies: if we allow policies which govern the usage of
  capabilities a user holds (such as enforcing number of approvals for an op),
  then I think whenever such "policy-bound" capabilities are used
  the user needs to reference _both_ its capability handle and the
  _policy_ it considers valid. And then policies can get
  replaced with a newer policy and the newer policy has a ref to that,
  and then peers must ack policy replacement and we must maintain
  a table of acks just like we do for cap removals.
  If not, we do need to have sort of policy visibility table, i.e.
  peer_id, policy_id -> version
- TODO: fork witnessing


## Acks
- on RemoveUser (REQUIRED):
  - ack any container heads i observed passed the witness boundary
    that is either directly or transitively in the RemoveUser op
    (transitive boundaries should be computed from previous observations
    of this user's logs - TODO how to track and compute that)
  - key wrap for any users/devices who didn't get the key wraps
    from the RemoveUser op, but who would be read excluded from
    any containers they are members of which had their keys rotated
    because of the removal
- on SetContainerPermission (Write -> None) (REQUIRED):
  - ack of any peer heads beyond witness boundary for this container
  - key wraps for any users/devices now excluded who shouldn't be
  - ack of _my_ write head on this container because i will need to rotate keys
- SetContainerPermission (Write -> Read) (REQUIRED):
  - ack of any peer heads beyond witness boundary for this container
  - no key wraps or acks of my write head needed
- SetContainerPermission (Read -> None) (REQUIRED):
  - ack of just my write head ack because i need to rotate keys
    but the removed peer's witness boundary _should_ already be frozen
  - key wraps for any peers who should have got the wrap
- SetContainerPermission (None -> Read) (NOT NEEDED):
  - key_wraps needed for added peers missed in share but no rotation or other acks needed
- SetContainerPermission (None -> Write) (NOT NEEDED):
  - key_wraps needed for added peers missed in share but no rotation or other acks needed
- SetContainerPermission (Read -> Write) (NOTHING NEEDED)
- RemoveDevice (user's own device) (REQUIRED):
  - ack any device container heads past the witness boundary in the op
  - any key wraps that were missed
- RemoveDeviceSelf (REQUIRED):
  - ack any device container heads past the witness boundary in the op
  - the device could not have wrapped keys, so the first witness should attempt
    to do it - this does risk a race condition that should be self-healing,
    we just need to decide whether there are any rules on which key is
    canonical, or maybe both should be allowed
- RemoveUserSelf (REQUIRED):
  - mostly same as RemoveDeviceSelf but acks and key wraps
    cover all devices we know of for that user
- AdmitDevice (NOT NEEDED):
  - key wraps needed for any _containers_ the device is a member
    of for which they didn't already get a wrap
- RemoveRecoveryKey (REQUIRED):
  - any key wraps that were missing when the removal op wrapped
  - a ctl head ack if it is past the recovery key head IF recovery keys
    are real peers, if they're not real peers this isn't needed
    but then maybe we have a race condition with a genesis signed
    by a recovery key with the alternate design
- Grant (NOT NEEDED):
  - ack required before usage however
- Revoke (REQUIRED):
  - i think just an ack of some peer ctl log which contains the removal in its fold
- change policy (REQUIRED):
  - just like revoke if we support this
- AdmitUser (NOT NEEDED):
  - nothing needed afaik



## Key Wraps

Q: when wrapping keys should we also encrypt the retired
key with the new key so that any new members only need
a single wrap to read the entire history??






  




