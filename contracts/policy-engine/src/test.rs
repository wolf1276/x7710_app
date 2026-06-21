#![cfg(test)]
use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, String, Vec,
};

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub enum MockKey {
    Exists(u64),
    Active(u64),
    Owner(u64),
}

#[contract]
pub struct MockDelegationManager;

#[contractimpl]
impl MockDelegationManager {
    pub fn delegation_exists(env: Env, delegation_id: u64) -> bool {
        env.storage().persistent().get::<_, bool>(&MockKey::Exists(delegation_id)).unwrap_or(false)
    }

    pub fn is_active_delegation(env: Env, delegation_id: u64) -> bool {
        env.storage().persistent().get::<_, bool>(&MockKey::Active(delegation_id)).unwrap_or(false)
    }

    pub fn get_owner(env: Env, delegation_id: u64) -> Option<Address> {
        env.storage().persistent().get::<_, Address>(&MockKey::Owner(delegation_id))
    }

    pub fn set_delegation(env: Env, id: u64, exists: bool, active: bool, owner: Address) {
        env.storage().persistent().set(&MockKey::Exists(id), &exists);
        env.storage().persistent().set(&MockKey::Active(id), &active);
        env.storage().persistent().set(&MockKey::Owner(id), &owner);
    }
}

fn set_ledger_time(env: &Env, timestamp: u64) {
    let mut info = env.ledger().get();
    info.timestamp = timestamp;
    env.ledger().set(info);
}

#[test]
fn test_create_and_update_policy() {
    let env = Env::default();
    set_ledger_time(&env, 1000);
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let owner = Address::generate(&env);
    let delegate = Address::generate(&env);
    let dm_address = env.register(MockDelegationManager, ());

    let contract_id = env.register(PolicyEngine, ());
    let client = PolicyEngineClient::new(&env, &contract_id);
    client.initialize(&dm_address, &admin);

    assert_eq!(client.get_admin(), admin);
    assert_eq!(client.get_delegation_manager(), dm_address);

    let allowed_assets = Vec::new(&env);
    let denied_assets = Vec::new(&env);
    let allowed_protocols = Vec::new(&env);
    let denied_protocols = Vec::new(&env);
    let metadata = String::from_str(&env, "Test Policy V1");

    let params = PolicyParams {
        strategy_id: Some(101),
        valid_from: 1000,
        valid_until: Some(5000),
        max_notional_per_tx: 500,
        max_notional_per_day: 2000,
        allowed_assets: allowed_assets.clone(),
        denied_assets: denied_assets.clone(),
        allowed_protocols: allowed_protocols.clone(),
        denied_protocols: denied_protocols.clone(),
        metadata: metadata.clone(),
    };

    let policy_id = client.create_policy(&owner, &delegate, &params);
    assert_eq!(policy_id, 1);

    let policy = client.get_policy(&policy_id).unwrap();
    assert_eq!(policy.id, 1);
    assert_eq!(policy.status, PolicyStatus::Pending);
    assert_eq!(policy.owner, owner);
    assert_eq!(policy.delegate, delegate);
    assert_eq!(policy.params.strategy_id, Some(101));
    assert_eq!(policy.params.max_notional_per_tx, 500);

    // Update the policy
    let updated_params = PolicyParams {
        strategy_id: Some(101),
        valid_from: 1200,
        valid_until: Some(6000),
        max_notional_per_tx: 1000,
        max_notional_per_day: 4000,
        allowed_assets: allowed_assets.clone(),
        denied_assets: denied_assets.clone(),
        allowed_protocols: allowed_protocols.clone(),
        denied_protocols: denied_protocols.clone(),
        metadata: String::from_str(&env, "Test Policy V2"),
    };

    client.update_policy(&policy_id, &updated_params);

    let policy = client.get_policy(&policy_id).unwrap();
    assert_eq!(policy.params.valid_from, 1200);
    assert_eq!(policy.params.valid_until, Some(6000));
    assert_eq!(policy.params.max_notional_per_tx, 1000);
    assert_eq!(policy.params.max_notional_per_day, 4000);
    assert_eq!(policy.params.metadata, String::from_str(&env, "Test Policy V2"));
    assert_eq!(policy.version, 2);
}

