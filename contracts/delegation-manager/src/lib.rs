#![no_std]
use soroban_sdk::{
    contract, contractimpl, contracttype, Address, Env, String, Symbol, Vec,
};

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub enum DelegationStatus {
    Pending,
    Active,
    Paused,
    Revoked,
    Expired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct DelegationMetadata {
    pub label: String,
    pub strategy_id: Option<u64>,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct Delegation {
    pub id: u64,
    pub owner: Address,
    pub delegate: Address,
    pub status: DelegationStatus,
    pub metadata: DelegationMetadata,
    pub version: u32,
    pub expires_at: Option<u64>,
    pub created_at: u64,
    pub updated_at: u64,
    pub paused_at: Option<u64>,
    pub resumed_at: Option<u64>,
    pub revoked_at: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub enum DataKey {
    Counter,
    Delegation(u64),
    ActiveDelegation(Address, Address),
}

const BUMP_THRESHOLD: u32 = 100_000;
const BUMP_LIMIT: u32 = 500_000;

fn bump_ttl(env: &Env, key: &DataKey) {
    env.storage().persistent().extend_ttl(key, BUMP_THRESHOLD, BUMP_LIMIT);
}

#[contract]
pub struct DelegationManager;

impl DelegationManager {
    fn ensure_not_expired(env: &Env, delegation_id: u64) {
        let key = DataKey::Delegation(delegation_id);
        if let Some(mut delegation) = env.storage().persistent().get::<_, Delegation>(&key) {
            let now = env.ledger().timestamp();
            if let Some(exp) = delegation.expires_at {
                if exp <= now
                    && delegation.status != DelegationStatus::Revoked
                    && delegation.status != DelegationStatus::Expired
                {
                    delegation.status = DelegationStatus::Expired;
                    delegation.version = delegation.version.checked_add(1).expect("version overflow");
                    delegation.updated_at = now;

                    // Remove ActiveDelegation mapping
                    let active_key = DataKey::ActiveDelegation(delegation.owner.clone(), delegation.delegate.clone());
                    if let Some(existing_id) = env.storage().persistent().get::<_, u64>(&active_key) {
                        if existing_id == delegation_id {
                            env.storage().persistent().remove(&active_key);
                        }
                    }

                    // Persist
                    env.storage().persistent().set(&key, &delegation);

                    // Emit event
                    env.events().publish(
                        (
                            Symbol::new(env, "delegation_expired"),
                            delegation_id,
                            delegation.owner.clone(),
                            delegation.delegate.clone(),
                        ),
                        (now, delegation.version),
                    );
                }
            }
            bump_ttl(env, &key);
        }
    }
}

#[contractimpl]
impl DelegationManager {
    /// Creates a delegation for a delegate in Pending status.
    pub fn create_delegation(
        env: Env,
        owner: Address,
        delegate: Address,
        metadata: DelegationMetadata,
        expires_at: Option<u64>,
    ) -> u64 {
        owner.require_auth();

        if owner == delegate {
            panic!("owner cannot delegate to self");
        }

        // Validate expiry if set
        let now = env.ledger().timestamp();
        if let Some(exp) = expires_at {
            if exp <= now {
                panic!("expiry must be in the future");
            }
        }

        let active_key = DataKey::ActiveDelegation(owner.clone(), delegate.clone());
        if let Some(existing_id) = env.storage().persistent().get::<_, u64>(&active_key) {
            if let Some(existing) = env
                .storage()
                .persistent()
                .get::<_, Delegation>(&DataKey::Delegation(existing_id))
            {
                // Check if existing delegation is still pending/active/paused and not expired
                let is_expired = existing.status == DelegationStatus::Expired 
                    || (existing.expires_at.is_some() && existing.expires_at.unwrap() <= now);
                if (existing.status == DelegationStatus::Pending 
                    || existing.status == DelegationStatus::Active 
                    || existing.status == DelegationStatus::Paused)
                    && !is_expired
                {
                    panic!("duplicate active/paused/pending delegation not allowed");
                }
            }
        }

        let mut counter = env
            .storage()
            .persistent()
            .get::<_, u64>(&DataKey::Counter)
            .unwrap_or(0);
        counter = counter.checked_add(1).expect("version overflow");
        env.storage().persistent().set(&DataKey::Counter, &counter);
        bump_ttl(&env, &DataKey::Counter);

        let delegation_id = counter;

        let delegation = Delegation {
            id: delegation_id,
            owner: owner.clone(),
            delegate: delegate.clone(),
            status: DelegationStatus::Pending,
            metadata,
            version: 1,
            expires_at,
            created_at: now,
            updated_at: now,
            paused_at: None,
            resumed_at: None,
            revoked_at: None,
        };

        // Save Delegation
        let del_key = DataKey::Delegation(delegation_id);
        env.storage().persistent().set(&del_key, &delegation);
        bump_ttl(&env, &del_key);

        // Update active delegation registry
        env.storage().persistent().set(&active_key, &delegation_id);
        bump_ttl(&env, &active_key);

        // Emit Event
        env.events().publish(
            (
                Symbol::new(&env, "delegation_created"),
                delegation_id,
                owner.clone(),
                delegate.clone(),
            ),
            (now, 1u32),
        );

        delegation_id
    }

    /// Delegate accepts the pending delegation.
    pub fn accept_delegation(env: Env, delegation_id: u64) {
        let key = DataKey::Delegation(delegation_id);
        let mut delegation = env
            .storage()
            .persistent()
            .get::<_, Delegation>(&key)
            .unwrap_or_else(|| panic!("delegation does not exist"));

        delegation.delegate.require_auth();

        let now = env.ledger().timestamp();
        // Check expiry
        if let Some(exp) = delegation.expires_at {
            if exp <= now {
                panic!("cannot accept an expired delegation");
            }
        }

        if delegation.status != DelegationStatus::Pending {
            panic!("delegation must be pending to accept");
        }

        delegation.status = DelegationStatus::Active;
        delegation.version = delegation.version.checked_add(1).expect("version overflow");
        delegation.updated_at = now;

        env.storage().persistent().set(&key, &delegation);
        bump_ttl(&env, &key);

        let active_key = DataKey::ActiveDelegation(delegation.owner.clone(), delegation.delegate.clone());
        env.storage().persistent().set(&active_key, &delegation_id);
        bump_ttl(&env, &active_key);

        // Emit Event
        env.events().publish(
            (
                Symbol::new(&env, "delegation_accepted"),
                delegation_id,
                delegation.owner.clone(),
                delegation.delegate.clone(),
            ),
            (now, delegation.version),
        );
    }

    /// Owner permanently revokes a delegation.
    pub fn revoke_delegation(env: Env, delegation_id: u64) {
        let key = DataKey::Delegation(delegation_id);
        let mut delegation = env
            .storage()
            .persistent()
            .get::<_, Delegation>(&key)
            .unwrap_or_else(|| panic!("delegation does not exist"));

        delegation.owner.require_auth();

        if delegation.status == DelegationStatus::Revoked {
            panic!("delegation already revoked");
        }

        let now = env.ledger().timestamp();
        delegation.status = DelegationStatus::Revoked;
        delegation.version = delegation.version.checked_add(1).expect("version overflow");
        delegation.revoked_at = Some(now);
        delegation.updated_at = now;

        env.storage().persistent().set(&key, &delegation);
        bump_ttl(&env, &key);

        // Remove from active registry mapping
        let active_key = DataKey::ActiveDelegation(delegation.owner.clone(), delegation.delegate.clone());
        if let Some(existing_id) = env.storage().persistent().get::<_, u64>(&active_key) {
            if existing_id == delegation_id {
                env.storage().persistent().remove(&active_key);
            }
        }

        // Emit Event
        env.events().publish(
            (
                Symbol::new(&env, "delegation_revoked"),
                delegation_id,
                delegation.owner.clone(),
                delegation.delegate.clone(),
            ),
            (now, delegation.version),
        );
    }

    /// Delegate renounces a delegation.
    pub fn renounce_delegation(env: Env, delegation_id: u64) {
        let key = DataKey::Delegation(delegation_id);
        let mut delegation = env
            .storage()
            .persistent()
            .get::<_, Delegation>(&key)
            .unwrap_or_else(|| panic!("delegation does not exist"));

        delegation.delegate.require_auth();

        if delegation.status == DelegationStatus::Revoked {
            panic!("delegation already revoked");
        }

        let now = env.ledger().timestamp();
        delegation.status = DelegationStatus::Revoked;
        delegation.version = delegation.version.checked_add(1).expect("version overflow");
        delegation.revoked_at = Some(now);
        delegation.updated_at = now;

        env.storage().persistent().set(&key, &delegation);
        bump_ttl(&env, &key);

        // Remove from active registry mapping
        let active_key = DataKey::ActiveDelegation(delegation.owner.clone(), delegation.delegate.clone());
        if let Some(existing_id) = env.storage().persistent().get::<_, u64>(&active_key) {
            if existing_id == delegation_id {
                env.storage().persistent().remove(&active_key);
            }
        }

        // Emit Event
        env.events().publish(
            (
                Symbol::new(&env, "delegation_renounced"),
                delegation_id,
                delegation.owner.clone(),
                delegation.delegate.clone(),
            ),
            (now, delegation.version),
        );
    }

    /// Owner temporarily disables a delegation.
    pub fn pause_delegation(env: Env, delegation_id: u64) {
        Self::ensure_not_expired(&env, delegation_id);

        let key = DataKey::Delegation(delegation_id);
        let mut delegation = env
            .storage()
            .persistent()
            .get::<_, Delegation>(&key)
            .unwrap_or_else(|| panic!("delegation does not exist"));

        delegation.owner.require_auth();

        let now = env.ledger().timestamp();
        // Check expiry
        if let Some(exp) = delegation.expires_at {
            if exp <= now {
                panic!("cannot pause an expired delegation");
            }
        }

        if delegation.status != DelegationStatus::Active {
            panic!("delegation must be active to pause");
        }

        delegation.status = DelegationStatus::Paused;
        delegation.version = delegation.version.checked_add(1).expect("version overflow");
        delegation.paused_at = Some(now);
        delegation.updated_at = now;

        env.storage().persistent().set(&key, &delegation);
        bump_ttl(&env, &key);

        // Emit Event
        env.events().publish(
            (
                Symbol::new(&env, "delegation_paused"),
                delegation_id,
                delegation.owner.clone(),
                delegation.delegate.clone(),
            ),
            (now, delegation.version),
        );
    }

    /// Owner re-enables a paused delegation.
    pub fn resume_delegation(env: Env, delegation_id: u64) {
        Self::ensure_not_expired(&env, delegation_id);

        let key = DataKey::Delegation(delegation_id);
        let mut delegation = env
            .storage()
            .persistent()
            .get::<_, Delegation>(&key)
            .unwrap_or_else(|| panic!("delegation does not exist"));

        delegation.owner.require_auth();

        let now = env.ledger().timestamp();
        // Check expiry
        if let Some(exp) = delegation.expires_at {
            if exp <= now {
                panic!("cannot resume an expired delegation");
            }
        }

        if delegation.status != DelegationStatus::Paused {
            panic!("delegation must be paused to resume");
        }

        delegation.status = DelegationStatus::Active;
        delegation.version = delegation.version.checked_add(1).expect("version overflow");
        delegation.resumed_at = Some(now);
        delegation.updated_at = now;

        env.storage().persistent().set(&key, &delegation);
        bump_ttl(&env, &key);

        // Emit Event
        env.events().publish(
            (
                Symbol::new(&env, "delegation_resumed"),
                delegation_id,
                delegation.owner.clone(),
                delegation.delegate.clone(),
            ),
            (now, delegation.version),
        );
    }

    /// Owner updates delegation metadata.
    pub fn update_metadata(env: Env, delegation_id: u64, metadata: DelegationMetadata) {
        Self::ensure_not_expired(&env, delegation_id);

        let key = DataKey::Delegation(delegation_id);
        let mut delegation = env
            .storage()
            .persistent()
            .get::<_, Delegation>(&key)
            .unwrap_or_else(|| panic!("delegation does not exist"));

        delegation.owner.require_auth();

        let now = env.ledger().timestamp();
        if delegation.status == DelegationStatus::Revoked {
            panic!("cannot update metadata of a revoked delegation");
        }

        // Check expiry
        if let Some(exp) = delegation.expires_at {
            if exp <= now {
                panic!("cannot update metadata of an expired delegation");
            }
        }

        delegation.metadata = metadata;
        delegation.version = delegation.version.checked_add(1).expect("version overflow");
        delegation.updated_at = now;

        env.storage().persistent().set(&key, &delegation);
        bump_ttl(&env, &key);

        // Emit Event
        env.events().publish(
            (
                Symbol::new(&env, "delegation_updated"),
                delegation_id,
                delegation.owner.clone(),
                delegation.delegate.clone(),
            ),
            (now, delegation.version),
        );
    }

    /// Explicitly trigger expiry event and update status to Expired if delegation is found to be expired
    pub fn check_and_expire(env: Env, delegation_id: u64) -> bool {
        let key = DataKey::Delegation(delegation_id);
        if let Some(mut delegation) = env.storage().persistent().get::<_, Delegation>(&key) {
            let now = env.ledger().timestamp();
            if let Some(exp) = delegation.expires_at {
                if exp <= now 
                    && delegation.status != DelegationStatus::Revoked 
                    && delegation.status != DelegationStatus::Expired 
                {
                    delegation.status = DelegationStatus::Expired;
                    delegation.version = delegation.version.checked_add(1).expect("version overflow");
                    delegation.updated_at = now;
                    env.storage().persistent().set(&key, &delegation);
                    bump_ttl(&env, &key);

                    // Remove from active registry mapping
                    let active_key = DataKey::ActiveDelegation(delegation.owner.clone(), delegation.delegate.clone());
                    if let Some(existing_id) = env.storage().persistent().get::<_, u64>(&active_key) {
                        if existing_id == delegation_id {
                            env.storage().persistent().remove(&active_key);
                        }
                    }

                    // Emit expired event
                    env.events().publish(
                        (
                            Symbol::new(&env, "delegation_expired"),
                            delegation_id,
                            delegation.owner.clone(),
                            delegation.delegate.clone(),
                        ),
                        (now, delegation.version),
                    );
                    return true;
                }
            }
            bump_ttl(&env, &key);
        }
        false
    }

    // --- Read Functions & Validation APIs ---

    pub fn get_delegation(env: Env, delegation_id: u64) -> Option<Delegation> {
        let key = DataKey::Delegation(delegation_id);
        let val = env.storage().persistent().get::<_, Delegation>(&key);
        if val.is_some() {
            bump_ttl(&env, &key);
        }
        val
    }

    pub fn delegation_exists(env: Env, delegation_id: u64) -> bool {
        let key = DataKey::Delegation(delegation_id);
        let has = env.storage().persistent().has(&key);
        if has {
            bump_ttl(&env, &key);
        }
        has
    }

    pub fn is_active_delegation(env: Env, delegation_id: u64) -> bool {
        Self::ensure_not_expired(&env, delegation_id);
        if let Some(delegation) = Self::get_delegation(env.clone(), delegation_id) {
            let now = env.ledger().timestamp();
            let is_expired = delegation.status == DelegationStatus::Expired 
                || (delegation.expires_at.is_some() && delegation.expires_at.unwrap() <= now);
            delegation.status == DelegationStatus::Active && !is_expired
        } else {
            false
        }
    }

    pub fn is_paused(env: Env, delegation_id: u64) -> bool {
        if let Some(delegation) = Self::get_delegation(env.clone(), delegation_id) {
            delegation.status == DelegationStatus::Paused
        } else {
            false
        }
    }

    pub fn is_revoked(env: Env, delegation_id: u64) -> bool {
        if let Some(delegation) = Self::get_delegation(env.clone(), delegation_id) {
            delegation.status == DelegationStatus::Revoked
        } else {
            false
        }
    }

    pub fn is_expired(env: Env, delegation_id: u64) -> bool {
        if let Some(delegation) = Self::get_delegation(env.clone(), delegation_id) {
            let now = env.ledger().timestamp();
            delegation.status == DelegationStatus::Expired 
                || (delegation.expires_at.is_some() && delegation.expires_at.unwrap() <= now)
        } else {
            false
        }
    }

    pub fn get_owner(env: Env, delegation_id: u64) -> Option<Address> {
        Self::get_delegation(env, delegation_id).map(|d| d.owner)
    }

    pub fn get_delegate(env: Env, delegation_id: u64) -> Option<Address> {
        Self::get_delegation(env, delegation_id).map(|d| d.delegate)
    }

    pub fn get_delegate_status(env: Env, delegation_id: u64) -> Option<DelegationStatus> {
        Self::get_delegation(env, delegation_id).map(|d| d.status)
    }

    pub fn get_active_delegation_id(env: Env, owner: Address, delegate: Address) -> Option<u64> {
        let active_key = DataKey::ActiveDelegation(owner, delegate);
        if let Some(id) = env.storage().persistent().get::<_, u64>(&active_key) {
            bump_ttl(&env, &active_key);
            // Ensure we never return an expired delegation
            Self::ensure_not_expired(&env, id);
            if let Some(delegation) = Self::get_delegation(env.clone(), id) {
                if delegation.status == DelegationStatus::Expired {
                    return None;
                }
                let now = env.ledger().timestamp();
                let is_exp = delegation.status == DelegationStatus::Expired 
                    || (delegation.expires_at.is_some() && delegation.expires_at.unwrap() <= now);
                if is_exp {
                    env.storage().persistent().remove(&active_key);
                    return None;
                }
            }
            Some(id)
        } else {
            None
        }
    }

    pub fn verify_authority(env: Env, owner: Address, delegate: Address) -> bool {
        if let Some(id) = Self::get_active_delegation_id(env.clone(), owner.clone(), delegate.clone()) {
            Self::ensure_not_expired(&env, id);
            Self::is_active_delegation(env, id)
        } else {
            false
        }
    }

    pub fn get_active_delegation(env: Env, owner: Address, delegate: Address) -> Option<Delegation> {
        let id = Self::get_active_delegation_id(env.clone(), owner.clone(), delegate.clone())?;
        Self::ensure_not_expired(&env, id);
        let delegation = Self::get_delegation(env.clone(), id)?;
        let now = env.ledger().timestamp();
        let is_expired = delegation.status == DelegationStatus::Expired 
            || (delegation.expires_at.is_some() && delegation.expires_at.unwrap() <= now);
        if delegation.status == DelegationStatus::Active && !is_expired {
            Some(delegation)
        } else {
            None
        }
    }

    pub fn get_delegation_by_pair(env: Env, owner: Address, delegate: Address) -> Option<Delegation> {
        let id = Self::get_active_delegation_id(env.clone(), owner, delegate)?;
        Self::ensure_not_expired(&env, id);
        Self::get_delegation(env, id)
    }

    pub fn is_delegation_valid(env: Env, delegation_id: u64) -> bool {
        Self::is_active_delegation(env, delegation_id)
    }

    pub fn get_delegations(env: Env, ids: Vec<u64>) -> Vec<Option<Delegation>> {
        let mut result = Vec::new(&env);
        for id in ids {
            result.push_back(Self::get_delegation(env.clone(), id));
        }
        result
    }

    // --- Administrative Hooks & Helpers ---

    /// Emergency Pause allows the owner to immediately pause a delegation.
    pub fn emergency_pause(env: Env, delegation_id: u64) {
        Self::pause_delegation(env, delegation_id);
    }
}

#[cfg(test)]
mod test;

