#![cfg(test)]
use super::*;
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger},
    Address, Env, String, Vec,
};

fn set_ledger_time(env: &Env, timestamp: u64) {
    let mut info = env.ledger().get();
    info.timestamp = timestamp;
    env.ledger().set(info);
}

#[test]
fn test_lifecycle_and_validation() {
    let env = Env::default();
    set_ledger_time(&env, 1000);

    let contract_id = env.register(DelegationManager, ());
    let client = DelegationManagerClient::new(&env, &contract_id);

    let owner = Address::generate(&env);
    let delegate = Address::generate(&env);

    let tags = Vec::new(&env);
    let metadata = DelegationMetadata {
        label: String::from_str(&env, "Agent Executor"),
        strategy_id: Some(42),
        description: Some(String::from_str(&env, "Test Desc")),
        tags,
    };

    env.mock_all_auths();

    // 1. Create Delegation with 1-hour expiry (timestamp 1000 + 3600 = 4600)
    let expires_at = Some(4600);
    let id = client.create_delegation(&owner, &delegate, &metadata, &expires_at);
    assert_eq!(id, 1);

    // Verify Pending State
    let delegation = client.get_delegation(&id).unwrap();
    assert_eq!(delegation.status, DelegationStatus::Pending);
    assert_eq!(delegation.version, 1);
    assert_eq!(delegation.created_at, 1000);
    assert!(!client.is_active_delegation(&id)); // Pending is not active yet

    // 2. Accept Delegation at timestamp 1500
    set_ledger_time(&env, 1500);
    client.accept_delegation(&id);

    let delegation = client.get_delegation(&id).unwrap();
    assert_eq!(delegation.status, DelegationStatus::Active);
    assert_eq!(delegation.version, 2);
    assert_eq!(delegation.updated_at, 1500);

    // Verify Active Validation APIs
    assert!(client.delegation_exists(&id));
    assert!(client.is_active_delegation(&id));
    assert!(!client.is_paused(&id));
    assert!(!client.is_revoked(&id));
    assert!(!client.is_expired(&id));
    assert_eq!(client.get_owner(&id).unwrap(), owner);
    assert_eq!(client.get_delegate(&id).unwrap(), delegate);
    assert_eq!(
        client.get_delegate_status(&id).unwrap(),
        DelegationStatus::Active
    );

    // 3. Pause Delegation at timestamp 2000
    set_ledger_time(&env, 2000);
    client.pause_delegation(&id);

    let delegation = client.get_delegation(&id).unwrap();
    assert_eq!(delegation.status, DelegationStatus::Paused);
    assert_eq!(delegation.version, 3);
    assert_eq!(delegation.paused_at, Some(2000));
    assert_eq!(delegation.updated_at, 2000);

    assert!(!client.is_active_delegation(&id));
    assert!(client.is_paused(&id));

    // 4. Resume Delegation at timestamp 3000
    set_ledger_time(&env, 3000);
    client.resume_delegation(&id);

    let delegation = client.get_delegation(&id).unwrap();
    assert_eq!(delegation.status, DelegationStatus::Active);
    assert_eq!(delegation.version, 4);
    assert_eq!(delegation.resumed_at, Some(3000));
    assert_eq!(delegation.updated_at, 3000);

    // 5. Update Metadata
    let updated_tags = Vec::new(&env);
    let updated_metadata = DelegationMetadata {
        label: String::from_str(&env, "New Executor Label"),
        strategy_id: Some(99),
        description: None,
        tags: updated_tags,
    };
    client.update_metadata(&id, &updated_metadata);

    let delegation = client.get_delegation(&id).unwrap();
    assert_eq!(
        delegation.metadata.label,
        String::from_str(&env, "New Executor Label")
    );
    assert_eq!(delegation.version, 5);

    // 6. Expiry test
    set_ledger_time(&env, 5000); // 5000 is > 4600 expiry time
    assert!(client.is_expired(&id));
    assert!(!client.is_active_delegation(&id));

    // Run check_and_expire to update state explicitly to Expired
    assert!(client.check_and_expire(&id));
    let delegation = client.get_delegation(&id).unwrap();
    assert_eq!(delegation.status, DelegationStatus::Expired);
    assert_eq!(delegation.version, 6);

    // Revoke delegation after expiry (allowed)
    client.revoke_delegation(&id);
    let delegation = client.get_delegation(&id).unwrap();
    assert_eq!(delegation.status, DelegationStatus::Revoked);
    assert_eq!(delegation.version, 7);
}

