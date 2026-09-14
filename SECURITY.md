# Security notes

This documents the results of a manual review pass over both contracts —
access control, arithmetic safety — plus the deliberate decisions behind
admin key management and upgradeability. It's not a substitute for a
professional third-party audit before any mainnet deployment (see
`DEPLOYMENT.md`), but it's a real accounting of what was checked and found.

## Access control audit

Every state-changing method on both contracts was checked for two things:
does it call `require_auth()` on the right party, and does it verify that
party is actually who the method's doc comment claims they must be (not
just *someone* who signed).

| Contract | Method | Caller | Verified |
|---|---|---|---|
| `orbit-contract` | `initialize` | the intended admin (self-authorizing, once) | ✅ |
| `orbit-contract` | `add_member` | `config.admin` | ✅ |
| `orbit-contract` | `lock_stake` | the member themselves | ✅ |
| `orbit-contract` | `activate` | `config.admin` | ✅ |
| `orbit-contract` | `contribute` | the member themselves | ✅ |
| `orbit-contract` | `place_bid` | the member themselves | ✅ |
| `orbit-contract` | `propose_dispute` | any **active member** | ⚠️ found & fixed — see below |
| `orbit-contract` | `vote_slash` | any active member except the defaulter | ✅ |
| `orbit-contract` | `finalize_dispute` | anyone (intentionally permissionless) | ✅ by design |
| `orbit-factory` | `initialize` | the intended admin (self-authorizing, once) | ✅ |
| `orbit-factory` | `set_wasm_hash` | the stored admin | ✅ |
| `orbit-factory` | `create_orbit` | the creator (permissionless by design — anyone can deploy their own group) | ✅ |

**Finding, fixed:** `propose_dispute` called `proposer.require_auth()` —
which proves the caller controls that key — but never checked that
`proposer` is actually a seated, active member of the orbit. Any outsider
address could have opened disputes against a legitimately-overdue defaulter.
This couldn't move funds (dispute resolution still independently re-checks
voter membership in `vote_slash`), but it violated the documented "any
active member" invariant and allowed dispute-spam from non-participants.
Fixed by adding the same active-member check `vote_slash` already had for
voters. Covered by the regression test `propose_dispute_by_non_member_fails`.

## Integer overflow / underflow audit

All arithmetic on stake and pot values (`contribute`'s
`state.live_pot_balance += config.contribution_amount`, `lock_stake`'s
`config.contribution_amount * config.stake_bps as i128 / 10_000`, the vote
tally increments, `member_count += 1`, etc.) relies on
`overflow-checks = true` in `Cargo.toml`'s `[profile.release]` — the profile
actually used to build the deployed wasm (confirmed against the build
commands in `README.md`). With that setting, Soroban panics and aborts the
transaction on overflow rather than silently wrapping. This is verified,
not just asserted: `stake_calculation_panics_on_overflow_instead_of_wrapping`
constructs a contribution amount that overflows `i128` in the stake
calculation and asserts the call panics.

No arithmetic in either contract uses unchecked operations or `unsafe`, and
none of it is reachable pre-`initialize` (config values come from the
deployer, not user input, before any member can act on them).

## Admin key: single EOA, not multisig (issue: "Consider multisig admin")

**Decision: not implementing multisig for this phase.** Each orbit's admin
(and the factory's admin) is a single externally-owned account. This is a
real centralization point — a compromised or lost admin key can't `activate`
a pending group or rotate the factory's `wasm_hash` — but member funds are
not at risk from it: `contribute`, `lock_stake`, and payout settlement are
all member- or protocol-driven, not admin-gated, so a malicious admin can't
move member funds directly.

Rationale for deferring: Soroban has no native multisig primitive — it
would mean deploying and integrating a separate multisig/policy contract
(e.g. an account contract with custom signature rules), which is a
meaningful scope increase and its own audit surface, for a protocol
currently in testnet demo phase with no real funds at risk. This is the
right thing to revisit **before any mainnet deployment with real user
funds** — tracked in `DEPLOYMENT.md`.

## Upgradeability: immutable by design (issue: "Document upgrade strategy")

**Decision: neither contract has an upgrade mechanism, and none is planned.**
Once `orbit-contract` wasm is uploaded and a group is deployed against it,
that group's logic is fixed for its lifetime — there's no
admin-gated `update_current_contract_wasm` call anywhere in this codebase.
`orbit-factory.set_wasm_hash` does **not** upgrade existing orbits; it only
changes which wasm *future* `create_orbit` calls deploy (see the "wasm_hash
rotation" note below and the regression test proving it).

Rationale: immutability is the simpler, more auditable trust model for a
protocol whose whole value proposition is "the rules can't be changed out
from under you after you've staked collateral." The cost is that fixing a
bug in an already-deployed orbit requires deploying a new orbit and members
migrating manually — an acceptable tradeoff at this stage, and one worth
re-examining only alongside the multisig-admin question above if this ever
moves toward mainnet.

## `wasm_hash` rotation safety (issue: "Handle wasm_hash rotation")

Verified, not just documented: `wasm_hash_rotation_does_not_affect_already_deployed_orbits`
deploys an orbit, rotates the factory's registered `wasm_hash`, and proves
the earlier orbit's address, config, and ability to run a full contribute
cycle to completion are all unaffected — because it's a separate deployed
contract instance with its own wasm baked in at `deploy_v2` time, not
something the factory's own storage pointer can reach into retroactively.
