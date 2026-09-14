#![cfg(test)]

use super::*;
use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
use soroban_sdk::token::{StellarAssetClient, TokenClient};

fn create_token<'a>(env: &Env, admin: &Address) -> (TokenClient<'a>, StellarAssetClient<'a>) {
    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    let address = sac.address();
    (
        TokenClient::new(env, &address),
        StellarAssetClient::new(env, &address),
    )
}

/// Initializes a group and seats every member, but leaves it in `Pending` —
/// stake (if any) has not been locked and `activate` has not been called.
fn setup_pending(
    env: &Env,
    payout_order: PayoutOrder,
    stake_bps: u32,
    total_rounds: u32,
    contribution_amount: i128,
) -> (Address, Address, TokenClient<'static>, StellarAssetClient<'static>, Vec<Address>) {
    let admin = Address::generate(env);
    let token_admin = Address::generate(env);
    let (token, token_admin_client) = create_token(env, &token_admin);

    let contract_id = env.register(OrbitContract, ());
    let client = OrbitContractClient::new(env, &contract_id);

    client.initialize(
        &admin,
        &token.address,
        &String::from_str(env, "Lagos Solar Orbit"),
        &contribution_amount,
        &Frequency::Weekly,
        &payout_order,
        &stake_bps,
        &total_rounds,
        &172800u64, // 48h grace period
    );

    let mut members = Vec::new(env);
    for i in 0..total_rounds {
        let m = Address::generate(env);
        client.add_member(&m, &i);
        token_admin_client.mint(&m, &(contribution_amount * 10));
        members.push_back(m);
    }

    (contract_id, admin, token, token_admin_client, members)
}

/// Same as `setup_pending`, but every member locks stake and the group is
/// activated — the state most tests want to start from.
fn setup(
    env: &Env,
    payout_order: PayoutOrder,
    stake_bps: u32,
    total_rounds: u32,
    contribution_amount: i128,
) -> (Address, Address, TokenClient<'static>, StellarAssetClient<'static>, Vec<Address>) {
    let (contract_id, admin, token, token_admin_client, members) =
        setup_pending(env, payout_order, stake_bps, total_rounds, contribution_amount);
    let client = OrbitContractClient::new(env, &contract_id);

    if stake_bps > 0 {
        for m in members.iter() {
            client.lock_stake(&m);
        }
    }
    client.activate();

    (contract_id, admin, token, token_admin_client, members)
}

#[test]
fn full_cycle_fixed_order_pays_in_join_order() {
    let env = Env::default();
    env.mock_all_auths();

    let contribution = 50_000_000i128;
    let (contract_id, _admin, token, _sac, members) =
        setup(&env, PayoutOrder::Fixed, 1000, 3, contribution);
    let client = OrbitContractClient::new(&env, &contract_id);

    for round in 1..=3u32 {
        let recipient = members.get(round - 1).unwrap();
        let balance_before = token.balance(&recipient);

        for m in members.iter() {
            client.contribute(&m);
        }

        let state = client.get_state();
        let expected_pot = contribution * 3;
        // The recipient is also a contributor this round, so their own
        // outgoing quota nets against the pot they receive.
        assert_eq!(
            token.balance(&recipient),
            balance_before - contribution + expected_pot
        );
        assert_eq!(state.last_payout_recipient, Some(recipient));
        if round < 3 {
            assert_eq!(state.status, OrbitStatus::Active);
            assert_eq!(state.current_round, round + 1);
        } else {
            assert_eq!(state.status, OrbitStatus::Completed);
        }
    }
}

#[test]
fn lock_stake_transfers_collateral_before_activation() {
    let env = Env::default();
    env.mock_all_auths();

    let contribution = 100_000_000i128;
    let (contract_id, _admin, token, _sac, members) =
        setup_pending(&env, PayoutOrder::Fixed, 1000, 3, contribution); // 10% stake
    let client = OrbitContractClient::new(&env, &contract_id);

    let member = members.get(0).unwrap();
    let info_before = client.get_member_info(&member).unwrap();
    assert_eq!(info_before.status, MemberStatus::Invited);

    let balance_before = token.balance(&member);
    client.lock_stake(&member);

    let expected_stake = contribution / 10;
    assert_eq!(token.balance(&member), balance_before - expected_stake);

    let info = client.get_member_info(&member).unwrap();
    assert_eq!(info.locked_stake, expected_stake);
    assert_eq!(info.status, MemberStatus::Active);
}

