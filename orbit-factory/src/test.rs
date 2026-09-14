#![cfg(test)]

use super::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::token::{StellarAssetClient, TokenClient};

fn create_token<'a>(env: &Env, admin: &Address) -> (TokenClient<'a>, StellarAssetClient<'a>) {
    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    let address = sac.address();
    (
        TokenClient::new(env, &address),
        StellarAssetClient::new(env, &address),
    )
}

fn setup(env: &Env) -> (Address, Address, TokenClient<'static>, StellarAssetClient<'static>) {
    let admin = Address::generate(env);
    let token_admin = Address::generate(env);
    let (token, token_admin_client) = create_token(env, &token_admin);

    let factory_id = env.register(OrbitFactory, ());
    let factory = OrbitFactoryClient::new(env, &factory_id);
    factory.initialize(&admin);

    let wasm_hash = env.deployer().upload_contract_wasm(orbit_wasm::WASM);
    factory.set_wasm_hash(&wasm_hash);

    (factory_id, admin, token, token_admin_client)
}

#[test]
fn create_orbit_deploys_and_initializes_a_distinct_contract() {
    let env = Env::default();
    env.mock_all_auths();

    let (factory_id, _admin, token, _sac) = setup(&env);
    let factory = OrbitFactoryClient::new(&env, &factory_id);
    let creator = Address::generate(&env);

    let orbit_1 = factory.create_orbit(
        &creator,
        &token.address,
        &String::from_str(&env, "Lagos Solar Orbit"),
        &50_000_000i128,
        &Frequency::Weekly,
        &PayoutOrder::Fixed,
        &1000u32,
        &5u32,
        &172800u64,
    );
    let orbit_2 = factory.create_orbit(
        &creator,
        &token.address,
        &String::from_str(&env, "Abuja Galaxy Orbit"),
        &150_000_000i128,
        &Frequency::Monthly,
        &PayoutOrder::Random,
        &1500u32,
        &5u32,
        &172800u64,
    );

    assert_ne!(orbit_1, orbit_2);
    assert_eq!(factory.orbit_count(), 2);
    assert_eq!(factory.get_orbit(&0), Some(orbit_1.clone()));
    assert_eq!(factory.get_orbit(&1), Some(orbit_2.clone()));
    assert_eq!(factory.get_all_orbits(), Vec::from_array(&env, [orbit_1.clone(), orbit_2.clone()]));

    let orbit_client = orbit_wasm::Client::new(&env, &orbit_1);
    let config = orbit_client.get_config();
    assert_eq!(config.contribution_amount, 50_000_000i128);
    assert_eq!(config.total_rounds, 5);
    let state = orbit_client.get_state();
    assert_eq!(state.status, orbit_wasm::OrbitStatus::Pending);
}

#[test]
fn deployed_orbit_is_immediately_usable_end_to_end() {
    let env = Env::default();
    env.mock_all_auths();

    let (factory_id, _admin, token, token_admin_client) = setup(&env);
    let factory = OrbitFactoryClient::new(&env, &factory_id);
    let creator = Address::generate(&env);

    let contribution = 40_000_000i128;
    let orbit_address = factory.create_orbit(
        &creator,
        &token.address,
        &String::from_str(&env, "Enugu Solar Orbit"),
        &contribution,
        &Frequency::Weekly,
        &PayoutOrder::Fixed,
        &0u32,
        &3u32,
        &172800u64,
    );
    let orbit_client = orbit_wasm::Client::new(&env, &orbit_address);

    let m0 = Address::generate(&env);
    let m1 = Address::generate(&env);
    let m2 = Address::generate(&env);
    for (i, m) in [&m0, &m1, &m2].into_iter().enumerate() {
        orbit_client.add_member(m, &(i as u32));
        token_admin_client.mint(m, &(contribution * 5));
    }
    orbit_client.activate();

    for m in [&m0, &m1, &m2] {
        orbit_client.contribute(m);
    }

    let info = orbit_client.get_member_info(&m0).unwrap();
    assert!(info.has_received_payout);
    assert_eq!(orbit_client.get_state().current_round, 2);
}

#[test]
fn create_orbit_requires_wasm_hash_to_be_set() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let (token, _sac) = create_token(&env, &token_admin);
    let factory_id = env.register(OrbitFactory, ());
    let factory = OrbitFactoryClient::new(&env, &factory_id);
    factory.initialize(&admin);

    let creator = Address::generate(&env);
    let result = factory.try_create_orbit(
        &creator,
        &token.address,
        &String::from_str(&env, "No Wasm Orbit"),
        &10_000_000i128,
        &Frequency::Weekly,
        &PayoutOrder::Fixed,
        &0u32,
        &3u32,
        &172800u64,
    );
    assert_eq!(result, Err(Ok(Error::WasmHashNotSet)));
}

// Issue: "Handle wasm_hash rotation on factory without breaking existing
// orbits". `set_wasm_hash` only updates the factory's own pointer used by
// *future* create_orbit calls — an already-deployed orbit-contract instance
// is a separate contract at a fixed address with its own wasm baked in at
// deploy time, so rotating the factory's registered hash cannot retroactively
// touch it. This proves that invariant rather than just asserting it.
#[test]
fn wasm_hash_rotation_does_not_affect_already_deployed_orbits() {
    let env = Env::default();
    env.mock_all_auths();

    let (factory_id, _admin, token, token_admin_client) = setup(&env);
    let factory = OrbitFactoryClient::new(&env, &factory_id);
    let creator = Address::generate(&env);

    let contribution = 25_000_000i128;
    let orbit_before = factory.create_orbit(
        &creator,
        &token.address,
        &String::from_str(&env, "Pre-Rotation Orbit"),
        &contribution,
        &Frequency::Weekly,
        &PayoutOrder::Fixed,
        &0u32,
        &2u32,
        &172800u64,
    );
    let client_before = orbit_wasm::Client::new(&env, &orbit_before);

    // Re-register the same wasm under a "rotation" (the only real build this
    // test has available — what matters is that the factory's own pointer
    // changing doesn't reach into already-deployed instances at all).
    let rotated_hash = env.deployer().upload_contract_wasm(orbit_wasm::WASM);
    factory.set_wasm_hash(&rotated_hash);

    // The pre-rotation orbit is untouched: same address, same live config,
    // still fully usable.
    assert_eq!(factory.get_orbit(&0), Some(orbit_before.clone()));
    assert_eq!(client_before.get_config().contribution_amount, contribution);
    let m0 = Address::generate(&env);
    let m1 = Address::generate(&env);
    for (i, m) in [&m0, &m1].into_iter().enumerate() {
        client_before.add_member(m, &(i as u32));
        token_admin_client.mint(m, &(contribution * 5));
    }
    client_before.activate();
    for _round in 1..=2 {
        for m in [&m0, &m1] {
            client_before.contribute(m);
        }
    }
    assert_eq!(client_before.get_state().status, orbit_wasm::OrbitStatus::Completed);

    // New orbits deploy fine against the rotated hash, and the registry now
    // correctly holds both, addressed independently by id.
    let orbit_after = factory.create_orbit(
        &creator,
        &token.address,
        &String::from_str(&env, "Post-Rotation Orbit"),
        &contribution,
        &Frequency::Weekly,
        &PayoutOrder::Fixed,
        &0u32,
        &2u32,
        &172800u64,
    );
    assert_ne!(orbit_before, orbit_after);
    assert_eq!(factory.orbit_count(), 2);
    assert_eq!(factory.get_orbit(&1), Some(orbit_after));
}