#[test]
fn test_lifecycle_and_assignment() {
    let env = Env::default();
    set_ledger_time(&env, 1000);
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let owner = Address::generate(&env);
    let delegate = Address::generate(&env);

    let dm_address = env.register(MockDelegationManager, ());
    let dm_client = MockDelegationManagerClient::new(&env, &dm_address);
    // Delegation ID 1 owned by owner, exists and active
    dm_client.set_delegation(&1, &true, &true, &owner);

    let contract_id = env.register(PolicyEngine, ());
    let client = PolicyEngineClient::new(&env, &contract_id);
    client.initialize(&dm_address, &admin);

    let params = PolicyParams {
        strategy_id: None,
        valid_from: 1000,
        valid_until: None,
        max_notional_per_tx: 100,
        max_notional_per_day: 500,
        allowed_assets: Vec::new(&env),
        denied_assets: Vec::new(&env),
        allowed_protocols: Vec::new(&env),
        denied_protocols: Vec::new(&env),
        metadata: String::from_str(&env, "Lifecycle Policy"),
    };

    let policy_id = client.create_policy(&owner, &delegate, &params);

    // Initial state is Pending
    let policy = client.get_policy(&policy_id).unwrap();
    assert_eq!(policy.status, PolicyStatus::Pending);

    // Accept policy
    client.accept_policy(&policy_id);
    let policy = client.get_policy(&policy_id).unwrap();
    assert_eq!(policy.status, PolicyStatus::Active);

    // Assign to delegation
    client.assign_policy_to_delegation(&policy_id, &1);

    let assigned_policies = client.get_policies_by_delegation(&1);
    assert_eq!(assigned_policies.len(), 1);
    assert_eq!(assigned_policies.get(0).unwrap().id, policy_id);

    let active_policy = client.get_active_policy(&1).unwrap();
    assert_eq!(active_policy.id, policy_id);

    // Pause policy
    client.pause_policy(&policy_id);
    let policy = client.get_policy(&policy_id).unwrap();
    assert_eq!(policy.status, PolicyStatus::Paused);

    // Resume policy
    client.resume_policy(&policy_id);
    let policy = client.get_policy(&policy_id).unwrap();
    assert_eq!(policy.status, PolicyStatus::Active);

    // Revoke policy
    client.revoke_policy(&policy_id);
    let policy = client.get_policy(&policy_id).unwrap();
    assert_eq!(policy.status, PolicyStatus::Revoked);

    // Revoked policy should be removed from active policy mapping
    let active_policy_after_revocation = client.get_active_policy(&1);
    assert!(active_policy_after_revocation.is_none());
}