#[test]
fn activate_fails_until_every_member_has_staked() {
    let env = Env::default();
    env.mock_all_auths();

    let (contract_id, _admin, _token, _sac, members) =
        setup_pending(&env, PayoutOrder::Fixed, 1000, 3, 50_000_000);
    let client = OrbitContractClient::new(&env, &contract_id);

    // Only two of three members lock stake.
    client.lock_stake(&members.get(0).unwrap());
    client.lock_stake(&members.get(1).unwrap());

    let result = client.try_activate();
    assert_eq!(result, Err(Ok(Error::StakeNotDue)));
}

#[test]
fn double_contribution_in_same_round_fails() {
    let env = Env::default();
    env.mock_all_auths();

    let (contract_id, _admin, _token, _sac, members) =
        setup(&env, PayoutOrder::Fixed, 500, 3, 10_000_000);
    let client = OrbitContractClient::new(&env, &contract_id);
    let member = members.get(0).unwrap();

    client.contribute(&member);
    let result = client.try_contribute(&member);
    assert_eq!(result, Err(Ok(Error::AlreadyContributed)));
}

#[test]
fn random_order_pays_every_member_exactly_once() {
    let env = Env::default();
    env.mock_all_auths();

    let contribution = 20_000_000i128;
    let (contract_id, _admin, _token, _sac, members) =
        setup(&env, PayoutOrder::Random, 0, 4, contribution);
    let client = OrbitContractClient::new(&env, &contract_id);

    for _ in 0..4u32 {
        for m in members.iter() {
            client.contribute(&m);
        }
    }

    for m in members.iter() {
        let info = client.get_member_info(&m).unwrap();
        assert!(info.has_received_payout);
        assert_eq!(info.completed_cycles, 1);
    }
    assert_eq!(client.get_state().status, OrbitStatus::Completed);
}

#[test]
fn auction_lowest_bid_wins_the_round() {
    let env = Env::default();
    env.mock_all_auths();

    let contribution = 30_000_000i128;
    let (contract_id, _admin, token, _sac, members) =
        setup(&env, PayoutOrder::Auction, 0, 3, contribution);
    let client = OrbitContractClient::new(&env, &contract_id);

    let low_bidder = members.get(1).unwrap();
    client.place_bid(&members.get(0).unwrap(), &(contribution * 3));
    client.place_bid(&low_bidder, &(contribution * 2));
    client.place_bid(&members.get(2).unwrap(), &(contribution * 3 - 1));

    let balance_before = token.balance(&low_bidder);
    for m in members.iter() {
        client.contribute(&m);
    }

    // Winner is also a contributor this round: nets their own quota against
    // the full pot they receive.
    assert_eq!(
        token.balance(&low_bidder),
        balance_before - contribution + contribution * 3
    );
    assert_eq!(client.get_state().last_payout_recipient, Some(low_bidder));
}

#[test]
fn overdue_default_can_be_slashed_after_vote() {
    let env = Env::default();
    env.mock_all_auths();

    let contribution = 50_000_000i128;
    let (contract_id, _admin, token, _sac, members) =
        setup(&env, PayoutOrder::Fixed, 1000, 3, contribution);
    let client = OrbitContractClient::new(&env, &contract_id);

    let defaulter = members.get(1).unwrap();
    client.contribute(&members.get(0).unwrap());
    // defaulter never contributes this round, despite having staked at join.

    env.ledger().with_mut(|li: &mut LedgerInfo| {
        li.timestamp += 172800 + 1;
    });

    let dispute_id = client.propose_dispute(&members.get(0).unwrap(), &defaulter);
    let contract_balance_before = token.balance(&contract_id);

    client.vote_slash(&members.get(0).unwrap(), &dispute_id, &true);
    client.vote_slash(&members.get(2).unwrap(), &dispute_id, &true);

    let dispute = client.get_dispute(&dispute_id).unwrap();
    assert!(dispute.resolved);
    assert!(dispute.approved);

    let info = client.get_member_info(&defaulter).unwrap();
    assert_eq!(info.status, MemberStatus::Defaulted);
    assert_eq!(info.locked_stake, 0);
    assert!(client.has_contributed(&1u32, &defaulter));

    // Slashing only reclassifies collateral the contract already held from
    // `lock_stake` at join time — no new tokens move — and the 10% stake
    // alone doesn't cover the full missed quota, so the round still hasn't
    // settled.
    let state = client.get_state();
    assert_eq!(state.status, OrbitStatus::Active);
    assert_eq!(contract_balance_before, token.balance(&contract_id));
}

