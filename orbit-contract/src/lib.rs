#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, Env,
    String, Vec,
};

const DAY_IN_LEDGERS: u32 = 17280; // ~5s per ledger close
const INSTANCE_BUMP: u32 = 30 * DAY_IN_LEDGERS;
const INSTANCE_THRESHOLD: u32 = INSTANCE_BUMP - DAY_IN_LEDGERS;
const PERSISTENT_BUMP: u32 = 90 * DAY_IN_LEDGERS;
const PERSISTENT_THRESHOLD: u32 = PERSISTENT_BUMP - DAY_IN_LEDGERS;

// Soroban only embeds a `#[contracttype]`'s full definition in a contract's
// published interface when it's defined in the same crate as `#[contract]`
// — a shared types crate compiles fine but silently drops these from the
// spec, breaking CLI/SDK introspection. So these are duplicated verbatim in
// orbit-factory rather than shared; see that crate's conversion helpers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[contracttype]
// No explicit discriminants: a plain integer-tagged enum (`Fixed = 0, ...`)
// compiles to Soroban's `ScSpecUdtEnumV0`, which the `stellar` CLI cannot
// parse as an invoke argument (unimplemented in `soroban-spec-tools` as of
// stellar-cli 27). Omitting the discriminants makes this a tagged union
// (`ScSpecUdtUnionV0`) instead, which the CLI accepts as e.g. `--frequency
// '"Weekly"'`. Nothing in this contract relies on the underlying numeric
// value, so this is a pure encoding change.
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[contracttype]
pub enum OrbitStatus {
    Pending = 0,
    Active = 1,
    Completed = 2,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[contracttype]
pub enum MemberStatus {
    /// Seated by the admin but hasn't posted collateral yet (only reachable
    /// when `stake_bps > 0`); can't contribute or vote until `lock_stake`.
    Invited = 0,
    Active = 1,
    Defaulted = 2,
}

#[derive(Clone)]
#[contracttype]
pub struct MemberInfo {
    pub rotation_index: u32,
    pub status: MemberStatus,
    pub locked_stake: i128,
    pub has_received_payout: bool,
    pub completed_cycles: u32,
}

#[derive(Clone)]
#[contracttype]
pub struct Config {
    pub admin: Address,
    pub token: Address,
    pub name: String,
    pub contribution_amount: i128,
    pub frequency: Frequency,
    pub payout_order: PayoutOrder,
    pub stake_bps: u32,
    pub total_rounds: u32,
    pub grace_period_secs: u64,
}

#[derive(Clone)]
#[contracttype]
pub struct State {
    pub status: OrbitStatus,
    pub current_round: u32,
    pub live_pot_balance: i128,
    pub member_count: u32,
    pub round_start_time: u64,
    pub last_payout_recipient: Option<Address>,
    pub last_payout_amount: i128,
}

#[derive(Clone)]
#[contracttype]
pub struct Dispute {
    pub id: u32,
    pub round: u32,
    pub defaulter: Address,
    pub proposer: Address,
    pub yes_votes: u32,
    pub no_votes: u32,
    pub voted: Vec<Address>,
    pub resolved: bool,
    pub approved: bool,
    pub opened_at: u64,
}

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Config,
    State,
    Members,
    MemberInfo(Address),
    Contributed(u32, Address),
    RoundRecipient(u32),
    Bid(u32, Address),
    Dispute(u32),
    NextDisputeId,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum Error {
    AlreadyInitialized = 1,
    NotAdmin = 2,
    NotPending = 3,
    NotActive = 4,
    GroupFull = 5,
    RotationIndexTaken = 6,
    RotationIndexOutOfRange = 7,
    IncompleteRotation = 8,
    MemberNotFound = 9,
    MemberNotActive = 10,
    AlreadyContributed = 11,
    NotEligibleForPayout = 12,
    AlreadyVoted = 13,
    VoterIneligible = 14,
    DisputeNotFound = 15,
    DisputeAlreadyResolved = 16,
    NotYetOverdue = 17,
    AlreadyMember = 18,
    InvalidBid = 19,
    NotAuctionOrder = 20,
    BiddingClosed = 21,
    StakeNotDue = 22,
}

fn bump_instance(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_THRESHOLD, INSTANCE_BUMP);
}

