#![no_std]
use soroban_sdk::{
    contract, contractimpl, contracttype, Address, BytesN, Env, String, Symbol,
};

use delegation_manager::DelegationManagerClient;
use policy_engine::{PolicyEngineClient, ValidationResult};

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct ExecutionIntent {
    pub intent_hash: BytesN<32>,
    pub delegation_id: u64,
    pub policy_id: u64,
    pub delegate: Address,
    pub asset: Address,
    pub protocol: Address,
    pub action_type: Symbol,
    pub amount: u128,
    pub target: Address,
    pub payload_hash: BytesN<32>,
    pub nonce: u64,
    pub timestamp: u64,
    pub expiry: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub enum ExecutionStatus {
    Pending,
    Approved,
    Executed,
    Rejected,
    Expired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct Execution {
    pub execution_id: u64,
    pub intent_hash: BytesN<32>,
    pub delegation_id: u64,
    pub policy_id: u64,
    pub delegate: Address,
    pub asset: Address,
    pub protocol: Address,
    pub action_type: Symbol,
    pub amount: u128,
    pub target: Address,
    pub timestamp: u64,
    pub status: ExecutionStatus,
    pub rejection_reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub enum DataKey {
    Counter,
    Execution(u64),
    IntentHash(BytesN<32>),
    DelegationManager,
    PolicyEngine,
    Admin,
}

#[contract]
pub struct ExecutionRouter;

#[contractimpl]
impl ExecutionRouter {
    pub fn initialize(
        env: Env,
        admin: Address,
        delegation_manager: Address,
        policy_engine: Address,
    ) {
        if env.storage().persistent().has(&DataKey::Admin) {
            panic!("already initialized");
        }
        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage()
            .persistent()
            .set(&DataKey::DelegationManager, &delegation_manager);
        env.storage()
            .persistent()
            .set(&DataKey::PolicyEngine, &policy_engine);
        env.storage().persistent().set(&DataKey::Counter, &0u64);
    }

    pub fn set_delegation_manager(env: Env, delegation_manager: Address) {
        let admin = Self::get_admin(env.clone());
        admin.require_auth();
        env.storage()
            .persistent()
            .set(&DataKey::DelegationManager, &delegation_manager);
    }

    pub fn set_policy_engine(env: Env, policy_engine: Address) {
        let admin = Self::get_admin(env.clone());
        admin.require_auth();
        env.storage()
            .persistent()
            .set(&DataKey::PolicyEngine, &policy_engine);
    }

    pub fn get_delegation_manager(env: Env) -> Address {
        env.storage()
            .persistent()
            .get(&DataKey::DelegationManager)
            .unwrap_or_else(|| panic!("not initialized"))
    }

    pub fn get_policy_engine(env: Env) -> Address {
        env.storage()
            .persistent()
            .get(&DataKey::PolicyEngine)
            .unwrap_or_else(|| panic!("not initialized"))
    }

    pub fn get_admin(env: Env) -> Address {
        env.storage()
            .persistent()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic!("not initialized"))
    }

    pub fn validate_execution(env: Env, intent: ExecutionIntent) -> ValidationResult {
        let now = env.ledger().timestamp();

        // 1. intent not expired
        if now >= intent.expiry {
            return ValidationResult::Rejected(String::from_str(&env, "intent expired"));
        }

        // 2. intent hash not seen before
        if env
            .storage()
            .persistent()
            .has(&DataKey::IntentHash(intent.intent_hash.clone()))
        {
            return ValidationResult::Rejected(String::from_str(&env, "intent hash already seen"));
        }

        // Load clients
        let dm_address = Self::get_delegation_manager(env.clone());
        let pe_address = Self::get_policy_engine(env.clone());

        let dm_client = DelegationManagerClient::new(&env, &dm_address);
        let pe_client = PolicyEngineClient::new(&env, &pe_address);

        // 6. delegation active
        if !dm_client.is_active_delegation(&intent.delegation_id) {
            return ValidationResult::Rejected(String::from_str(&env, "delegation not active"));
        }

        // 3. verify_authority()
        let owner = match dm_client.get_owner(&intent.delegation_id) {
            Some(o) => o,
            None => {
                return ValidationResult::Rejected(String::from_str(
                    &env,
                    "delegation does not exist",
                ))
            }
        };

        let delegate = match dm_client.get_delegate(&intent.delegation_id) {
            Some(d) => d,
            None => {
                return ValidationResult::Rejected(String::from_str(
                    &env,
                    "delegation does not exist",
                ))
            }
        };

        if delegate != intent.delegate {
            return ValidationResult::Rejected(String::from_str(&env, "delegate mismatch"));
        }

        if !dm_client.verify_authority(&owner, &intent.delegate) {
            return ValidationResult::Rejected(String::from_str(&env, "invalid authority"));
        }

        // 4. validate_action()
        let validation = pe_client.validate_action(
            &intent.delegation_id,
            &intent.asset,
            &intent.protocol,
            &intent.amount,
            &now,
        );
        if let ValidationResult::Rejected(reason) = validation {
            return ValidationResult::Rejected(reason);
        }

        // 5. policy active
        let active_policy = match pe_client.get_active_policy(&intent.delegation_id) {
            Some(p) => p,
            None => {
                return ValidationResult::Rejected(String::from_str(
                    &env,
                    "no active policy assigned",
                ))
            }
        };
        if active_policy.id != intent.policy_id {
            return ValidationResult::Rejected(String::from_str(&env, "policy mismatch"));
        }
        if active_policy.status != policy_engine::PolicyStatus::Active {
            return ValidationResult::Rejected(String::from_str(&env, "policy not active"));
        }

        ValidationResult::Approved
    }

    pub fn execute_intent(env: Env, intent: ExecutionIntent) -> u64 {
        // 1. validate_execution()
        let validation = Self::validate_execution(env.clone(), intent.clone());

        let mut counter = env
            .storage()
            .persistent()
            .get::<_, u64>(&DataKey::Counter)
            .unwrap_or(0);
        counter += 1;
        env.storage().persistent().set(&DataKey::Counter, &counter);

        let execution_id = counter;
        let now = env.ledger().timestamp();

        match validation {
            ValidationResult::Approved => {
                // 2. record_action()
                let pe_address = Self::get_policy_engine(env.clone());
                let pe_client = PolicyEngineClient::new(&env, &pe_address);
                pe_client.record_action(&intent.delegation_id, &intent.amount, &now);

                // 3. create execution record
                let execution = Execution {
                    execution_id,
                    intent_hash: intent.intent_hash.clone(),
                    delegation_id: intent.delegation_id,
                    policy_id: intent.policy_id,
                    delegate: intent.delegate.clone(),
                    asset: intent.asset.clone(),
                    protocol: intent.protocol.clone(),
                    action_type: intent.action_type.clone(),
                    amount: intent.amount,
                    target: intent.target.clone(),
                    timestamp: now,
                    status: ExecutionStatus::Executed,
                    rejection_reason: None,
                };
                env.storage()
                    .persistent()
                    .set(&DataKey::Execution(execution_id), &execution);

                // 4. mark intent hash consumed
                env.storage()
                    .persistent()
                    .set(&DataKey::IntentHash(intent.intent_hash.clone()), &execution_id);

                // 5. emit execution events
                env.events().publish(
                    (Symbol::new(&env, "execution_requested"), execution_id),
                    (
                        intent.intent_hash.clone(),
                        intent.delegation_id,
                        intent.policy_id,
                        intent.amount,
                        now,
                    ),
                );
                env.events().publish(
                    (Symbol::new(&env, "execution_validated"), execution_id),
                    (
                        intent.intent_hash.clone(),
                        intent.delegation_id,
                        intent.policy_id,
                        intent.amount,
                        now,
                    ),
                );
                env.events().publish(
                    (Symbol::new(&env, "execution_approved"), execution_id),
                    (
                        intent.intent_hash.clone(),
                        intent.delegation_id,
                        intent.policy_id,
                        intent.amount,
                        now,
                    ),
                );
                env.events().publish(
                    (Symbol::new(&env, "execution_executed"), execution_id),
                    (
                        intent.intent_hash.clone(),
                        intent.delegation_id,
                        intent.policy_id,
                        intent.amount,
                        now,
                    ),
                );

                execution_id
            }
            ValidationResult::Rejected(reason) => {
                // If it is a replay attempt, panic to reject transaction and avoid creating duplicate records.
                if reason == String::from_str(&env, "intent hash already seen") {
                    panic!("intent hash already seen");
                }

                // Create rejected execution record
                let execution = Execution {
                    execution_id,
                    intent_hash: intent.intent_hash.clone(),
                    delegation_id: intent.delegation_id,
                    policy_id: intent.policy_id,
                    delegate: intent.delegate.clone(),
                    asset: intent.asset.clone(),
                    protocol: intent.protocol.clone(),
                    action_type: intent.action_type.clone(),
                    amount: intent.amount,
                    target: intent.target.clone(),
                    timestamp: now,
                    status: ExecutionStatus::Rejected,
                    rejection_reason: Some(reason.clone()),
                };
                env.storage()
                    .persistent()
                    .set(&DataKey::Execution(execution_id), &execution);

                // Mark intent hash consumed
                env.storage()
                    .persistent()
                    .set(&DataKey::IntentHash(intent.intent_hash.clone()), &execution_id);

                env.events().publish(
                    (Symbol::new(&env, "execution_requested"), execution_id),
                    (
                        intent.intent_hash.clone(),
                        intent.delegation_id,
                        intent.policy_id,
                        intent.amount,
                        now,
                    ),
                );
                env.events().publish(
                    (Symbol::new(&env, "execution_validated"), execution_id),
                    (
                        intent.intent_hash.clone(),
                        intent.delegation_id,
                        intent.policy_id,
                        intent.amount,
                        now,
                    ),
                );
                env.events().publish(
                    (Symbol::new(&env, "execution_rejected"), execution_id),
                    (
                        intent.intent_hash.clone(),
                        intent.delegation_id,
                        intent.policy_id,
                        intent.amount,
                        now,
                    ),
                );

                execution_id
            }
        }
    }

    pub fn simulate_intent(env: Env, intent: ExecutionIntent) -> ValidationResult {
        Self::validate_execution(env, intent)
    }

    pub fn get_execution(env: Env, execution_id: u64) -> Option<Execution> {
        env.storage()
            .persistent()
            .get(&DataKey::Execution(execution_id))
    }

    pub fn get_execution_by_hash(env: Env, intent_hash: BytesN<32>) -> Option<Execution> {
        if let Some(execution_id) = env
            .storage()
            .persistent()
            .get::<_, u64>(&DataKey::IntentHash(intent_hash))
        {
            Self::get_execution(env, execution_id)
        } else {
            None
        }
    }

    pub fn execution_exists(env: Env, execution_id: u64) -> bool {
        env.storage()
            .persistent()
            .has(&DataKey::Execution(execution_id))
    }

    pub fn get_execution_status(env: Env, execution_id: u64) -> Option<ExecutionStatus> {
        Self::get_execution(env, execution_id).map(|e| e.status)
    }
}

#[cfg(test)]
mod test;

