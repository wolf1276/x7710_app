#![cfg(test)]
use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, String, Symbol, Vec, BytesN,
};

use delegation_manager::{DelegationManager, DelegationManagerClient, DelegationMetadata};
use policy_engine::{PolicyEngine, PolicyEngineClient, PolicyParams, ValidationResult};

fn set_ledger_time(env: &Env, timestamp: u64) {
    let mut info = env.ledger().get();
    info.timestamp = timestamp;
    env.ledger().set(info);
}

fn make_intent_hash(env: &Env, val: u8) -> BytesN<32> {
    let mut buf = [0u8; 32];
    buf[0] = val;
    BytesN::from_array(env, &buf)
}

struct TestFixture<'a> {
    env: Env,
    delegate: Address,
    asset: Address,
    protocol: Address,
    target: Address,
    er_client: ExecutionRouterClient<'a>,
    delegation_id: u64,
    policy_id: u64,
}

impl<'a> TestFixture<'a> {
    fn setup() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        set_ledger_time(&env, 1000);

        let admin = Address::generate(&env);
        let owner = Address::generate(&env);
        let delegate = Address::generate(&env);
        let asset = Address::generate(&env);
        let protocol = Address::generate(&env);
        let target = Address::generate(&env);

        let dm_id = env.register(DelegationManager, ());
        let dm_client = DelegationManagerClient::new(&env, &dm_id);

        let pe_id = env.register(PolicyEngine, ());
        let pe_client = PolicyEngineClient::new(&env, &pe_id);
        pe_client.initialize(&dm_id, &admin);

        let er_id = env.register(ExecutionRouter, ());
        let er_client = ExecutionRouterClient::new(&env, &er_id);
        er_client.initialize(&admin, &dm_id, &pe_id);

        pe_client.set_execution_router(&er_id);

        // Create delegation
        let tags = Vec::new(&env);
        let dm_meta = DelegationMetadata {
            label: String::from_str(&env, "Test Delegation"),
            strategy_id: Some(1),
            description: None,
            tags,
        };
        let delegation_id = dm_client.create_delegation(&owner, &delegate, &dm_meta, &Some(20000));
        dm_client.accept_delegation(&delegation_id);

        // Create policy
        let allowed_assets = Vec::new(&env);
        let denied_assets = Vec::new(&env);
        let allowed_protocols = Vec::new(&env);
        let denied_protocols = Vec::new(&env);
        let pe_params = PolicyParams {
            strategy_id: Some(1),
            valid_from: 1000,
            valid_until: Some(15000),
            max_notional_per_tx: 1000,
            max_notional_per_day: 5000,
            allowed_assets,
            denied_assets,
            allowed_protocols,
            denied_protocols,
            metadata: String::from_str(&env, "Test Policy"),
        };
        let policy_id = pe_client.create_policy(&owner, &delegate, &pe_params);
        pe_client.accept_policy(&policy_id);
        pe_client.assign_policy_to_delegation(&policy_id, &delegation_id);

        Self {
            env,
            delegate,
            asset,
            protocol,
            target,
            er_client,
            delegation_id,
            policy_id,
        }
    }
}

#[test]
fn test_successful_execution() {
    let f = TestFixture::setup();
    let intent_hash = make_intent_hash(&f.env, 1);

    let intent = ExecutionIntent {
        intent_hash: intent_hash.clone(),
        delegation_id: f.delegation_id,
        policy_id: f.policy_id,
        delegate: f.delegate.clone(),
        asset: f.asset.clone(),
        protocol: f.protocol.clone(),
        action_type: Symbol::new(&f.env, "swap"),
        amount: 500,
        target: f.target.clone(),
        payload_hash: make_intent_hash(&f.env, 99),
        nonce: 1,
        timestamp: 1000,
        expiry: 2000,
    };

    let exec_id = f.er_client.execute_intent(&intent);
    assert_eq!(exec_id, 1);

    // Verify record exists
    let record = f.er_client.get_execution(&exec_id).unwrap();
    assert_eq!(record.execution_id, 1);
    assert_eq!(record.intent_hash, intent_hash);
    assert_eq!(record.status, ExecutionStatus::Executed);
    assert_eq!(record.amount, 500);

    // Verify hash query works
    let record_by_hash = f.er_client.get_execution_by_hash(&intent_hash).unwrap();
    assert_eq!(record_by_hash.execution_id, 1);

    assert!(f.er_client.execution_exists(&exec_id));
    assert_eq!(f.er_client.get_execution_status(&exec_id), Some(ExecutionStatus::Executed));
}