#[test]
#[should_panic(expected = "expiry must be in the future")]
fn test_invalid_expiry_creation() {
    let env = Env::default();
    set_ledger_time(&env, 1000);

    let contract_id = env.register(DelegationManager, ());
    let client = DelegationManagerClient::new(&env, &contract_id);

    let owner = Address::generate(&env);
    let delegate = Address::generate(&env);

    let tags = Vec::new(&env);
    let metadata = DelegationMetadata {
        label: String::from_str(&env, "Agent Executor"),
        strategy_id: None,
        description: None,
        tags,
    };

    env.mock_all_auths();
    client.create_delegation(&owner, &delegate, &metadata, &Some(900));
}

#[test]
#[should_panic(expected = "duplicate active/paused/pending delegation not allowed")]
fn test_duplicate_prevention() {
    let env = Env::default();
    set_ledger_time(&env, 1000);

    let contract_id = env.register(DelegationManager, ());
    let client = DelegationManagerClient::new(&env, &contract_id);

    let owner = Address::generate(&env);
    let delegate = Address::generate(&env);

    let tags = Vec::new(&env);
    let metadata = DelegationMetadata {
        label: String::from_str(&env, "Agent Executor"),
        strategy_id: None,
        description: None,
        tags,
    };

    env.mock_all_auths();
    client.create_delegation(&owner, &delegate, &metadata, &None);
    // Should panic due to duplicate pending delegation
    client.create_delegation(&owner, &delegate, &metadata, &None);
}

#[test]
fn test_allow_recreation_after_revoked_or_expired() {
    let env = Env::default();
    set_ledger_time(&env, 1000);

    let contract_id = env.register(DelegationManager, ());
    let client = DelegationManagerClient::new(&env, &contract_id);

    let owner = Address::generate(&env);
    let delegate = Address::generate(&env);

    let tags = Vec::new(&env);
    let metadata = DelegationMetadata {
        label: String::from_str(&env, "Agent Executor"),
        strategy_id: None,
        description: None,
        tags,
    };

    env.mock_all_auths();
    let id_1 = client.create_delegation(&owner, &delegate, &metadata, &Some(1500));

    // 1. Recreate after expiry
    set_ledger_time(&env, 2000); // Wait until expired
    assert!(client.check_and_expire(&id_1));

    let id_2 = client.create_delegation(&owner, &delegate, &metadata, &None);
    assert_eq!(id_2, 2);

    // Accept and then revoke active one
    client.accept_delegation(&id_2);
    client.revoke_delegation(&id_2);

    // 3. Recreate after revoked
    let id_3 = client.create_delegation(&owner, &delegate, &metadata, &None);
    assert_eq!(id_3, 3);
}

#[test]
fn test_renounce_flow() {
    let env = Env::default();
    set_ledger_time(&env, 1000);

    let contract_id = env.register(DelegationManager, ());
    let client = DelegationManagerClient::new(&env, &contract_id);

    let owner = Address::generate(&env);
    let delegate = Address::generate(&env);

    let tags = Vec::new(&env);
    let metadata = DelegationMetadata {
        label: String::from_str(&env, "Agent Executor"),
        strategy_id: None,
        description: None,
        tags,
    };

    env.mock_all_auths();
    let id = client.create_delegation(&owner, &delegate, &metadata, &None);

    // Delegate accepts the delegation
    client.accept_delegation(&id);
    assert_eq!(client.get_delegate_status(&id).unwrap(), DelegationStatus::Active);

    // Delegate renounces the delegation
    client.renounce_delegation(&id);
    assert_eq!(client.get_delegate_status(&id).unwrap(), DelegationStatus::Revoked);
    assert!(!client.is_active_delegation(&id));
}

