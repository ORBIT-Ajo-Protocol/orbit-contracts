# Error codes

Every `Result<_, Error>`-returning method can fail with one of these. Each
one below has the scenario that actually triggers it in this codebase today
(grep-verified, not guessed).

## `orbit-contract`

| Code | Error | Triggered when |
|---|---|---|
| 1 | `AlreadyInitialized` | `initialize` called on a group that already has config set |
| 2 | `NotAdmin` | **Not currently returned anywhere** — see note below |
| 3 | `NotPending` | `add_member` / `lock_stake` / `activate` called after the group has already left `Pending` |
| 4 | `NotActive` | `contribute` / `place_bid` / `propose_dispute` called while the group isn't `Active` (still `Pending`, or already `Completed`) |
| 5 | `GroupFull` | `add_member` once `total_rounds` members are already seated |
| 6 | `RotationIndexTaken` | `add_member` with a `rotation_index` another member already holds |
| 7 | `RotationIndexOutOfRange` | `add_member` with `rotation_index >= total_rounds` |
| 8 | `IncompleteRotation` | `activate` before every rotation slot is filled |
| 9 | `MemberNotFound` | any lookup (`get_member_info` internals, `vote_slash`, `propose_dispute`, ...) for an address never seated via `add_member` |
| 10 | `MemberNotActive` | `contribute` by a member who is `Invited` or `Defaulted`, not `Active`; or `propose_dispute` naming a `defaulter` who isn't `Active` |
| 11 | `AlreadyContributed` | `contribute` called twice by the same member in the same round |
| 12 | `NotEligibleForPayout` | `resolve_recipient` can't find a valid recipient — e.g. every member has already received a payout (shouldn't happen in normal operation, a defensive case) |
| 13 | `AlreadyVoted` | `vote_slash` called twice by the same voter on the same dispute |
| 14 | `VoterIneligible` | `vote_slash` by the defaulter themselves, or by a non-`Active` member; or `propose_dispute` by a seated-but-not-`Active` member (an `Invited` or already-`Defaulted` address) |
| 15 | `DisputeNotFound` | `vote_slash` / `finalize_dispute` with a `dispute_id` that was never opened |
| 16 | `DisputeAlreadyResolved` | `vote_slash` / `finalize_dispute` on a dispute that already resolved (either by unanimous vote or a prior `finalize_dispute`) |
| 17 | `NotYetOverdue` | `propose_dispute` before the grace period has elapsed since round start, or when the named defaulter has already contributed this round; `finalize_dispute` before the grace period has re-elapsed since the dispute opened |
| 18 | `AlreadyMember` | `add_member` for an address already seated in this group |
| 19 | `InvalidBid` | `place_bid` with `bid_amount <= 0` |
| 20 | `NotAuctionOrder` | `place_bid` on a group whose `payout_order` isn't `Auction` |
| 21 | `BiddingClosed` | `place_bid` by a member who has already received a payout this cycle |
| 22 | `StakeNotDue` | `lock_stake` by a member not currently `Invited` (already staked, or not seated); `activate` when any seated member's status isn't `Active` yet |

**Note on `NotAdmin`:** it's defined but not currently reachable — every
admin-only method (`add_member`, `activate`) enforces the admin check via
`config.admin.require_auth()` directly, which fails with Soroban's own
authorization trap rather than this contract-level error. Left as-is rather
than force a code path to use it: changing this would mean wrapping
`require_auth()` calls in a manual signer check, adding complexity to swap
one failure mode (a clear host-level auth trap) for another (a custom error
that's arguably less informative about *why* it failed). Flagged here so
it's a documented, deliberate observation rather than a silent gap.

## `orbit-factory`

| Code | Error | Triggered when |
|---|---|---|
| 1 | `AlreadyInitialized` | `initialize` called on a factory that's already been initialized |
| 2 | `NotAdmin` | `set_wasm_hash` called before the factory has been initialized (no admin on record) |
| 3 | `WasmHashNotSet` | `create_orbit` called before `set_wasm_hash` has registered which `orbit-contract` build to deploy |