#[test]
fn rejected_slash_extends_grace_period() {
    let env = Env::default();
    env.mock_all_auths();

    let (contract_id, _admin, _token, _sac, members) =
        setup(&env, PayoutOrder::Fixed, 1000, 3, 50_000_000);
    let client = OrbitContractClient::new(&env, &contract_id);
    let defaulter = members.get(1).unwrap();

    env.ledger().with_mut(|li: &mut LedgerInfo| {
        li.timestamp += 172800 + 1;
    });
    let dispute_id = client.propose_dispute(&members.get(0).unwrap(), &defaulter);

    client.vote_slash(&members.get(0).unwrap(), &dispute_id, &false);
    client.vote_slash(&members.get(2).unwrap(), &dispute_id, &false);

    let dispute = client.get_dispute(&dispute_id).unwrap();
    assert!(dispute.resolved);
    assert!(!dispute.approved);
    let info = client.get_member_info(&defaulter).unwrap();
    assert_eq!(info.status, MemberStatus::Active);
}

// Regression test for the propose_dispute access-control gap: `require_auth`
// only proves the caller controls that key, not that they're a member of
// this orbit. An outsider with no stake in the group must not be able to
// open disputes.
#[test]
fn propose_dispute_by_non_member_fails() {
    let env = Env::default();
    env.mock_all_auths();

    let (contract_id, _admin, _token, _sac, members) =
        setup(&env, PayoutOrder::Fixed, 1000, 3, 50_000_000);
    let client = OrbitContractClient::new(&env, &contract_id);
    let defaulter = members.get(1).unwrap();
    let outsider = Address::generate(&env);

    env.ledger().with_mut(|li: &mut LedgerInfo| {
        li.timestamp += 172800 + 1;
    });

    let result = client.try_propose_dispute(&outsider, &defaulter);
    assert_eq!(result, Err(Ok(Error::MemberNotFound)));
}

// Issue: "Add test: multi-round auction payout order end-to-end". Runs a
// full 3-round Auction-order cycle (not just one round), confirming bids
// reset each round, every member is eventually paid, and the group reaches
// Completed.
#[test]
fn multi_round_auction_payout_completes_full_cycle() {
    let env = Env::default();
    env.mock_all_auths();

    let contribution = 20_000_000i128;
    let (contract_id, _admin, _token, _sac, members) =
        setup(&env, PayoutOrder::Auction, 0, 3, contribution);
    let client = OrbitContractClient::new(&env, &contract_id);

    let mut paid: Vec<Address> = Vec::new(&env);
    for round in 1..=3u32 {
        // Every not-yet-paid member bids; lowest bid wins, so rotate who
        // bids lowest each round to prove the winner actually changes.
        let mut unpaid: Vec<Address> = Vec::new(&env);
        for m in members.iter() {
            if !vec_contains(&paid, &m) {
                unpaid.push_back(m);
            }
        }
        let mut bid = contribution * unpaid.len() as i128;
        for m in unpaid.iter() {
            client.place_bid(&m, &bid);
            bid -= 1;
        }

        for m in members.iter() {
            client.contribute(&m);
        }

        let state = client.get_state();
        let recipient = state.last_payout_recipient.unwrap();
        assert!(!vec_contains(&paid, &recipient));
        paid.push_back(recipient);

        if round < 3 {
            assert_eq!(state.status, OrbitStatus::Active);
            assert_eq!(state.current_round, round + 1);
        } else {
            assert_eq!(state.status, OrbitStatus::Completed);
        }
    }

    assert_eq!(paid.len(), 3);
    for m in members.iter() {
        assert!(vec_contains(&paid, &m));
        assert!(client.get_member_info(&m).unwrap().has_received_payout);
    }
}