#[test]
#[should_panic(expected = "intent hash already seen")]
fn test_replay_prevention() {
    let f = TestFixture::setup();
    let intent_hash = make_intent_hash(&f.env, 1);

    let intent = ExecutionIntent {
        intent_hash: intent_hash.clone(),
        delegation_id: f.delegation_id,
        policy_id: f.policy_id,
        delegate: f.delegate.clone(),
        asset: f.asset.clone(),
        protocol: f.protocol.clone(),
        action_type: Symbol::new(&f.env, "swap"),
        amount: 500,
        target: f.target.clone(),
        payload_hash: make_intent_hash(&f.env, 99),
        nonce: 1,
        timestamp: 1000,
        expiry: 2000,
    };

    // Execute first time
    f.er_client.execute_intent(&intent);

    // Second time must panic (replay attempt rejected)
    f.er_client.execute_intent(&intent);
}

#[test]
fn test_invalid_authority() {
    let f = TestFixture::setup();
    let intent_hash = make_intent_hash(&f.env, 1);

    let wrong_delegate = Address::generate(&f.env);
    let intent = ExecutionIntent {
        intent_hash: intent_hash.clone(),
        delegation_id: f.delegation_id,
        policy_id: f.policy_id,
        delegate: wrong_delegate,
        asset: f.asset.clone(),
        protocol: f.protocol.clone(),
        action_type: Symbol::new(&f.env, "swap"),
        amount: 500,
        target: f.target.clone(),
        payload_hash: make_intent_hash(&f.env, 99),
        nonce: 1,
        timestamp: 1000,
        expiry: 2000,
    };

    let exec_id = f.er_client.execute_intent(&intent);
    let record = f.er_client.get_execution(&exec_id).unwrap();
    assert_eq!(record.status, ExecutionStatus::Rejected);
    assert!(record.rejection_reason.is_some());
}

#[test]
fn test_invalid_policy() {
    let f = TestFixture::setup();
    let intent_hash = make_intent_hash(&f.env, 1);

    let intent = ExecutionIntent {
        intent_hash: intent_hash.clone(),
        delegation_id: f.delegation_id,
        policy_id: f.policy_id + 1, // Invalid policy ID
        delegate: f.delegate.clone(),
        asset: f.asset.clone(),
        protocol: f.protocol.clone(),
        action_type: Symbol::new(&f.env, "swap"),
        amount: 500,
        target: f.target.clone(),
        payload_hash: make_intent_hash(&f.env, 99),
        nonce: 1,
        timestamp: 1000,
        expiry: 2000,
    };

    let exec_id = f.er_client.execute_intent(&intent);
    let record = f.er_client.get_execution(&exec_id).unwrap();
    assert_eq!(record.status, ExecutionStatus::Rejected);
    assert_eq!(record.rejection_reason.unwrap(), String::from_str(&f.env, "policy mismatch"));
}

#[test]
fn test_expired_delegation() {
    let f = TestFixture::setup();
    let intent_hash = make_intent_hash(&f.env, 1);

    let intent = ExecutionIntent {
        intent_hash: intent_hash.clone(),
        delegation_id: f.delegation_id,
        policy_id: f.policy_id,
        delegate: f.delegate.clone(),
        asset: f.asset.clone(),
        protocol: f.protocol.clone(),
        action_type: Symbol::new(&f.env, "swap"),
        amount: 500,
        target: f.target.clone(),
        payload_hash: make_intent_hash(&f.env, 99),
        nonce: 1,
        timestamp: 1000,
        expiry: 30000,
    };

    // Advance time beyond delegation expiry (20000)
    set_ledger_time(&f.env, 21000);

    let exec_id = f.er_client.execute_intent(&intent);
    let record = f.er_client.get_execution(&exec_id).unwrap();
    assert_eq!(record.status, ExecutionStatus::Rejected);
    assert_eq!(record.rejection_reason.unwrap(), String::from_str(&f.env, "delegation not active"));
}