#[test]
fn test_validation_paths() {
    let env = Env::default();
    set_ledger_time(&env, 1000);
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let owner = Address::generate(&env);
    let delegate = Address::generate(&env);
    let asset_a = Address::generate(&env);
    let asset_b = Address::generate(&env);
    let protocol_x = Address::generate(&env);
    let protocol_y = Address::generate(&env);
    let router = Address::generate(&env);

    let dm_address = env.register(MockDelegationManager, ());
    let dm_client = MockDelegationManagerClient::new(&env, &dm_address);

    // Delegation ID 1: Active
    dm_client.set_delegation(&1, &true, &true, &owner);
    // Delegation ID 2: Inactive/Suspended
    dm_client.set_delegation(&2, &true, &false, &owner);

    let contract_id = env.register(PolicyEngine, ());
    let client = PolicyEngineClient::new(&env, &contract_id);
    client.initialize(&dm_address, &admin);

    // Set Execution Router
    client.set_execution_router(&router);
    assert_eq!(client.get_execution_router(), Some(router.clone()));

    // 1. Create a policy with allowed/denied parameters
    let mut allowed_assets = Vec::new(&env);
    allowed_assets.push_back(asset_a.clone());

    let mut denied_assets = Vec::new(&env);
    denied_assets.push_back(asset_b.clone());

    let mut allowed_protocols = Vec::new(&env);
    allowed_protocols.push_back(protocol_x.clone());

    let mut denied_protocols = Vec::new(&env);
    denied_protocols.push_back(protocol_y.clone());

    let params = PolicyParams {
        strategy_id: None,
        valid_from: 1000,
        valid_until: Some(2000),
        max_notional_per_tx: 250,
        max_notional_per_day: 300,
        allowed_assets,
        denied_assets,
        allowed_protocols,
        denied_protocols,
        metadata: String::from_str(&env, "Val Policy"),
    };

    let policy_id = client.create_policy(&owner, &delegate, &params);
    client.accept_policy(&policy_id);
    client.assign_policy_to_delegation(&policy_id, &1);

    // Check remaining daily limit (should be max_notional_per_day)
    assert_eq!(client.get_remaining_daily_limit(&1, &1500), 300);

    // Validation 1: Successful Validation
    let res = client.validate_action(&1, &asset_a, &protocol_x, &50, &1500);
    assert_eq!(res, ValidationResult::Approved);

    // Record action and verify daily limits (requires Execution Router authorized call)
    client.record_action(&1, &50, &1500);
    assert_eq!(client.get_remaining_daily_limit(&1, &1500), 250);

    let res = client.validate_action(&1, &asset_a, &protocol_x, &200, &1500);
    assert_eq!(res, ValidationResult::Approved); // 50 + 200 = 250 <= 300 daily limit

    client.record_action(&1, &200, &1500);
    assert_eq!(client.get_remaining_daily_limit(&1, &1500), 50);

    // Now daily total is 250. Trying another 60 should exceed 300 daily limit.
    let res = client.validate_action(&1, &asset_a, &protocol_x, &60, &1500);
    assert_eq!(
        res,
        ValidationResult::Rejected(String::from_str(&env, "amount exceeds daily limit"))
    );

    // Validation 2: Delegation checks
    let res = client.validate_action(&2, &asset_a, &protocol_x, &50, &1500);
    assert_eq!(
        res,
        ValidationResult::Rejected(String::from_str(&env, "delegation not active"))
    );

    let res = client.validate_action(&99, &asset_a, &protocol_x, &50, &1500);
    assert_eq!(
        res,
        ValidationResult::Rejected(String::from_str(&env, "delegation does not exist"))
    );

    // Validation 3: Expiry checks
    let res = client.validate_action(&1, &asset_a, &protocol_x, &50, &900);
    assert_eq!(
        res,
        ValidationResult::Rejected(String::from_str(&env, "policy not yet valid"))
    );

    let res = client.validate_action(&1, &asset_a, &protocol_x, &50, &2100);
    assert_eq!(
        res,
        ValidationResult::Rejected(String::from_str(&env, "policy expired"))
    );

    // Validation 4: Allowed / Denied Asset checks
    let other_asset = Address::generate(&env);
    let res = client.validate_action(&1, &other_asset, &protocol_x, &50, &1500);
    assert_eq!(
        res,
        ValidationResult::Rejected(String::from_str(&env, "asset not allowed"))
    );

    let res = client.validate_action(&1, &asset_b, &protocol_x, &50, &1500);
    assert_eq!(
        res,
        ValidationResult::Rejected(String::from_str(&env, "asset denied"))
    );

    // Validation 5: Allowed / Denied Protocol checks
    let other_protocol = Address::generate(&env);
    let res = client.validate_action(&1, &asset_a, &other_protocol, &50, &1500);
    assert_eq!(
        res,
        ValidationResult::Rejected(String::from_str(&env, "protocol not allowed"))
    );

    let res = client.validate_action(&1, &asset_a, &protocol_y, &50, &1500);
    assert_eq!(
        res,
        ValidationResult::Rejected(String::from_str(&env, "protocol denied"))
    );

    // Validation 6: Tx Limit
    let res = client.validate_action(&1, &asset_a, &protocol_x, &300, &1500);
    assert_eq!(
        res,
        ValidationResult::Rejected(String::from_str(&env, "amount exceeds max_notional_per_tx"))
    );
}

#[test]
#[should_panic(expected = "Execution Router not set")]
fn test_record_action_fails_without_router() {
    let env = Env::default();
    set_ledger_time(&env, 1000);
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let dm_address = env.register(MockDelegationManager, ());
    let contract_id = env.register(PolicyEngine, ());
    let client = PolicyEngineClient::new(&env, &contract_id);
    client.initialize(&dm_address, &admin);

    client.record_action(&1, &100, &1000);
}

