#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, symbol_short, Address, BytesN, Env, String, Vec};

// Depending on `orbit-contract` as a normal crate would link its
// `#[contractimpl]`-generated wasm exports (e.g. `initialize`) into this
// contract's own binary, colliding with the factory's identically-named
// exports. `contractimport!` instead pulls in only the compiled child
// contract's XDR interface + a `Client` for cross-contract calls, so this
// binary stays a factory and nothing else. Requires `orbit-contract` to have
// already been built (`cargo build --target wasm32v1-none --release -p
// orbit-contract && stellar contract optimize ...`) before this crate builds.
mod orbit_wasm {
    soroban_sdk::contractimport!(
        file = "../target/wasm32v1-none/release/orbit_contract.optimized.wasm"
    );
}

// Soroban only embeds a `#[contracttype]`'s full definition in a contract's
// published interface when it's defined in the same crate as `#[contract]`
// — a shared types crate compiles fine but silently drops these from both
// contracts' specs, breaking CLI/SDK introspection (confirmed via `stellar
// contract info interface`). So these are duplicated verbatim from
// orbit-contract rather than shared, and converted to `orbit_wasm`'s
// re-generated equivalents only at the cross-contract call site below.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[contracttype]
pub enum PayoutOrder {
    Fixed,
    Random,
    Auction,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[contracttype]
pub enum Frequency {
    Daily,
    Weekly,
    Monthly,
}

fn to_wasm_frequency(f: Frequency) -> orbit_wasm::Frequency {
    match f {
        Frequency::Daily => orbit_wasm::Frequency::Daily,
        Frequency::Weekly => orbit_wasm::Frequency::Weekly,
        Frequency::Monthly => orbit_wasm::Frequency::Monthly,
    }
}

fn to_wasm_payout_order(p: PayoutOrder) -> orbit_wasm::PayoutOrder {
    match p {
        PayoutOrder::Fixed => orbit_wasm::PayoutOrder::Fixed,
        PayoutOrder::Random => orbit_wasm::PayoutOrder::Random,
        PayoutOrder::Auction => orbit_wasm::PayoutOrder::Auction,
    }
}

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Admin,
    WasmHash,
    NextId,
    Orbit(u32),
    AllOrbits,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum Error {
    AlreadyInitialized = 1,
    NotAdmin = 2,
    WasmHashNotSet = 3,
}

#[contract]
pub struct OrbitFactory;

#[contractimpl]
impl OrbitFactory {
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::NextId, &0u32);
        env.storage()
            .instance()
            .set(&DataKey::AllOrbits, &Vec::<Address>::new(&env));
        Ok(())
    }

    /// Admin-only: registers the WASM hash of the orbit-contract build that
    /// `create_orbit` should instantiate. Must be uploaded to the network
    /// separately first (`stellar contract upload`).
    pub fn set_wasm_hash(env: Env, wasm_hash: BytesN<32>) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotAdmin)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::WasmHash, &wasm_hash);
        Ok(())
    }

    /// Deploys a fresh orbit-contract instance for `creator` and initializes
    /// it in the same transaction. Each orbit gets its own contract address,
    /// deterministically derived from this factory's address plus a
    /// monotonically increasing id (used as the deploy salt) so addresses
    /// never collide.
    #[allow(clippy::too_many_arguments)]
    pub fn create_orbit(
        env: Env,
        creator: Address,
        token: Address,
        name: String,
        contribution_amount: i128,
        frequency: Frequency,
        payout_order: PayoutOrder,
        stake_bps: u32,
        total_rounds: u32,
        grace_period_secs: u64,
    ) -> Result<Address, Error> {
        creator.require_auth();
        let wasm_hash: BytesN<32> = env
            .storage()
            .instance()
            .get(&DataKey::WasmHash)
            .ok_or(Error::WasmHashNotSet)?;

        let mut next_id: u32 = env.storage().instance().get(&DataKey::NextId).unwrap_or(0);
        let mut salt_bytes = [0u8; 32];
        salt_bytes[28..32].copy_from_slice(&next_id.to_be_bytes());
        let salt = BytesN::from_array(&env, &salt_bytes);

        let deployed_address = env
            .deployer()
            .with_current_contract(salt)
            .deploy_v2(wasm_hash, ());

        let orbit_client = orbit_wasm::Client::new(&env, &deployed_address);
        orbit_client.initialize(
            &creator,
            &token,
            &name,
            &contribution_amount,
            &to_wasm_frequency(frequency),
            &to_wasm_payout_order(payout_order),
            &stake_bps,
            &total_rounds,
            &grace_period_secs,
        );

        env.storage()
            .instance()
            .set(&DataKey::Orbit(next_id), &deployed_address);
        let mut all: Vec<Address> = env.storage().instance().get(&DataKey::AllOrbits).unwrap();
        all.push_back(deployed_address.clone());
        env.storage().instance().set(&DataKey::AllOrbits, &all);

        env.events().publish(
            (symbol_short!("created"), creator),
            (next_id, deployed_address.clone()),
        );

        next_id += 1;
        env.storage().instance().set(&DataKey::NextId, &next_id);

        Ok(deployed_address)
    }

    pub fn get_orbit(env: Env, orbit_id: u32) -> Option<Address> {
        env.storage().instance().get(&DataKey::Orbit(orbit_id))
    }

    pub fn get_all_orbits(env: Env) -> Vec<Address> {
        env.storage()
            .instance()
            .get(&DataKey::AllOrbits)
            .unwrap_or(Vec::new(&env))
    }

    pub fn orbit_count(env: Env) -> u32 {
        env.storage().instance().get(&DataKey::NextId).unwrap_or(0)
    }
}

mod test;