fn bump_persistent(env: &Env, key: &DataKey) {
    env.storage()
        .persistent()
        .extend_ttl(key, PERSISTENT_THRESHOLD, PERSISTENT_BUMP);
}

fn read_config(env: &Env) -> Config {
    env.storage().instance().get(&DataKey::Config).unwrap()
}

fn read_state(env: &Env) -> State {
    env.storage().instance().get(&DataKey::State).unwrap()
}

fn read_members(env: &Env) -> Vec<Address> {
    env.storage().instance().get(&DataKey::Members).unwrap()
}

fn read_member_info(env: &Env, member: &Address) -> Result<MemberInfo, Error> {
    let key = DataKey::MemberInfo(member.clone());
    let info = env
        .storage()
        .persistent()
        .get(&key)
        .ok_or(Error::MemberNotFound)?;
    bump_persistent(env, &key);
    Ok(info)
}

fn write_member_info(env: &Env, member: &Address, info: &MemberInfo) {
    let key = DataKey::MemberInfo(member.clone());
    env.storage().persistent().set(&key, info);
    bump_persistent(env, &key);
}

fn vec_contains(v: &Vec<Address>, item: &Address) -> bool {
    for x in v.iter() {
        if x == *item {
            return true;
        }
    }
    false
}

#[contract]
pub struct OrbitContract;

