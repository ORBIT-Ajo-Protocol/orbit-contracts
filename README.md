# ORBIT Ajo Protocol — Contracts

Soroban smart contracts implementing the ORBIT Ajo Protocol: a rotating
savings & credit association (ROSCA) with staked collateral and
member-voted default slashing.

## Layout

- `orbit-contract/` — the ROSCA group contract. One deployed instance per
  savings group (an "orbit"): membership, collateral staking, contributions,
  rotating payout (fixed / random / auction order), and dispute/slashing.
- `orbit-factory/` — deploys and tracks `orbit-contract` instances. Each
  orbit gets its own contract address, derived deterministically from the
  factory's address plus a monotonically increasing id.

`orbit-factory` depends on a **compiled artifact** of `orbit-contract`
(via `soroban_sdk::contractimport!`), not on its source crate — two
Soroban `#[contract]` crates can't depend on each other directly, since
their macros generate identically-named wasm exports that collide. This
means `orbit-contract` must be built to wasm *before* `orbit-factory` will
build at all (tests included).

## Build order

```sh
# 1. Build + optimize the group contract first — orbit-factory's
#    build depends on this artifact existing at target/wasm32v1-none/release/.
cargo build --target wasm32v1-none --release -p orbit-contract
stellar contract optimize --wasm target/wasm32v1-none/release/orbit_contract.wasm

# 2. Now everything builds/tests normally.
cargo test
cargo build --target wasm32v1-none --release -p orbit-factory
stellar contract optimize --wasm target/wasm32v1-none/release/orbit_factory.wasm
```

## Deploying to testnet

```sh
# Upload the group contract once, note the wasm hash it prints.
stellar contract upload --wasm target/wasm32v1-none/release/orbit_contract.optimized.wasm \
  --source <your-identity> --network testnet

# Deploy the factory.
stellar contract deploy --wasm target/wasm32v1-none/release/orbit_factory.optimized.wasm \
  --source <your-identity> --network testnet

# Initialize the factory and register the group contract's wasm hash.
stellar contract invoke --id <factory-id> --source <your-identity> --network testnet \
  -- initialize --admin <admin-address>
stellar contract invoke --id <factory-id> --source <your-identity> --network testnet \
  -- set_wasm_hash --wasm_hash <hash-from-upload-step>

# Deploy a group. Enum args must be JSON-quoted strings.
stellar contract invoke --id <factory-id> --source <your-identity> --network testnet \
  -- create_orbit --creator <address> --token <token-address> --name "Lagos Solar Orbit" \
  --contribution_amount 500000000 --frequency '"Weekly"' --payout_order '"Fixed"' \
  --stake_bps 1000 --total_rounds 5 --grace_period_secs 172800
```

## Notes

- `Frequency` and `PayoutOrder` are deliberately duplicated between
  `orbit-contract` and `orbit-factory` rather than shared via a common
  crate: Soroban only embeds a `#[contracttype]`'s full definition in a
  contract's published interface when it's defined in the *same* crate as
  `#[contract]` — a shared types crate compiles fine but silently drops the
  type definitions from both contracts' specs (confirmed via `stellar
  contract info interface`), breaking CLI/SDK introspection.
- Both enums are defined without explicit integer discriminants
  (`Fixed`, not `Fixed = 0`) so they encode as tagged unions rather than
  plain integer enums — the `stellar` CLI (as of v27) cannot parse plain
  integer-discriminant enums as invoke arguments.
- Collateral stake is collected upfront via `lock_stake` (before a group
  activates), not on a member's first contribution — this ensures a member
  who defaults on round 1 without ever contributing still has collateral on
  the line to slash.
- `PayoutOrder::Auction`'s bid only decides who wins the round; the winner
  still receives the full pot (matching Fixed/Random's custody invariants),
  not a discounted payout with rebate redistribution to other members.