#[test]
#[should_panic(expected = "cannot assign policy to an inactive delegation")]
fn test_assign_policy_inactive_delegation_fails() {
    let env = Env::default();
    set_ledger_time(&env, 1000);
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let dm_address = env.register(MockDelegationManager, ());
    let dm_client = MockDelegationManagerClient::new(&env, &dm_address);
    // Mock delegation exists but NOT active
    let owner = Address::generate(&env);
    dm_client.set_delegation(&1, &true, &false, &owner);

    let contract_id = env.register(PolicyEngine, ());
    let client = PolicyEngineClient::new(&env, &contract_id);
    client.initialize(&dm_address, &admin);

    let allowed_assets = Vec::new(&env);
    let denied_assets = Vec::new(&env);
    let allowed_protocols = Vec::new(&env);
    let denied_protocols = Vec::new(&env);
    let params = PolicyParams {
        strategy_id: Some(1),
        valid_from: 1000,
        valid_until: Some(5000),
        max_notional_per_tx: 500,
        max_notional_per_day: 2000,
        allowed_assets,
        denied_assets,
        allowed_protocols,
        denied_protocols,
        metadata: String::from_str(&env, "Test Policy"),
    };
    let delegate = Address::generate(&env);
    let policy_id = client.create_policy(&owner, &delegate, &params);
    client.accept_policy(&policy_id);

    client.assign_policy_to_delegation(&policy_id, &1);
}

#[test]
#[should_panic(expected = "delegation already has an active policy")]
fn test_prevent_multiple_active_policies() {
    let env = Env::default();
    set_ledger_time(&env, 1000);
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let owner = Address::generate(&env);
    let delegate = Address::generate(&env);
    let dm_address = env.register(MockDelegationManager, ());
    let dm_client = MockDelegationManagerClient::new(&env, &dm_address);
    dm_client.set_delegation(&1, &true, &true, &owner);

    let contract_id = env.register(PolicyEngine, ());
    let client = PolicyEngineClient::new(&env, &contract_id);
    client.initialize(&dm_address, &admin);

    let params = PolicyParams {
        strategy_id: Some(1),
        valid_from: 1000,
        valid_until: Some(5000),
        max_notional_per_tx: 500,
        max_notional_per_day: 2000,
        allowed_assets: Vec::new(&env),
        denied_assets: Vec::new(&env),
        allowed_protocols: Vec::new(&env),
        denied_protocols: Vec::new(&env),
        metadata: String::from_str(&env, "Test Policy"),
    };

    let p1 = client.create_policy(&owner, &delegate, &params);
    client.accept_policy(&p1);
    client.assign_policy_to_delegation(&p1, &1);

    let p2 = client.create_policy(&owner, &delegate, &params);
    client.accept_policy(&p2);
    // This assignment should panic because delegation ID 1 already has an active policy p1
    client.assign_policy_to_delegation(&p2, &1);
}

#[test]
fn test_lazy_policy_expiry() {
    let env = Env::default();
    set_ledger_time(&env, 1000);
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let owner = Address::generate(&env);
    let delegate = Address::generate(&env);
    let dm_address = env.register(MockDelegationManager, ());
    let dm_client = MockDelegationManagerClient::new(&env, &dm_address);
    dm_client.set_delegation(&1, &true, &true, &owner);

    let contract_id = env.register(PolicyEngine, ());
    let client = PolicyEngineClient::new(&env, &contract_id);
    client.initialize(&dm_address, &admin);

    let params = PolicyParams {
        strategy_id: Some(1),
        valid_from: 1000,
        valid_until: Some(2000), // Expiry at 2000
        max_notional_per_tx: 500,
        max_notional_per_day: 2000,
        allowed_assets: Vec::new(&env),
        denied_assets: Vec::new(&env),
        allowed_protocols: Vec::new(&env),
        denied_protocols: Vec::new(&env),
        metadata: String::from_str(&env, "Test Policy"),
    };

    let p1 = client.create_policy(&owner, &delegate, &params);
    client.accept_policy(&p1);
    client.assign_policy_to_delegation(&p1, &1);

    assert!(client.get_active_policy(&1).is_some());

    // Advance time beyond valid_until (2000)
    set_ledger_time(&env, 3000);

    // get_active_policy should return None and transition the policy status to Expired lazily
    assert!(client.get_active_policy(&1).is_none());

    let policy = client.get_policy(&p1).unwrap();
    assert_eq!(policy.status, PolicyStatus::Expired);
}