#[contractimpl]
impl OrbitContract {
    /// Creates a new ROSCA group. Called once by the deployer (typically the
    /// orbit-factory contract) immediately after WASM instantiation.
    #[allow(clippy::too_many_arguments)]
    pub fn initialize(
        env: Env,
        admin: Address,
        token: Address,
        name: String,
        contribution_amount: i128,
        frequency: Frequency,
        payout_order: PayoutOrder,
        stake_bps: u32,
        total_rounds: u32,
        grace_period_secs: u64,
    ) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Config) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();

        let config = Config {
            admin,
            token,
            name,
            contribution_amount,
            frequency,
            payout_order,
            stake_bps,
            total_rounds,
            grace_period_secs,
        };
        let state = State {
            status: OrbitStatus::Pending,
            current_round: 1,
            live_pot_balance: 0,
            member_count: 0,
            round_start_time: env.ledger().timestamp(),
            last_payout_recipient: None,
            last_payout_amount: 0,
        };
        env.storage().instance().set(&DataKey::Config, &config);
        env.storage().instance().set(&DataKey::State, &state);
        env.storage()
            .instance()
            .set(&DataKey::Members, &Vec::<Address>::new(&env));
        env.storage().instance().set(&DataKey::NextDisputeId, &0u32);
        bump_instance(&env);
        Ok(())
    }

    /// Admin-only: seat a member at a fixed rotation slot while the group is
    /// still `Pending`. `rotation_index` only matters for `PayoutOrder::Fixed`
    /// but every member is assigned one as a stable join-order identity.
    pub fn add_member(env: Env, member: Address, rotation_index: u32) -> Result<(), Error> {
        bump_instance(&env);
        let config = read_config(&env);
        config.admin.require_auth();
        let mut state = read_state(&env);
        if state.status != OrbitStatus::Pending {
            return Err(Error::NotPending);
        }
        if rotation_index >= config.total_rounds {
            return Err(Error::RotationIndexOutOfRange);
        }
        let mut members = read_members(&env);
        if vec_contains(&members, &member) {
            return Err(Error::AlreadyMember);
        }
        if members.len() >= config.total_rounds {
            return Err(Error::GroupFull);
        }
        for m in members.iter() {
            let info = read_member_info(&env, &m)?;
            if info.rotation_index == rotation_index {
                return Err(Error::RotationIndexTaken);
            }
        }

        members.push_back(member.clone());
        env.storage().instance().set(&DataKey::Members, &members);
        // Collateral is due before the group can go live (see `lock_stake`);
        // a member with no stake requirement configured skips straight to
        // Active since there's nothing to post.
        let initial_status = if config.stake_bps > 0 {
            MemberStatus::Invited
        } else {
            MemberStatus::Active
        };
        write_member_info(
            &env,
            &member,
            &MemberInfo {
                rotation_index,
                status: initial_status,
                locked_stake: 0,
                has_received_payout: false,
                completed_cycles: 0,
            },
        );
        state.member_count += 1;
        env.storage().instance().set(&DataKey::State, &state);
        env.events()
            .publish((symbol_short!("joined"), member), rotation_index);
        Ok(())
    }

    /// Member-authorized: posts the collateral stake required to move from
    /// `Invited` to `Active`. Must happen while the group is still `Pending`
    /// — collecting stake upfront (rather than on a member's first
    /// contribution) ensures a member who defaults on round 1 without ever
    /// contributing still has something on the line to slash.
    pub fn lock_stake(env: Env, member: Address) -> Result<(), Error> {
        bump_instance(&env);
        member.require_auth();
        let config = read_config(&env);
        let state = read_state(&env);
        if state.status != OrbitStatus::Pending {
            return Err(Error::NotPending);
        }
        let mut info = read_member_info(&env, &member)?;
        if info.status != MemberStatus::Invited {
            return Err(Error::StakeNotDue);
        }
        let stake = (config.contribution_amount * config.stake_bps as i128) / 10_000;
        if stake > 0 {
            let token_client = token::Client::new(&env, &config.token);
            token_client.transfer(&member, &env.current_contract_address(), &stake);
        }
        info.locked_stake = stake;
        info.status = MemberStatus::Active;
        write_member_info(&env, &member, &info);
        env.events()
            .publish((symbol_short!("staked"), member), stake);
        Ok(())
    }

    /// Admin-only: flips the group live once every rotation slot is filled
    /// and every member has posted their stake.
    pub fn activate(env: Env) -> Result<(), Error> {
        bump_instance(&env);
        let config = read_config(&env);
        config.admin.require_auth();
        let mut state = read_state(&env);
        if state.status != OrbitStatus::Pending {
            return Err(Error::NotPending);
        }
        if state.member_count != config.total_rounds {
            return Err(Error::IncompleteRotation);
        }
        let members = read_members(&env);
        for m in members.iter() {
            let info = read_member_info(&env, &m)?;
            if info.status != MemberStatus::Active {
                return Err(Error::StakeNotDue);
            }
        }
        state.status = OrbitStatus::Active;
        state.round_start_time = env.ledger().timestamp();
        env.storage().instance().set(&DataKey::State, &state);
        env.events().publish((symbol_short!("active"),), ());
        Ok(())
    }

    /// Member contributes this round's quota. When every active member has
    /// contributed for the round, the round is auto-settled in the same
    /// transaction (no separate claim step), which avoids the
    /// payout-eligibility gaps this logic replaces from the frontend
    /// simulation.
    pub fn contribute(env: Env, member: Address) -> Result<(), Error> {
        bump_instance(&env);
        member.require_auth();
        let config = read_config(&env);
        let mut state = read_state(&env);
        if state.status != OrbitStatus::Active {
            return Err(Error::NotActive);
        }
        let info = read_member_info(&env, &member)?;
        if info.status != MemberStatus::Active {
            return Err(Error::MemberNotActive);
        }
        let round = state.current_round;
        let contributed_key = DataKey::Contributed(round, member.clone());
        if env
            .storage()
            .persistent()
            .get::<_, bool>(&contributed_key)
            .unwrap_or(false)
        {
            return Err(Error::AlreadyContributed);
        }

        let token_client = token::Client::new(&env, &config.token);
        token_client.transfer(&member, &env.current_contract_address(), &config.contribution_amount);

        state.live_pot_balance += config.contribution_amount;
        env.storage().persistent().set(&contributed_key, &true);
        bump_persistent(&env, &contributed_key);
        env.events()
            .publish((symbol_short!("contrib"), member), (round, config.contribution_amount));

        Self::maybe_settle_round(&env, &config, &mut state)?;
        env.storage().instance().set(&DataKey::State, &state);
        Ok(())
    }

    /// `PayoutOrder::Auction` only. A member signals the smallest payout
    /// they're willing to accept this round; lowest bidder wins the slot.
    /// Simplification: the winner still receives the *full* pot (matching
    /// Fixed/Random custody invariants) — the bid only decides priority, not
    /// a discounted payout with rebate redistribution. That richer auction
    /// economics is a separate feature, not implemented here.
    pub fn place_bid(env: Env, member: Address, bid_amount: i128) -> Result<(), Error> {
        bump_instance(&env);
        member.require_auth();
        let config = read_config(&env);
        if config.payout_order != PayoutOrder::Auction {
            return Err(Error::NotAuctionOrder);
        }
        let state = read_state(&env);
        if state.status != OrbitStatus::Active {
            return Err(Error::NotActive);
        }
        if bid_amount <= 0 {
            return Err(Error::InvalidBid);
        }
        let info = read_member_info(&env, &member)?;
        if info.has_received_payout {
            return Err(Error::BiddingClosed);
        }
        let key = DataKey::Bid(state.current_round, member.clone());
        env.storage().persistent().set(&key, &bid_amount);
        bump_persistent(&env, &key);
        env.events()
            .publish((symbol_short!("bid"), member), (state.current_round, bid_amount));
        Ok(())
    }

    /// Any active member (excluding the defaulter) may open a dispute once
    /// the round's grace period has elapsed without a contribution from
    /// `defaulter`.
    pub fn propose_dispute(env: Env, proposer: Address, defaulter: Address) -> Result<u32, Error> {
        bump_instance(&env);
        proposer.require_auth();
        let config = read_config(&env);
        let state = read_state(&env);
        if state.status != OrbitStatus::Active {
            return Err(Error::NotActive);
        }
        // `require_auth` only proves the caller controls `proposer`'s key —
        // it does not prove `proposer` is a member of this orbit. Without
        // this check any outsider address could open unlimited disputes
        // against a legitimately-overdue defaulter (spam, not fund-theft,
        // since vote_slash independently re-checks voter membership — but
        // still a real deviation from the documented "any active member"
        // invariant).
        let proposer_info = read_member_info(&env, &proposer)?;
        if proposer_info.status != MemberStatus::Active {
            return Err(Error::VoterIneligible);
        }
        let round = state.current_round;
        let now = env.ledger().timestamp();
        if now < state.round_start_time + config.grace_period_secs {
            return Err(Error::NotYetOverdue);
        }
        let already_contributed = env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Contributed(round, defaulter.clone()))
            .unwrap_or(false);
        if already_contributed {
            return Err(Error::NotYetOverdue);
        }
        let info = read_member_info(&env, &defaulter)?;
        if info.status != MemberStatus::Active {
            return Err(Error::MemberNotActive);
        }

        let mut next_id: u32 = env
            .storage()
            .instance()
            .get(&DataKey::NextDisputeId)
            .unwrap_or(0);
        let dispute = Dispute {
            id: next_id,
            round,
            defaulter: defaulter.clone(),
            proposer,
            yes_votes: 0,
            no_votes: 0,
            voted: Vec::new(&env),
            resolved: false,
            approved: false,
            opened_at: now,
        };
        let key = DataKey::Dispute(next_id);
        env.storage().persistent().set(&key, &dispute);
        bump_persistent(&env, &key);
        next_id += 1;
        env.storage().instance().set(&DataKey::NextDisputeId, &next_id);
        env.events()
            .publish((symbol_short!("dispute"), defaulter), dispute.id);
        Ok(dispute.id)
    }

    /// Active members (not the defaulter) vote once each. Once every
    /// eligible member has voted the dispute auto-resolves in this call.
    pub fn vote_slash(env: Env, voter: Address, dispute_id: u32, approve: bool) -> Result<(), Error> {
        bump_instance(&env);
        voter.require_auth();
        let dispute_key = DataKey::Dispute(dispute_id);
        let mut dispute: Dispute = env
            .storage()
            .persistent()
            .get(&dispute_key)
            .ok_or(Error::DisputeNotFound)?;
        if dispute.resolved {
            return Err(Error::DisputeAlreadyResolved);
        }
        if voter == dispute.defaulter {
            return Err(Error::VoterIneligible);
        }
        let voter_info = read_member_info(&env, &voter)?;
        if voter_info.status != MemberStatus::Active {
            return Err(Error::VoterIneligible);
        }
        if vec_contains(&dispute.voted, &voter) {
            return Err(Error::AlreadyVoted);
        }
        dispute.voted.push_back(voter.clone());
        if approve {
            dispute.yes_votes += 1;
        } else {
            dispute.no_votes += 1;
        }
        env.events()
            .publish((symbol_short!("voted"), voter), (dispute_id, approve));

        let members = read_members(&env);
        let mut eligible = 0u32;
        for m in members.iter() {
            if m == dispute.defaulter {
                continue;
            }
            let mi = read_member_info(&env, &m)?;
            if mi.status == MemberStatus::Active {
                eligible += 1;
            }
        }

        if dispute.voted.len() >= eligible {
            Self::resolve_dispute_internal(&env, &mut dispute)?;
        }
        env.storage().persistent().set(&dispute_key, &dispute);
        bump_persistent(&env, &dispute_key);
        Ok(())
    }

    /// Anyone may force a resolution once the grace period has re-elapsed
    /// since the dispute opened, so a dispute can't be stuck forever if a
    /// member refuses to vote.
    pub fn finalize_dispute(env: Env, dispute_id: u32) -> Result<(), Error> {
        bump_instance(&env);
        let config = read_config(&env);
        let dispute_key = DataKey::Dispute(dispute_id);
        let mut dispute: Dispute = env
            .storage()
            .persistent()
            .get(&dispute_key)
            .ok_or(Error::DisputeNotFound)?;
        if dispute.resolved {
            return Err(Error::DisputeAlreadyResolved);
        }
        let now = env.ledger().timestamp();
        if now < dispute.opened_at + config.grace_period_secs {
            return Err(Error::NotYetOverdue);
        }
        Self::resolve_dispute_internal(&env, &mut dispute)?;
        env.storage().persistent().set(&dispute_key, &dispute);
        bump_persistent(&env, &dispute_key);
        Ok(())
    }

    fn resolve_dispute_internal(env: &Env, dispute: &mut Dispute) -> Result<(), Error> {
        dispute.resolved = true;
        let config = read_config(env);
        let mut state = read_state(env);

        if dispute.yes_votes > dispute.no_votes {
            dispute.approved = true;
            let mut info = read_member_info(env, &dispute.defaulter)?;
            let slashed = info.locked_stake;
            info.locked_stake = 0;
            info.status = MemberStatus::Defaulted;
            write_member_info(env, &dispute.defaulter, &info);

            // The stake can only ever be a fraction (stake_bps) of a full
            // quota, so slashing may not fully cover the missed
            // contribution — the round's pot is simply smaller by the
            // shortfall rather than the contract attempting to cover funds
            // it never held.
            if slashed > 0 {
                state.live_pot_balance += slashed;
            }
            let contributed_key = DataKey::Contributed(dispute.round, dispute.defaulter.clone());
            env.storage().persistent().set(&contributed_key, &true);
            bump_persistent(env, &contributed_key);
            env.storage().instance().set(&DataKey::State, &state);

            Self::maybe_settle_round(env, &config, &mut state)?;
            env.storage().instance().set(&DataKey::State, &state);
            env.events().publish(
                (symbol_short!("slashed"), dispute.defaulter.clone()),
                (dispute.round, slashed),
            );
        } else {
            dispute.approved = false;
            state.round_start_time = env.ledger().timestamp();
            env.storage().instance().set(&DataKey::State, &state);
            env.events()
                .publish((symbol_short!("kept"), dispute.defaulter.clone()), dispute.round);
        }
        Ok(())
    }

    fn maybe_settle_round(env: &Env, config: &Config, state: &mut State) -> Result<(), Error> {
        let members = read_members(env);
        let round = state.current_round;
        for m in members.iter() {
            let info = read_member_info(env, &m)?;
            if info.status == MemberStatus::Active {
                let contributed = env
                    .storage()
                    .persistent()
                    .get::<_, bool>(&DataKey::Contributed(round, m.clone()))
                    .unwrap_or(false);
                if !contributed {
                    return Ok(());
                }
            }
        }

        let recipient = Self::resolve_recipient(env, config, state, &members)?;
        let payout = state.live_pot_balance;
        if payout > 0 {
            let token_client = token::Client::new(env, &config.token);
            token_client.transfer(&env.current_contract_address(), &recipient, &payout);
        }

        let mut r_info = read_member_info(env, &recipient)?;
        r_info.has_received_payout = true;
        r_info.completed_cycles += 1;
        write_member_info(env, &recipient, &r_info);

        env.events()
            .publish((symbol_short!("payout"), recipient.clone()), (round, payout));

        state.live_pot_balance = 0;
        state.last_payout_recipient = Some(recipient);
        state.last_payout_amount = payout;
        if round >= config.total_rounds {
            state.status = OrbitStatus::Completed;
        } else {
            state.current_round += 1;
            state.round_start_time = env.ledger().timestamp();
        }
        Ok(())
    }

    fn resolve_recipient(
        env: &Env,
        config: &Config,
        state: &State,
        members: &Vec<Address>,
    ) -> Result<Address, Error> {
        match config.payout_order {
            PayoutOrder::Fixed => {
                for m in members.iter() {
                    let info = read_member_info(env, &m)?;
                    if info.rotation_index == state.current_round - 1 {
                        return Ok(m);
                    }
                }
                Err(Error::NotEligibleForPayout)
            }
            PayoutOrder::Random => {
                let key = DataKey::RoundRecipient(state.current_round);
                if let Some(r) = env.storage().persistent().get::<_, Address>(&key) {
                    bump_persistent(env, &key);
                    return Ok(r);
                }
                let mut candidates: Vec<Address> = Vec::new(env);
                for m in members.iter() {
                    let info = read_member_info(env, &m)?;
                    if !info.has_received_payout && info.status == MemberStatus::Active {
                        candidates.push_back(m);
                    }
                }
                if candidates.is_empty() {
                    return Err(Error::NotEligibleForPayout);
                }
                let idx: u64 = env.prng().gen_range(0u64..candidates.len() as u64);
                let idx = idx as u32;
                let chosen = candidates.get(idx).unwrap();
                env.storage().persistent().set(&key, &chosen);
                bump_persistent(env, &key);
                Ok(chosen)
            }
            PayoutOrder::Auction => {
                let mut best: Option<(Address, i128)> = None;
                for m in members.iter() {
                    let info = read_member_info(env, &m)?;
                    if info.has_received_payout || info.status != MemberStatus::Active {
                        continue;
                    }
                    let bid_key = DataKey::Bid(state.current_round, m.clone());
                    if let Some(bid) = env.storage().persistent().get::<_, i128>(&bid_key) {
                        if best.as_ref().map_or(true, |(_, b)| bid < *b) {
                            best = Some((m.clone(), bid));
                        }
                    }
                }
                if let Some((winner, _)) = best {
                    return Ok(winner);
                }
                // No bids placed this round — fall back to join-order among
                // members still owed a payout so the cycle can't stall.
                for m in members.iter() {
                    let info = read_member_info(env, &m)?;
                    if !info.has_received_payout && info.status == MemberStatus::Active {
                        return Ok(m);
                    }
                }
                Err(Error::NotEligibleForPayout)
            }
        }
    }

    /// The group's immutable configuration set at `initialize` — token,
    /// contribution amount, frequency, payout order, stake %, round count.
    pub fn get_config(env: Env) -> Config {
        read_config(&env)
    }

    /// The group's current mutable state: status, round number, live pot
    /// balance, and the most recent payout.
    pub fn get_state(env: Env) -> State {
        read_state(&env)
    }

    /// All seated members, in join order (not payout order — see
    /// `MemberInfo.rotation_index` for `PayoutOrder::Fixed`'s order).
    pub fn get_members(env: Env) -> Vec<Address> {
        read_members(&env)
    }

    /// A member's stake, status, and payout history. `None` if `member` was
    /// never seated via `add_member`.
    pub fn get_member_info(env: Env, member: Address) -> Option<MemberInfo> {
        read_member_info(&env, &member).ok()
    }

    /// Whether `member` has already contributed for `round` (used by
    /// `maybe_settle_round` and to guard against double contribution).
    pub fn has_contributed(env: Env, round: u32, member: Address) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Contributed(round, member))
            .unwrap_or(false)
    }

    /// A dispute's full record — round, defaulter, vote tally, and
    /// resolution outcome once settled. `None` if `dispute_id` was never
    /// opened via `propose_dispute`.
    pub fn get_dispute(env: Env, dispute_id: u32) -> Option<Dispute> {
        env.storage().persistent().get(&DataKey::Dispute(dispute_id))
    }
}

mod test;