#[test]
fn test_expired_policy() {
    let f = TestFixture::setup();
    let intent_hash = make_intent_hash(&f.env, 1);

    let intent = ExecutionIntent {
        intent_hash: intent_hash.clone(),
        delegation_id: f.delegation_id,
        policy_id: f.policy_id,
        delegate: f.delegate.clone(),
        asset: f.asset.clone(),
        protocol: f.protocol.clone(),
        action_type: Symbol::new(&f.env, "swap"),
        amount: 500,
        target: f.target.clone(),
        payload_hash: make_intent_hash(&f.env, 99),
        nonce: 1,
        timestamp: 1000,
        expiry: 30000,
    };

    // Advance time beyond policy expiry (15000) but before delegation expiry (20000)
    set_ledger_time(&f.env, 16000);

    let exec_id = f.er_client.execute_intent(&intent);
    let record = f.er_client.get_execution(&exec_id).unwrap();
    assert_eq!(record.status, ExecutionStatus::Rejected);
    assert_eq!(record.rejection_reason.unwrap(), String::from_str(&f.env, "policy expired"));
}

#[test]
fn test_expired_intent() {
    let f = TestFixture::setup();
    let intent_hash = make_intent_hash(&f.env, 1);

    let intent = ExecutionIntent {
        intent_hash: intent_hash.clone(),
        delegation_id: f.delegation_id,
        policy_id: f.policy_id,
        delegate: f.delegate.clone(),
        asset: f.asset.clone(),
        protocol: f.protocol.clone(),
        action_type: Symbol::new(&f.env, "swap"),
        amount: 500,
        target: f.target.clone(),
        payload_hash: make_intent_hash(&f.env, 99),
        nonce: 1,
        timestamp: 1000,
        expiry: 2000,
    };

    // Advance time beyond intent expiry (2000)
    set_ledger_time(&f.env, 2500);

    let exec_id = f.er_client.execute_intent(&intent);
    let record = f.er_client.get_execution(&exec_id).unwrap();
    assert_eq!(record.status, ExecutionStatus::Rejected);
    assert_eq!(record.rejection_reason.unwrap(), String::from_str(&f.env, "intent expired"));
}

#[test]
fn test_simulation_path() {
    let f = TestFixture::setup();
    let intent_hash = make_intent_hash(&f.env, 1);

    let intent = ExecutionIntent {
        intent_hash: intent_hash.clone(),
        delegation_id: f.delegation_id,
        policy_id: f.policy_id,
        delegate: f.delegate.clone(),
        asset: f.asset.clone(),
        protocol: f.protocol.clone(),
        action_type: Symbol::new(&f.env, "swap"),
        amount: 500,
        target: f.target.clone(),
        payload_hash: make_intent_hash(&f.env, 99),
        nonce: 1,
        timestamp: 1000,
        expiry: 2000,
    };

    // Simulation of valid intent
    let result = f.er_client.simulate_intent(&intent);
    assert!(matches!(result, ValidationResult::Approved));

    // Verify no state changes (execution id counter is still 0, execution doesn't exist)
    assert!(!f.er_client.execution_exists(&1));

    // Simulation of invalid intent (e.g. amount exceeds limit)
    let mut bad_intent = intent.clone();
    bad_intent.amount = 10000;
    let result_bad = f.er_client.simulate_intent(&bad_intent);
    assert!(matches!(result_bad, ValidationResult::Rejected(_)));
}

#[test]
#[should_panic]
fn test_unauthorized_router_dependencies_update() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let dm_id = Address::generate(&env);
    let pe_id = Address::generate(&env);

    let er_id = env.register(ExecutionRouter, ());
    let er_client = ExecutionRouterClient::new(&env, &er_id);
    er_client.initialize(&admin, &dm_id, &pe_id);

    // Non-admin attempt to update DM (since we don't mock auth and don't call as admin, this should fail)
    let new_dm = Address::generate(&env);
    er_client.set_delegation_manager(&new_dm);
}