#[test]
fn test_events() {
    let env = Env::default();
    set_ledger_time(&env, 1000);

    let contract_id = env.register(DelegationManager, ());
    let client = DelegationManagerClient::new(&env, &contract_id);

    let owner = Address::generate(&env);
    let delegate = Address::generate(&env);

    let tags = Vec::new(&env);
    let metadata = DelegationMetadata {
        label: String::from_str(&env, "Agent"),
        strategy_id: None,
        description: None,
        tags,
    };

    env.mock_all_auths();
    let _id = client.create_delegation(&owner, &delegate, &metadata, &None);

    let all_events = env.events().all();
    assert!(all_events.len() > 0);
}

#[test]
fn test_new_apis() {
    let env = Env::default();
    set_ledger_time(&env, 1000);

    let contract_id = env.register(DelegationManager, ());
    let client = DelegationManagerClient::new(&env, &contract_id);

    let owner = Address::generate(&env);
    let delegate = Address::generate(&env);

    let tags = Vec::new(&env);
    let metadata = DelegationMetadata {
        label: String::from_str(&env, "Agent API Test"),
        strategy_id: Some(101),
        description: Some(String::from_str(&env, "Testing new APIs")),
        tags,
    };

    env.mock_all_auths();

    // 1. Create pending delegation
    let id = client.create_delegation(&owner, &delegate, &metadata, &Some(2000));
    
    // verify_authority should be false (pending)
    assert!(!client.verify_authority(&owner, &delegate));
    
    // get_active_delegation should be None (pending)
    assert!(client.get_active_delegation(&owner, &delegate).is_none());

    // get_delegation_by_pair should be Some (pending is mapped)
    let delegation_by_pair = client.get_delegation_by_pair(&owner, &delegate).unwrap();
    assert_eq!(delegation_by_pair.id, id);
    assert_eq!(delegation_by_pair.status, DelegationStatus::Pending);

    // is_delegation_valid should be false (pending is not active)
    assert!(!client.is_delegation_valid(&id));

    // 2. Accept delegation (Active)
    client.accept_delegation(&id);

    // verify_authority should be true
    assert!(client.verify_authority(&owner, &delegate));

    // get_active_delegation should be Some
    let active_del = client.get_active_delegation(&owner, &delegate).unwrap();
    assert_eq!(active_del.id, id);
    assert_eq!(active_del.status, DelegationStatus::Active);

    // get_delegation_by_pair should be Some
    let pair_del = client.get_delegation_by_pair(&owner, &delegate).unwrap();
    assert_eq!(pair_del.status, DelegationStatus::Active);

    // is_delegation_valid should be true
    assert!(client.is_delegation_valid(&id));

    // 3. get_delegations batch query
    let mut query_ids = Vec::new(&env);
    query_ids.push_back(id);
    query_ids.push_back(999); // non-existent

    let batch = client.get_delegations(&query_ids);
    assert_eq!(batch.len(), 2);
    assert!(batch.get(0).unwrap().is_some());
    assert!(batch.get(1).unwrap().is_none());

    // 4. Pause delegation (Paused)
    client.pause_delegation(&id);
    assert!(!client.verify_authority(&owner, &delegate));
    assert!(client.get_active_delegation(&owner, &delegate).is_none());
    assert!(client.get_delegation_by_pair(&owner, &delegate).is_some());
    assert!(!client.is_delegation_valid(&id));

    // 5. Resume (Active again)
    client.resume_delegation(&id);
    assert!(client.verify_authority(&owner, &delegate));
    assert!(client.get_active_delegation(&owner, &delegate).is_some());

    // 6. Expired
    set_ledger_time(&env, 2500); // Beyond 2000
    assert!(!client.verify_authority(&owner, &delegate));
    assert!(client.get_active_delegation(&owner, &delegate).is_none());
    assert!(!client.is_delegation_valid(&id));
}


