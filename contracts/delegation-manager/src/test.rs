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
fn test_v3_lifecycle_and_validation() {
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

    // Verify Audit Trail & Version
    let delegation = client.get_delegation(&id).unwrap();
    assert_eq!(delegation.version, 1);
    assert_eq!(delegation.created_at, 1000);
    assert_eq!(delegation.updated_at, 1000);
    assert_eq!(delegation.expires_at, expires_at);

    // Verify Validation APIs
    assert!(client.delegation_exists(&id));
    assert!(client.is_active_delegation(&id));
    assert!(!client.is_paused(&id));
    assert!(!client.is_revoked(&id));
    assert!(!client.is_expired(&id));
    assert_eq!(client.get_owner(&id).unwrap(), owner);
    assert_eq!(client.get_delegate(&id).unwrap(), delegate);
    assert_eq!(client.get_delegate_status(&id).unwrap(), DelegationStatus::Active);

    // Verify Stats
    let owner_stats = client.get_owner_stats(&owner).unwrap();
    assert_eq!(owner_stats.total_created, 1);
    assert_eq!(owner_stats.active_count, 1);

    let delegate_stats = client.get_delegate_stats(&delegate).unwrap();
    assert_eq!(delegate_stats.total_received, 1);
    assert_eq!(delegate_stats.active_received, 1);

    // 2. Pause Delegation at timestamp 2000
    set_ledger_time(&env, 2000);
    client.pause_delegation(&id);

    let delegation = client.get_delegation(&id).unwrap();
    assert_eq!(delegation.status, DelegationStatus::Paused);
    assert_eq!(delegation.version, 2);
    assert_eq!(delegation.paused_at, Some(2000));
    assert_eq!(delegation.updated_at, 2000);

    assert!(!client.is_active_delegation(&id));
    assert!(client.is_paused(&id));

    // Verify Stats update
    let owner_stats = client.get_owner_stats(&owner).unwrap();
    assert_eq!(owner_stats.active_count, 0);
    assert_eq!(owner_stats.paused_count, 1);

    // 3. Resume Delegation at timestamp 3000
    set_ledger_time(&env, 3000);
    client.resume_delegation(&id);

    let delegation = client.get_delegation(&id).unwrap();
    assert_eq!(delegation.status, DelegationStatus::Active);
    assert_eq!(delegation.version, 3);
    assert_eq!(delegation.resumed_at, Some(3000));
    assert_eq!(delegation.updated_at, 3000);

    // 4. Update Metadata
    let updated_tags = Vec::new(&env);
    let updated_metadata = DelegationMetadata {
        label: String::from_str(&env, "New Executor Label"),
        strategy_id: Some(99),
        description: None,
        tags: updated_tags,
    };
    client.update_metadata(&id, &updated_metadata);

    let delegation = client.get_delegation(&id).unwrap();
    assert_eq!(delegation.metadata.label, String::from_str(&env, "New Executor Label"));
    assert_eq!(delegation.version, 4);

    // 5. Expiry test
    set_ledger_time(&env, 5000); // 5000 is > 4600 expiry time
    assert!(client.is_expired(&id));
    assert!(!client.is_active_delegation(&id)); // Validation API rejects expired delegation

    // Check statistics are preserved
    let owner_stats = client.get_owner_stats(&owner).unwrap();
    assert_eq!(owner_stats.total_created, 1);

    // Revoke delegation after expiry (allowed)
    client.revoke_delegation(&id);
    let delegation = client.get_delegation(&id).unwrap();
    assert_eq!(delegation.status, DelegationStatus::Revoked);
    assert_eq!(delegation.version, 5);
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
#[should_panic(expected = "duplicate active/paused delegation not allowed")]
fn test_duplicate_prevention_v3() {
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
    // Should panic due to duplicate active delegation
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
    let _id_1 = client.create_delegation(&owner, &delegate, &metadata, &Some(1500));
    
    // 1. Recreate after expiry
    set_ledger_time(&env, 2000); // Wait until expired
    let id_2 = client.create_delegation(&owner, &delegate, &metadata, &None);
    assert_eq!(id_2, 2);

    // 2. Revoke active one
    client.revoke_delegation(&id_2);

    // 3. Recreate after revoked
    let id_3 = client.create_delegation(&owner, &delegate, &metadata, &None);
    assert_eq!(id_3, 3);
}

#[test]
fn test_events_v2() {
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