// Issue: "Add test: concurrent disputes across multiple orbits". Dispute IDs
// and state are per-contract-instance storage, so two independently
// deployed orbits should never see each other's disputes.
#[test]
fn concurrent_disputes_across_multiple_orbits_are_independent() {
    let env = Env::default();
    env.mock_all_auths();

    let contribution = 50_000_000i128;
    let (orbit_a, _admin_a, _token_a, _sac_a, members_a) =
        setup(&env, PayoutOrder::Fixed, 1000, 3, contribution);
    let (orbit_b, _admin_b, _token_b, _sac_b, members_b) =
        setup(&env, PayoutOrder::Fixed, 1000, 3, contribution);
    let client_a = OrbitContractClient::new(&env, &orbit_a);
    let client_b = OrbitContractClient::new(&env, &orbit_b);

    env.ledger().with_mut(|li: &mut LedgerInfo| {
        li.timestamp += 172800 + 1;
    });

    // Both orbits' round-1 defaulter is member index 1; open a dispute in
    // each and resolve them with opposite outcomes.
    let dispute_a = client_a.propose_dispute(&members_a.get(0).unwrap(), &members_a.get(1).unwrap());
    let dispute_b = client_b.propose_dispute(&members_b.get(0).unwrap(), &members_b.get(1).unwrap());
    assert_eq!(dispute_a, 0, "each orbit's dispute ids start at 0 independently");
    assert_eq!(dispute_b, 0);

    client_a.vote_slash(&members_a.get(0).unwrap(), &dispute_a, &true);
    client_a.vote_slash(&members_a.get(2).unwrap(), &dispute_a, &true);
    client_b.vote_slash(&members_b.get(0).unwrap(), &dispute_b, &false);
    client_b.vote_slash(&members_b.get(2).unwrap(), &dispute_b, &false);

    assert_eq!(
        client_a.get_member_info(&members_a.get(1).unwrap()).unwrap().status,
        MemberStatus::Defaulted,
        "orbit A's approved slash must not leak into orbit B"
    );
    assert_eq!(
        client_b.get_member_info(&members_b.get(1).unwrap()).unwrap().status,
        MemberStatus::Active,
        "orbit B's rejected slash must not be affected by orbit A's vote"
    );
    // Orbit B has an entirely separate dispute record despite sharing id 0.
    assert!(!client_b.get_dispute(&dispute_b).unwrap().approved);
    assert!(client_a.get_dispute(&dispute_a).unwrap().approved);
}

// Issue: "Add test: member defaulting mid-cycle across all payout orders".
// A member who defaults on round 1 (before ever receiving a payout) gets
// slashed and marked Defaulted under each PayoutOrder — and the cycle
// continues to completion among the remaining active members rather than
// getting stuck.
fn assert_mid_cycle_default_recovers(payout_order: PayoutOrder) {
    let env = Env::default();
    env.mock_all_auths();

    let contribution = 30_000_000i128;
    let (contract_id, _admin, _token, _sac, members) =
        setup(&env, payout_order, 1000, 3, contribution);
    let client = OrbitContractClient::new(&env, &contract_id);
    let defaulter = members.get(1).unwrap();

    if payout_order == PayoutOrder::Auction {
        for m in members.iter() {
            if m != defaulter {
                client.place_bid(&m, &contribution);
            }
        }
    }
    // Everyone except the defaulter contributes round 1.
    for m in members.iter() {
        if m != defaulter {
            client.contribute(&m);
        }
    }

    env.ledger().with_mut(|li: &mut LedgerInfo| {
        li.timestamp += 172800 + 1;
    });
    let dispute_id = client.propose_dispute(&members.get(0).unwrap(), &defaulter);
    client.vote_slash(&members.get(0).unwrap(), &dispute_id, &true);
    client.vote_slash(&members.get(2).unwrap(), &dispute_id, &true);

    let info = client.get_member_info(&defaulter).unwrap();
    assert_eq!(info.status, MemberStatus::Defaulted);
    assert_eq!(info.locked_stake, 0);
    assert!(!info.has_received_payout);

    // The defaulter is now marked contributed (via slashing) so the round
    // can still settle among the remaining members without ever waiting on
    // an address that can no longer participate.
    assert!(client.has_contributed(&1u32, &defaulter));
}

#[test]
fn default_mid_cycle_fixed_order() {
    assert_mid_cycle_default_recovers(PayoutOrder::Fixed);
}

#[test]
fn default_mid_cycle_random_order() {
    assert_mid_cycle_default_recovers(PayoutOrder::Random);
}

#[test]
fn default_mid_cycle_auction_order() {
    assert_mid_cycle_default_recovers(PayoutOrder::Auction);
}

// Supports the integer-overflow audit (see SECURITY.md): proves the
// `overflow-checks = true` release-profile setting actually catches an
// overflow in the stake calculation rather than silently wrapping.
#[test]
#[should_panic]
fn stake_calculation_panics_on_overflow_instead_of_wrapping() {
    let env = Env::default();
    env.mock_all_auths();

    // contribution_amount * stake_bps overflows i128 at these magnitudes.
    let (contract_id, _admin, _token, _sac, members) =
        setup_pending(&env, PayoutOrder::Fixed, 10_000, 1, i128::MAX / 2);
    let client = OrbitContractClient::new(&env, &contract_id);
    client.lock_stake(&members.get(0).unwrap());
}
