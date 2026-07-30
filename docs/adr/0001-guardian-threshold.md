# ADR 0001: Fixed 2-of-3 Guardian Quorum

## Status

Accepted (current implementation). Superseded in part by the per-will
configurable `guardian_threshold` parameter already accepted by `create_will`
(see "Current state" below); the hard cap `MAX_GUARDIANS = 3` remains a
protocol-level constant.

## Context

`WillContract` lets an owner name a set of guardians who can force an early
release of a will via `guardian_trigger`, without waiting out the full
check-in + grace period. This exists for the case where an owner is known to
be incapacitated (not just inactive) and beneficiaries or trusted parties
need a faster path than the deadline-based one.

Two numbers govern this mechanism:

- `MAX_GUARDIANS`, the maximum number of guardian addresses a single will may
  name.
- A per-will `guardian_threshold`, the number of distinct guardian votes
  required to force release.

Early designs of this contract fixed both of these: at most 3 guardians,
and exactly 2 of them (a simple majority) required to trigger. That
2-of-3 shape is still the value most callers pass today (the SDK/app's
default will-creation flow), and it is why this ADR exists — to record why
that shape was chosen as the default and what its tradeoffs are, even though
the threshold parameter is no longer hardcoded in the contract itself.

## Why 2-of-3

- **Majority-of-small-committee is the simplest quorum rule that tolerates a
  single bad actor.** With 3 guardians and a 2-vote threshold, no single
  guardian — malicious, compromised, or simply wrong about the owner's
  status — can force a release unilaterally. At the same time, the will
  isn't blocked by a single guardian being unreachable, since the other two
  are sufficient.
- **3 is small enough to keep guardian-list management cheap and safe.**
  Every guardian in the list is an address the owner must trust with the
  power to accelerate release of the entire balance. Larger committees
  increase the attack surface (more addresses that could be compromised,
  more social-engineering targets) without a proportional increase in
  safety, since the relevant property is "can a minority collude or be
  coerced," not "how many people are watching."
- **Odd-vs-even quorum math is simpler at N=3.** A 2-of-3 threshold has no
  tie case. Generalizing to arbitrary N reintroduces the usual questions
  around what happens at exact ties (round up? require strict majority?
  allow the owner to pick 50%?) that a fixed small N avoids entirely.
- **Matches the common real-world case.** Most people naming guardians for
  an inheritance-style contract think in terms of "a couple of people I
  trust," e.g. a spouse, a sibling, a lawyer — not a large multi-party DAO-like
  committee. Three is enough to model "majority of a small trusted group"
  without asking the owner to recruit and maintain a larger roster.

## Current state

As of this contract version, `guardian_threshold` is **not** hardcoded to 2 —
it is a `create_will` parameter validated to fall within `1..=guardians.len()`
(see `WillError::InvalidGuardianThreshold`). Callers remain free to configure
1-of-N, N-of-N, or anything in between, up to the hard `MAX_GUARDIANS = 3`
ceiling on list size. The `GUARDIAN_THRESHOLD = 2` constant in `lib.rs` is
retained as the documented default rationale above, not as an enforced value.

What is still fixed at the protocol level is the **cap of 3 guardians**. This
ADR's guardian-count reasoning above (small trusted committee, bounded attack
surface, simple validation) is the rationale for that cap remaining a
constant rather than a parameter.

## Known limitations

- **No configurable guardian count.** Wills that would benefit from a larger
  guardian committee (e.g. a family of 5, or an estate with multiple
  co-executors) cannot express that today; they're capped at 3 named
  guardians regardless of `guardian_threshold`.
- **No weighted-quorum nuance beyond vote weight.** `Guardian.weight` exists
  and threshold comparisons are weight-based, but in the common case where
  every guardian has weight 1, this collapses to a simple headcount — there
  is no notion of, say, requiring a specific guardian's vote (e.g. "the
  lawyer must always be one of the 2").
- **Threshold changes are per-guardian-list-update, not independently
  adjustable.** Changing `guardian_threshold` requires the owner to go
  through `update_beneficiaries`/guardian-update entry points and is subject
  to the same `GUARDIAN_COOLDOWN_DAYS` cooldown as guardian-list changes,
  which is a deliberate anti-griefing measure but also means it cannot be
  tuned instantly.

## Relation to the proposed configurable M-of-N feature (issue #6)

Issue #6 (from the first batch of proposed features) asks for a fully
configurable M-of-N guardian quorum — i.e., removing the `MAX_GUARDIANS = 3`
cap entirely and letting the owner pick both N (list size) and M (threshold)
without an upper bound. The per-will `guardian_threshold` parameter described
above is the "M" half of that feature, already implemented. Issue #6, if
picked up, would extend this ADR's reasoning by:

1. Raising or removing `MAX_GUARDIANS`, trading the "small trusted
   committee" simplicity argument above for flexibility that larger estates
   or multi-party arrangements need.
2. Re-examining the storage and CPU cost of guardian bookkeeping at larger N
   (see [docs/RESOURCE_COSTS.md](../RESOURCE_COSTS.md)), since `Vec<Guardian>`
   operations and cooldown checks scale with guardian-list length.
3. Deciding whether a per-will configurable cap (owner picks N up to some
   larger ceiling) is preferable to removing the ceiling altogether, to keep
   an upper bound on worst-case resource usage per will.

Until issue #6 is implemented, `MAX_GUARDIANS = 3` remains the enforced
ceiling, and this ADR is the record of why that number — and the 2-of-3
default built on top of it — was chosen.
