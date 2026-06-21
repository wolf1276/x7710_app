#![no_std]
use soroban_sdk::{
    contract, contractimpl, contracttype, Address, Env, String, Symbol, Vec,
};

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub enum PolicyStatus {
    Pending,
    Active,
    Paused,
    Revoked,
    Expired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct PolicyParams {
    pub strategy_id: Option<u64>,
    pub valid_from: u64,
    pub valid_until: Option<u64>,
    pub max_notional_per_tx: u128,
    pub max_notional_per_day: u128,
    pub allowed_assets: Vec<Address>,
    pub denied_assets: Vec<Address>,
    pub allowed_protocols: Vec<Address>,
    pub denied_protocols: Vec<Address>,
    pub metadata: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct Policy {
    pub id: u64,
    pub delegation_id: Option<u64>,
    pub owner: Address,
    pub delegate: Address,
    pub status: PolicyStatus,
    pub params: PolicyParams,
    pub created_at: u64,
    pub updated_at: u64,
    pub version: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub enum DataKey {
    Counter,
    Policy(u64),
    DelegationPolicies(u64),
    ActivePolicyId(u64),
    DelegationManager,
    ExecutionRouter,
    Admin,
    DailyNotional(u64, u64), // (policy_id, day_id) -> spent amount
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub enum ValidationResult {
    Approved,
    Rejected(String),
}

pub mod delegation_manager_client {
    use soroban_sdk::{contractclient, Address, Env};

    #[contractclient(name = "DelegationManagerClient")]
    pub trait DelegationManagerInterface {
        fn delegation_exists(env: Env, delegation_id: u64) -> bool;
        fn is_active_delegation(env: Env, delegation_id: u64) -> bool;
        fn get_owner(env: Env, delegation_id: u64) -> Option<Address>;
    }
}

const BUMP_THRESHOLD: u32 = 100_000;
const BUMP_LIMIT: u32 = 500_000;

fn bump_ttl(env: &Env, key: &DataKey) {
    if env.storage().persistent().has(key) {
        env.storage().persistent().extend_ttl(key, BUMP_THRESHOLD, BUMP_LIMIT);
    }
}

#[contract]
pub struct PolicyEngine;

impl PolicyEngine {
    fn ensure_not_expired(env: &Env, policy_id: u64) {
        let key = DataKey::Policy(policy_id);
        if let Some(mut policy) = env.storage().persistent().get::<_, Policy>(&key) {
            let now = env.ledger().timestamp();
            if let Some(until) = policy.params.valid_until {
                if until <= now
                    && policy.status != PolicyStatus::Revoked
                    && policy.status != PolicyStatus::Expired
                {
                    policy.status = PolicyStatus::Expired;
                    policy.version = policy.version.checked_add(1).expect("version overflow");
                    policy.updated_at = now;

                    // Remove from ActivePolicyId
                    if let Some(del_id) = policy.delegation_id {
                        let active_key = DataKey::ActivePolicyId(del_id);
                        if let Some(act_id) = env.storage().persistent().get::<_, u64>(&active_key) {
                            if act_id == policy_id {
                                env.storage().persistent().remove(&active_key);
                            }
                        }
                    }

                    // Save
                    env.storage().persistent().set(&key, &policy);

                    // Emit event
                    env.events().publish(
                        (Symbol::new(env, "policy_expired"), policy_id, policy.owner.clone(), policy.delegate.clone()),
                        (now, policy.version),
                    );
                }
            }
            bump_ttl(env, &key);
        }
    }
}

#[contractimpl]
impl PolicyEngine {
    pub fn initialize(env: Env, delegation_manager: Address, admin: Address) {
        if env.storage().persistent().has(&DataKey::DelegationManager) {
            panic!("already initialized");
        }
        env.storage().persistent().set(&DataKey::DelegationManager, &delegation_manager);
        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage().persistent().set(&DataKey::Counter, &0u64);

        bump_ttl(&env, &DataKey::DelegationManager);
        bump_ttl(&env, &DataKey::Admin);
        bump_ttl(&env, &DataKey::Counter);
    }

    pub fn get_delegation_manager(env: Env) -> Address {
        let key = DataKey::DelegationManager;
        let addr = env.storage().persistent().get::<_, Address>(&key)
            .unwrap_or_else(|| panic!("not initialized"));
        bump_ttl(&env, &key);
        addr
    }

    pub fn get_admin(env: Env) -> Address {
        let key = DataKey::Admin;
        let addr = env.storage().persistent().get::<_, Address>(&key)
            .unwrap_or_else(|| panic!("not initialized"));
        bump_ttl(&env, &key);
        addr
    }

    pub fn set_execution_router(env: Env, router: Address) {
        let admin = Self::get_admin(env.clone());
        admin.require_auth();
        let key = DataKey::ExecutionRouter;
        env.storage().persistent().set(&key, &router);
        bump_ttl(&env, &key);
    }

    pub fn get_execution_router(env: Env) -> Option<Address> {
        let key = DataKey::ExecutionRouter;
        let val = env.storage().persistent().get::<_, Address>(&key);
        if val.is_some() {
            bump_ttl(&env, &key);
        }
        val
    }

    pub fn get_remaining_daily_limit(env: Env, delegation_id: u64, timestamp: u64) -> u128 {
        let active_key = DataKey::ActivePolicyId(delegation_id);
        let active_id = match env.storage().persistent().get::<_, u64>(&active_key) {
            Some(id) => id,
            None => return 0,
        };
        bump_ttl(&env, &active_key);
        Self::ensure_not_expired(&env, active_id);

        let policy = match Self::get_policy(env.clone(), active_id) {
            Some(p) => p,
            None => return 0,
        };
        let day_id = timestamp / 86400;
        let daily_key = DataKey::DailyNotional(active_id, day_id);
        let spent = env.storage().persistent().get::<_, u128>(&daily_key).unwrap_or(0);
        bump_ttl(&env, &daily_key);

        if spent >= policy.params.max_notional_per_day {
            0
        } else {
            policy.params.max_notional_per_day - spent
        }
    }

    pub fn create_policy(
        env: Env,
        owner: Address,
        delegate: Address,
        params: PolicyParams,
    ) -> u64 {
        owner.require_auth();

        let now = env.ledger().timestamp();
        if let Some(until) = params.valid_until {
            if until <= now {
                panic!("valid_until must be in the future");
            }
        }

        let mut counter = env.storage().persistent().get::<_, u64>(&DataKey::Counter).unwrap_or(0);
        counter = counter.checked_add(1).expect("version overflow");
        env.storage().persistent().set(&DataKey::Counter, &counter);
        bump_ttl(&env, &DataKey::Counter);

        let policy = Policy {
            id: counter,
            delegation_id: None,
            owner: owner.clone(),
            delegate: delegate.clone(),
            status: PolicyStatus::Pending,
            params,
            created_at: now,
            updated_at: now,
            version: 1,
        };

        let key = DataKey::Policy(counter);
        env.storage().persistent().set(&key, &policy);
        bump_ttl(&env, &key);

        env.events().publish(
            (Symbol::new(&env, "policy_created"), counter, owner, delegate),
            (now, 1u32),
        );

        counter
    }

    pub fn accept_policy(env: Env, policy_id: u64) {
        let key = DataKey::Policy(policy_id);
        let mut policy = env.storage().persistent().get::<_, Policy>(&key)
            .unwrap_or_else(|| panic!("policy not found"));

        policy.delegate.require_auth();

        if policy.status != PolicyStatus::Pending {
            panic!("policy not pending");
        }

        let now = env.ledger().timestamp();
        policy.status = PolicyStatus::Active;
        policy.version = policy.version.checked_add(1).expect("version overflow");
        policy.updated_at = now;

        env.storage().persistent().set(&key, &policy);
        bump_ttl(&env, &key);

        // If assigned, we update the active policy mapping (preventing multiple active policies)
        if let Some(del_id) = policy.delegation_id {
            let active_key = DataKey::ActivePolicyId(del_id);
            if let Some(existing_id) = env.storage().persistent().get::<_, u64>(&active_key) {
                if let Some(existing_policy) = Self::get_policy(env.clone(), existing_id) {
                    let is_expired = existing_policy.status == PolicyStatus::Expired 
                        || (existing_policy.params.valid_until.is_some() && existing_policy.params.valid_until.unwrap() <= now);
                    if existing_policy.status == PolicyStatus::Active && !is_expired && existing_id != policy_id {
                        panic!("delegation already has an active policy");
                    }
                }
            }
            env.storage().persistent().set(&active_key, &policy_id);
            bump_ttl(&env, &active_key);
        }

        env.events().publish(
            (Symbol::new(&env, "policy_updated"), policy_id, policy.owner.clone(), policy.delegate.clone()),
            (now, policy.version),
        );
    }

    pub fn pause_policy(env: Env, policy_id: u64) {
        Self::ensure_not_expired(&env, policy_id);

        let key = DataKey::Policy(policy_id);
        let mut policy = env.storage().persistent().get::<_, Policy>(&key)
            .unwrap_or_else(|| panic!("policy not found"));

        policy.owner.require_auth();

        if policy.status != PolicyStatus::Active {
            panic!("policy not active");
        }

        let now = env.ledger().timestamp();
        policy.status = PolicyStatus::Paused;
        policy.version = policy.version.checked_add(1).expect("version overflow");
        policy.updated_at = now;

        env.storage().persistent().set(&key, &policy);
        bump_ttl(&env, &key);

        env.events().publish(
            (Symbol::new(&env, "policy_paused"), policy_id, policy.owner.clone(), policy.delegate.clone()),
            (now, policy.version),
        );
    }

    pub fn resume_policy(env: Env, policy_id: u64) {
        Self::ensure_not_expired(&env, policy_id);

        let key = DataKey::Policy(policy_id);
        let mut policy = env.storage().persistent().get::<_, Policy>(&key)
            .unwrap_or_else(|| panic!("policy not found"));

        policy.owner.require_auth();

        if policy.status != PolicyStatus::Paused {
            panic!("policy not paused");
        }

        let now = env.ledger().timestamp();
        policy.status = PolicyStatus::Active;
        policy.version = policy.version.checked_add(1).expect("version overflow");
        policy.updated_at = now;

        env.storage().persistent().set(&key, &policy);
        bump_ttl(&env, &key);

        env.events().publish(
            (Symbol::new(&env, "policy_resumed"), policy_id, policy.owner.clone(), policy.delegate.clone()),
            (now, policy.version),
        );
    }

    pub fn revoke_policy(env: Env, policy_id: u64) {
        let key = DataKey::Policy(policy_id);
        let mut policy = env.storage().persistent().get::<_, Policy>(&key)
            .unwrap_or_else(|| panic!("policy not found"));

        policy.owner.require_auth();

        if policy.status == PolicyStatus::Revoked {
            panic!("already revoked");
        }

        let now = env.ledger().timestamp();
        policy.status = PolicyStatus::Revoked;
        policy.version = policy.version.checked_add(1).expect("version overflow");
        policy.updated_at = now;

        env.storage().persistent().set(&key, &policy);
        bump_ttl(&env, &key);

        if let Some(del_id) = policy.delegation_id {
            let active_key = DataKey::ActivePolicyId(del_id);
            if let Some(act_id) = env.storage().persistent().get::<_, u64>(&active_key) {
                if act_id == policy_id {
                    env.storage().persistent().remove(&active_key);
                }
            }
        }

        env.events().publish(
            (Symbol::new(&env, "policy_revoked"), policy_id, policy.owner.clone(), policy.delegate.clone()),
            (now, policy.version),
        );
    }

    pub fn update_policy(
        env: Env,
        policy_id: u64,
        params: PolicyParams,
    ) {
        Self::ensure_not_expired(&env, policy_id);

        let key = DataKey::Policy(policy_id);
        let mut policy = env.storage().persistent().get::<_, Policy>(&key)
            .unwrap_or_else(|| panic!("policy not found"));

        policy.owner.require_auth();

        if policy.status == PolicyStatus::Revoked {
            panic!("cannot update revoked policy");
        }

        let now = env.ledger().timestamp();
        policy.params = params;
        policy.version = policy.version.checked_add(1).expect("version overflow");
        policy.updated_at = now;

        env.storage().persistent().set(&key, &policy);
        bump_ttl(&env, &key);

        env.events().publish(
            (Symbol::new(&env, "policy_updated"), policy_id, policy.owner.clone(), policy.delegate.clone()),
            (now, policy.version),
        );
    }

    pub fn assign_policy_to_delegation(env: Env, policy_id: u64, delegation_id: u64) {
        Self::ensure_not_expired(&env, policy_id);

        let key = DataKey::Policy(policy_id);
        let mut policy = env.storage().persistent().get::<_, Policy>(&key)
            .unwrap_or_else(|| panic!("policy not found"));

        policy.owner.require_auth();

        let dm_address = Self::get_delegation_manager(env.clone());
        let dm_client = delegation_manager_client::DelegationManagerClient::new(&env, &dm_address);
        
        // Delegation activity validation
        if !dm_client.is_active_delegation(&delegation_id) {
            panic!("cannot assign policy to an inactive delegation");
        }

        let del_owner = dm_client.get_owner(&delegation_id)
            .unwrap_or_else(|| panic!("delegation not found"));

        if del_owner != policy.owner {
            panic!("policy owner must match delegation owner");
        }

        policy.delegation_id = Some(delegation_id);
        env.storage().persistent().set(&key, &policy);
        bump_ttl(&env, &key);

        // Add to DelegationPolicies
        let list_key = DataKey::DelegationPolicies(delegation_id);
        let mut list = env.storage().persistent().get::<_, Vec<u64>>(&list_key)
            .unwrap_or_else(|| Vec::new(&env));
        list.push_back(policy_id);
        env.storage().persistent().set(&list_key, &list);
        bump_ttl(&env, &list_key);

        // Set as active policy if policy is currently Active (multiple active policy prevention)
        if policy.status == PolicyStatus::Active {
            let active_key = DataKey::ActivePolicyId(delegation_id);
            let now = env.ledger().timestamp();
            if let Some(existing_id) = env.storage().persistent().get::<_, u64>(&active_key) {
                if let Some(existing_policy) = Self::get_policy(env.clone(), existing_id) {
                    let is_expired = existing_policy.status == PolicyStatus::Expired 
                        || (existing_policy.params.valid_until.is_some() && existing_policy.params.valid_until.unwrap() <= now);
                    if existing_policy.status == PolicyStatus::Active && !is_expired && existing_id != policy_id {
                        panic!("delegation already has an active policy");
                    }
                }
            }
            env.storage().persistent().set(&active_key, &policy_id);
            bump_ttl(&env, &active_key);
        }

        let now = env.ledger().timestamp();
        env.events().publish(
            (Symbol::new(&env, "policy_assigned"), policy_id, delegation_id),
            (now, policy.version),
        );
    }

    pub fn unassign_policy(env: Env, policy_id: u64) {
        Self::ensure_not_expired(&env, policy_id);

        let key = DataKey::Policy(policy_id);
        let mut policy = env.storage().persistent().get::<_, Policy>(&key)
            .unwrap_or_else(|| panic!("policy not found"));

        policy.owner.require_auth();

        if let Some(del_id) = policy.delegation_id {
            // Remove from active policy mapping if active
            let active_key = DataKey::ActivePolicyId(del_id);
            if let Some(act_id) = env.storage().persistent().get::<_, u64>(&active_key) {
                if act_id == policy_id {
                    env.storage().persistent().remove(&active_key);
                }
            }

            // Remove from list
            let list_key = DataKey::DelegationPolicies(del_id);
            if let Some(list) = env.storage().persistent().get::<_, Vec<u64>>(&list_key) {
                let mut new_list = Vec::new(&env);
                for id in list {
                    if id != policy_id {
                        new_list.push_back(id);
                    }
                }
                env.storage().persistent().set(&list_key, &new_list);
                bump_ttl(&env, &list_key);
            }

            policy.delegation_id = None;
            env.storage().persistent().set(&key, &policy);
            bump_ttl(&env, &key);
        }
    }

    pub fn get_policy(env: Env, policy_id: u64) -> Option<Policy> {
        let key = DataKey::Policy(policy_id);
        let val = env.storage().persistent().get::<_, Policy>(&key);
        if val.is_some() {
            bump_ttl(&env, &key);
            Self::ensure_not_expired(&env, policy_id);
            // Retrieve again to get updated status
            return env.storage().persistent().get::<_, Policy>(&key);
        }
        val
    }

    pub fn get_policies_by_delegation(env: Env, delegation_id: u64) -> Vec<Policy> {
        let list_key = DataKey::DelegationPolicies(delegation_id);
        let list = env.storage().persistent().get::<_, Vec<u64>>(&list_key)
            .unwrap_or_else(|| Vec::new(&env));
        bump_ttl(&env, &list_key);

        let mut result = Vec::new(&env);
        for id in list {
            if let Some(policy) = Self::get_policy(env.clone(), id) {
                result.push_back(policy);
            }
        }
        result
    }

    pub fn get_active_policy(env: Env, delegation_id: u64) -> Option<Policy> {
        let active_key = DataKey::ActivePolicyId(delegation_id);
        let active_id = env.storage().persistent().get::<_, u64>(&active_key)?;
        bump_ttl(&env, &active_key);
        Self::ensure_not_expired(&env, active_id);

        let policy = Self::get_policy(env.clone(), active_id)?;
        if policy.status == PolicyStatus::Expired {
            return None;
        }
        Some(policy)
    }

    pub fn validate_action(
        env: Env,
        delegation_id: u64,
        asset: Address,
        protocol: Address,
        amount: u128,
        timestamp: u64,
    ) -> ValidationResult {
        let dm_address = match env.storage().persistent().get::<_, Address>(&DataKey::DelegationManager) {
            Some(addr) => addr,
            None => return ValidationResult::Rejected(String::from_str(&env, "Delegation Manager not initialized")),
        };
        bump_ttl(&env, &DataKey::DelegationManager);
        let dm_client = delegation_manager_client::DelegationManagerClient::new(&env, &dm_address);

        if !dm_client.delegation_exists(&delegation_id) {
            return ValidationResult::Rejected(String::from_str(&env, "delegation does not exist"));
        }
        if !dm_client.is_active_delegation(&delegation_id) {
            return ValidationResult::Rejected(String::from_str(&env, "delegation not active"));
        }

        let policy_id = match env.storage().persistent().get::<_, u64>(&DataKey::ActivePolicyId(delegation_id)) {
            Some(id) => id,
            None => return ValidationResult::Rejected(String::from_str(&env, "no active policy assigned")),
        };
        bump_ttl(&env, &DataKey::ActivePolicyId(delegation_id));
        Self::ensure_not_expired(&env, policy_id);

        let policy = match Self::get_policy(env.clone(), policy_id) {
            Some(p) => p,
            None => return ValidationResult::Rejected(String::from_str(&env, "policy not found")),
        };

        if policy.status == PolicyStatus::Expired {
            return ValidationResult::Rejected(String::from_str(&env, "policy expired"));
        }

        if policy.status != PolicyStatus::Active {
            return ValidationResult::Rejected(String::from_str(&env, "policy not active"));
        }

        if timestamp < policy.params.valid_from {
            return ValidationResult::Rejected(String::from_str(&env, "policy not yet valid"));
        }
        if let Some(until) = policy.params.valid_until {
            if timestamp > until {
                return ValidationResult::Rejected(String::from_str(&env, "policy expired"));
            }
        }

        for a in policy.params.denied_assets.iter() {
            if a == asset {
                return ValidationResult::Rejected(String::from_str(&env, "asset denied"));
            }
        }

        // Check asset allowance
        if policy.params.allowed_assets.len() > 0 {
            let mut allowed = false;
            for a in policy.params.allowed_assets.iter() {
                if a == asset {
                    allowed = true;
                    break;
                }
            }
            if !allowed {
                return ValidationResult::Rejected(String::from_str(&env, "asset not allowed"));
            }
        }

        // Check protocol denial
        for p in policy.params.denied_protocols.iter() {
            if p == protocol {
                return ValidationResult::Rejected(String::from_str(&env, "protocol denied"));
            }
        }

        // Check protocol allowance
        if policy.params.allowed_protocols.len() > 0 {
            let mut allowed = false;
            for p in policy.params.allowed_protocols.iter() {
                if p == protocol {
                    allowed = true;
                    break;
                }
            }
            if !allowed {
                return ValidationResult::Rejected(String::from_str(&env, "protocol not allowed"));
            }
        }

        // Check amounts
        if amount > policy.params.max_notional_per_tx {
            return ValidationResult::Rejected(String::from_str(&env, "amount exceeds max_notional_per_tx"));
        }

        let day_id = timestamp / 86400;
        let daily_key = DataKey::DailyNotional(policy_id, day_id);
        let spent = env.storage().persistent().get::<_, u128>(&daily_key).unwrap_or(0);
        bump_ttl(&env, &daily_key);

        let new_spent = match spent.checked_add(amount) {
            Some(s) => s,
            None => return ValidationResult::Rejected(String::from_str(&env, "spent overflow")),
        };
        if new_spent > policy.params.max_notional_per_day {
            return ValidationResult::Rejected(String::from_str(&env, "amount exceeds daily limit"));
        }

        ValidationResult::Approved
    }

    pub fn record_action(env: Env, delegation_id: u64, amount: u128, timestamp: u64) {
        let router = Self::get_execution_router(env.clone())
            .unwrap_or_else(|| panic!("Execution Router not set"));
        router.require_auth();

        let active_id = env.storage().persistent().get::<_, u64>(&DataKey::ActivePolicyId(delegation_id))
            .unwrap_or_else(|| panic!("no active policy assigned"));
        bump_ttl(&env, &DataKey::ActivePolicyId(delegation_id));
        Self::ensure_not_expired(&env, active_id);

        let day_id = timestamp / 86400;
        let daily_key = DataKey::DailyNotional(active_id, day_id);
        let spent = env.storage().persistent().get::<_, u128>(&daily_key).unwrap_or(0);
        let new_spent = spent.checked_add(amount).expect("spent overflow");
        env.storage().persistent().set(&daily_key, &new_spent);
        bump_ttl(&env, &daily_key);
    }
}

#[cfg(test)]
mod test;

