# Mainnet deployment checklist

This protocol is currently deployed to **testnet only** (see `README.md` for
addresses) and has not gone through the process below. This is what would
need to happen first — written now, while the reasoning for each item is
fresh, rather than reconstructed later under deadline pressure.

## Before writing any deployment command

- [ ] **Third-party audit.** `SECURITY.md` documents a manual review pass
      (access control, integer overflow) with one real finding already
      fixed, but that's not a substitute for an independent audit — get one
      before any contract holding real user funds goes live.
- [ ] **Resolve the admin key question.** `SECURITY.md` documents the
      decision to use a single EOA admin for the testnet phase specifically
      *because* no real funds are at risk. That reasoning doesn't hold for
      mainnet — decide on a multisig or other admin-key policy before
      deploying with real funds behind it.
- [ ] **Decide the token.** Contracts are token-agnostic (`Config.token` is
      just an `Address`). For mainnet this needs to be a real, audited
      Stellar Asset Contract (e.g. mainnet USDC) — confirm the issuer,
      confirm it has no transfer restrictions that would break `contribute`
      / `maybe_settle_round`'s token transfers, and test against it on
      testnet first (tracked in the contracts repo's existing "Test and
      document real USDC Stellar Asset Contract integration" issue).
- [ ] **Fuzz the slashing/payout math** beyond the current unit test suite
      — property-based tests around stake percentages, round counts, and
      payout amounts at the boundaries (0%, 100% stake; 1-member and
      very-large groups).
- [ ] **Gas/fee profiling.** No real profiling has been done yet (see the
      open "Gas/fee optimization pass" issue) — measure `contribute` and
      `maybe_settle_round`'s resource costs against mainnet fee schedules
      before committing to them being acceptable.

## Deployment steps (once the above is resolved)

Mirrors the testnet process in `README.md`'s "Deploying to testnet", with
`--network mainnet` in place of `--network testnet`, plus:

1. Build and optimize `orbit-contract` (build order matters — see README).
2. Upload the optimized `orbit-contract` wasm; record the hash **and the
   upload transaction** for the audit trail.
3. Deploy `orbit-factory`; record the deployed address.
4. Initialize the factory with the **agreed-upon admin** (multisig or
   otherwise resolved per the checklist above, not a throwaway single key).
5. Register the wasm hash via `set_wasm_hash`.
6. Deploy one real orbit as a canary with a small, real contribution amount
   before advertising the protocol publicly. Run it through at least one
   full round (contribute → payout) before trusting it with more.
7. Update every README and the frontend's live-contract links (all three
   repos currently point at testnet addresses) to the new mainnet addresses
   — don't leave testnet and mainnet addresses ambiguous in the same docs.

## After deployment

- [ ] Set up real monitoring on the indexer (see orbit-backend's "Add
      observability" and "Add error tracking/alerting" issues) — silent
      indexer failures on mainnet mean silently stale UI state.
- [ ] Publish the audit report and the addresses above somewhere permanent
      and linkable (not just a README that can be force-pushed over).
- [ ] Have a documented incident-response plan for a critical bug found
      post-deployment — given contracts are immutable by design (see
      `SECURITY.md`), the only remedy is deploying a replacement and
      migrating members, so know that process before you need it, not
      during an incident.
