#![no_std]
use soroban_sdk::{
    contract, contractimpl, contracttype, Address, Env, String, Vec, Symbol,
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

#[contract]
pub struct PolicyEngine;

#[contractimpl]
impl PolicyEngine {
    pub fn initialize(env: Env, delegation_manager: Address, admin: Address) {
        if env.storage().persistent().has(&DataKey::DelegationManager) {
            panic!("already initialized");
        }
        env.storage().persistent().set(&DataKey::DelegationManager, &delegation_manager);
        env.storage().persistent().set(&DataKey::Admin, &admin);
    }

    pub fn get_delegation_manager(env: Env) -> Address {
        env.storage().persistent().get::<_, Address>(&DataKey::DelegationManager)
            .unwrap_or_else(|| panic!("not initialized"))
    }

    pub fn get_admin(env: Env) -> Address {
        env.storage().persistent().get::<_, Address>(&DataKey::Admin)
            .unwrap_or_else(|| panic!("not initialized"))
    }

    pub fn set_execution_router(env: Env, router: Address) {
        let admin = Self::get_admin(env.clone());
        admin.require_auth();
        env.storage().persistent().set(&DataKey::ExecutionRouter, &router);
    }

    pub fn get_execution_router(env: Env) -> Option<Address> {
        env.storage().persistent().get::<_, Address>(&DataKey::ExecutionRouter)
    }

    pub fn get_remaining_daily_limit(env: Env, delegation_id: u64, timestamp: u64) -> u128 {
        let active_id = match env.storage().persistent().get::<_, u64>(&DataKey::ActivePolicyId(delegation_id)) {
            Some(id) => id,
            None => return 0,
        };
        let policy = match Self::get_policy(env.clone(), active_id) {
            Some(p) => p,
            None => return 0,
        };
        let day_id = timestamp / 86400;
        let spent = env.storage().persistent().get::<_, u128>(&DataKey::DailyNotional(active_id, day_id)).unwrap_or(0);
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
        counter += 1;
        env.storage().persistent().set(&DataKey::Counter, &counter);

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

        env.storage().persistent().set(&DataKey::Policy(counter), &policy);

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
        policy.version += 1;
        policy.updated_at = now;

        env.storage().persistent().set(&key, &policy);

        // If assigned, we update the active policy mapping
        if let Some(del_id) = policy.delegation_id {
            env.storage().persistent().set(&DataKey::ActivePolicyId(del_id), &policy_id);
        }

        env.events().publish(
            (Symbol::new(&env, "policy_updated"), policy_id, policy.owner.clone(), policy.delegate.clone()),
            (now, policy.version),
        );
    }

    pub fn pause_policy(env: Env, policy_id: u64) {
        let key = DataKey::Policy(policy_id);
        let mut policy = env.storage().persistent().get::<_, Policy>(&key)
            .unwrap_or_else(|| panic!("policy not found"));

        policy.owner.require_auth();

        if policy.status != PolicyStatus::Active {
            panic!("policy not active");
        }

        let now = env.ledger().timestamp();
        policy.status = PolicyStatus::Paused;
        policy.version += 1;
        policy.updated_at = now;

        env.storage().persistent().set(&key, &policy);

        env.events().publish(
            (Symbol::new(&env, "policy_paused"), policy_id, policy.owner.clone(), policy.delegate.clone()),
            (now, policy.version),
        );
    }

    pub fn resume_policy(env: Env, policy_id: u64) {
        let key = DataKey::Policy(policy_id);
        let mut policy = env.storage().persistent().get::<_, Policy>(&key)
            .unwrap_or_else(|| panic!("policy not found"));

        policy.owner.require_auth();

        if policy.status != PolicyStatus::Paused {
            panic!("policy not paused");
        }

        let now = env.ledger().timestamp();
        policy.status = PolicyStatus::Active;
        policy.version += 1;
        policy.updated_at = now;

        env.storage().persistent().set(&key, &policy);

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
        policy.version += 1;
        policy.updated_at = now;

        env.storage().persistent().set(&key, &policy);

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
        let key = DataKey::Policy(policy_id);
        let mut policy = env.storage().persistent().get::<_, Policy>(&key)
            .unwrap_or_else(|| panic!("policy not found"));

        policy.owner.require_auth();

        if policy.status == PolicyStatus::Revoked {
            panic!("cannot update revoked policy");
        }

        let now = env.ledger().timestamp();
        policy.params = params;
        policy.version += 1;
        policy.updated_at = now;

        env.storage().persistent().set(&key, &policy);

        env.events().publish(
            (Symbol::new(&env, "policy_updated"), policy_id, policy.owner.clone(), policy.delegate.clone()),
            (now, policy.version),
        );
    }

    pub fn assign_policy_to_delegation(env: Env, policy_id: u64, delegation_id: u64) {
        let key = DataKey::Policy(policy_id);
        let mut policy = env.storage().persistent().get::<_, Policy>(&key)
            .unwrap_or_else(|| panic!("policy not found"));

        policy.owner.require_auth();

        let dm_address = Self::get_delegation_manager(env.clone());
        let dm_client = delegation_manager_client::DelegationManagerClient::new(&env, &dm_address);
        let del_owner = dm_client.get_owner(&delegation_id)
            .unwrap_or_else(|| panic!("delegation not found"));

        if del_owner != policy.owner {
            panic!("policy owner must match delegation owner");
        }

        policy.delegation_id = Some(delegation_id);
        env.storage().persistent().set(&key, &policy);

        // Add to DelegationPolicies
        let list_key = DataKey::DelegationPolicies(delegation_id);
        let mut list = env.storage().persistent().get::<_, Vec<u64>>(&list_key)
            .unwrap_or_else(|| Vec::new(&env));
        list.push_back(policy_id);
        env.storage().persistent().set(&list_key, &list);

        // Set as active policy if policy is currently Active
        if policy.status == PolicyStatus::Active {
            env.storage().persistent().set(&DataKey::ActivePolicyId(delegation_id), &policy_id);
        }

        let now = env.ledger().timestamp();
        env.events().publish(
            (Symbol::new(&env, "policy_assigned"), policy_id, delegation_id),
            (now, policy.version),
        );
    }

    pub fn unassign_policy(env: Env, policy_id: u64) {
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
            }

            policy.delegation_id = None;
            env.storage().persistent().set(&key, &policy);
        }
    }

    pub fn get_policy(env: Env, policy_id: u64) -> Option<Policy> {
        env.storage().persistent().get::<_, Policy>(&DataKey::Policy(policy_id))
    }

    pub fn get_policies_by_delegation(env: Env, delegation_id: u64) -> Vec<Policy> {
        let list_key = DataKey::DelegationPolicies(delegation_id);
        let list = env.storage().persistent().get::<_, Vec<u64>>(&list_key)
            .unwrap_or_else(|| Vec::new(&env));
        let mut result = Vec::new(&env);
        for id in list {
            if let Some(policy) = Self::get_policy(env.clone(), id) {
                result.push_back(policy);
            }
        }
        result
    }

    pub fn get_active_policy(env: Env, delegation_id: u64) -> Option<Policy> {
        let active_id = env.storage().persistent().get::<_, u64>(&DataKey::ActivePolicyId(delegation_id))?;
        Self::get_policy(env, active_id)
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

        let policy = match Self::get_policy(env.clone(), policy_id) {
            Some(p) => p,
            None => return ValidationResult::Rejected(String::from_str(&env, "policy not found")),
        };

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
        let spent = env.storage().persistent().get::<_, u128>(&DataKey::DailyNotional(policy_id, day_id)).unwrap_or(0);
        if spent + amount > policy.params.max_notional_per_day {
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
        let day_id = timestamp / 86400;
        let daily_key = DataKey::DailyNotional(active_id, day_id);
        let spent = env.storage().persistent().get::<_, u128>(&daily_key).unwrap_or(0);
        env.storage().persistent().set(&daily_key, &(spent + amount));
    }
}

#[cfg(test)]
mod test;
