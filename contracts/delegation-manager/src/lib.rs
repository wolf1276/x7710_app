#![no_std]
use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short, Address, Env, String, Vec,
};

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub enum DelegationStatus {
    Active,
    Paused,
    Revoked,
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
pub struct OwnerStats {
    pub total_created: u32,
    pub active_count: u32,
    pub paused_count: u32,
    pub revoked_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct DelegateStats {
    pub active_received: u32,
    pub total_received: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub enum DataKey {
    Counter,
    Delegation(u64),
    ActiveDelegation(Address, Address),
    OwnerDelegations(Address),
    DelegateDelegations(Address),
    OwnerStats(Address),
    DelegateStats(Address),
}

#[contract]
pub struct DelegationManager;

#[contractimpl]
impl DelegationManager {
    /// Creates a delegation for a delegate.
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
                // Check if existing delegation is still active and not expired
                let is_expired = existing.expires_at.is_some() && existing.expires_at.unwrap() <= now;
                if (existing.status == DelegationStatus::Active || existing.status == DelegationStatus::Paused)
                    && !is_expired
                {
                    panic!("duplicate active/paused delegation not allowed");
                }
            }
        }

        let mut counter = env
            .storage()
            .persistent()
            .get::<_, u64>(&DataKey::Counter)
            .unwrap_or(0);
        counter += 1;
        env.storage().persistent().set(&DataKey::Counter, &counter);

        let delegation_id = counter;

        let delegation = Delegation {
            id: delegation_id,
            owner: owner.clone(),
            delegate: delegate.clone(),
            status: DelegationStatus::Active,
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
        env.storage()
            .persistent()
            .set(&DataKey::Delegation(delegation_id), &delegation);

        // Update active delegation registry
        env.storage().persistent().set(&active_key, &delegation_id);

        // Update Owner list (for scanning/indexing)
        let owner_key = DataKey::OwnerDelegations(owner.clone());
        let mut owner_list = env
            .storage()
            .persistent()
            .get::<_, Vec<u64>>(&owner_key)
            .unwrap_or_else(|| Vec::new(&env));
        owner_list.push_back(delegation_id);
        env.storage().persistent().set(&owner_key, &owner_list);

        // Update Delegate list
        let delegate_key = DataKey::DelegateDelegations(delegate.clone());
        let mut delegate_list = env
            .storage()
            .persistent()
            .get::<_, Vec<u64>>(&delegate_key)
            .unwrap_or_else(|| Vec::new(&env));
        delegate_list.push_back(delegation_id);
        env.storage().persistent().set(&delegate_key, &delegate_list);

        // Update statistics
        let owner_stats_key = DataKey::OwnerStats(owner.clone());
        let mut owner_stats = env
            .storage()
            .persistent()
            .get::<_, OwnerStats>(&owner_stats_key)
            .unwrap_or(OwnerStats {
                total_created: 0,
                active_count: 0,
                paused_count: 0,
                revoked_count: 0,
            });
        owner_stats.total_created += 1;
        owner_stats.active_count += 1;
        env.storage().persistent().set(&owner_stats_key, &owner_stats);

        let delegate_stats_key = DataKey::DelegateStats(delegate.clone());
        let mut delegate_stats = env
            .storage()
            .persistent()
            .get::<_, DelegateStats>(&delegate_stats_key)
            .unwrap_or(DelegateStats {
                active_received: 0,
                total_received: 0,
            });
        delegate_stats.total_received += 1;
        delegate_stats.active_received += 1;
        env.storage().persistent().set(&delegate_stats_key, &delegate_stats);

        // Emit Event V2
        env.events().publish(
            (
                symbol_short!("created"),
                delegation_id,
                owner.clone(),
                delegate.clone(),
            ),
            (now, 1u32),
        );

        delegation_id
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
        let was_paused = delegation.status == DelegationStatus::Paused;

        delegation.status = DelegationStatus::Revoked;
        delegation.version += 1;
        delegation.revoked_at = Some(now);
        delegation.updated_at = now;

        env.storage().persistent().set(&key, &delegation);

        // Remove from active registry mapping
        let active_key = DataKey::ActiveDelegation(delegation.owner.clone(), delegation.delegate.clone());
        if let Some(existing_id) = env.storage().persistent().get::<_, u64>(&active_key) {
            if existing_id == delegation_id {
                env.storage().persistent().remove(&active_key);
            }
        }

        // Update statistics
        let owner_stats_key = DataKey::OwnerStats(delegation.owner.clone());
        if let Some(mut owner_stats) = env.storage().persistent().get::<_, OwnerStats>(&owner_stats_key) {
            if was_paused {
                if owner_stats.paused_count > 0 {
                    owner_stats.paused_count -= 1;
                }
            } else {
                if owner_stats.active_count > 0 {
                    owner_stats.active_count -= 1;
                }
            }
            owner_stats.revoked_count += 1;
            env.storage().persistent().set(&owner_stats_key, &owner_stats);
        }

        let delegate_stats_key = DataKey::DelegateStats(delegation.delegate.clone());
        if let Some(mut delegate_stats) = env.storage().persistent().get::<_, DelegateStats>(&delegate_stats_key) {
            if !was_paused {
                if delegate_stats.active_received > 0 {
                    delegate_stats.active_received -= 1;
                }
            }
            env.storage().persistent().set(&delegate_stats_key, &delegate_stats);
        }

        // Emit Event V2
        env.events().publish(
            (
                symbol_short!("revoked"),
                delegation_id,
                delegation.owner.clone(),
                delegation.delegate.clone(),
            ),
            (now, delegation.version),
        );
    }

    /// Owner temporarily disables a delegation.
    pub fn pause_delegation(env: Env, delegation_id: u64) {
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
        delegation.version += 1;
        delegation.paused_at = Some(now);
        delegation.updated_at = now;

        env.storage().persistent().set(&key, &delegation);

        // Update statistics
        let owner_stats_key = DataKey::OwnerStats(delegation.owner.clone());
        if let Some(mut owner_stats) = env.storage().persistent().get::<_, OwnerStats>(&owner_stats_key) {
            if owner_stats.active_count > 0 {
                owner_stats.active_count -= 1;
            }
            owner_stats.paused_count += 1;
            env.storage().persistent().set(&owner_stats_key, &owner_stats);
        }

        let delegate_stats_key = DataKey::DelegateStats(delegation.delegate.clone());
        if let Some(mut delegate_stats) = env.storage().persistent().get::<_, DelegateStats>(&delegate_stats_key) {
            if delegate_stats.active_received > 0 {
                delegate_stats.active_received -= 1;
            }
            env.storage().persistent().set(&delegate_stats_key, &delegate_stats);
        }

        // Emit Event V2
        env.events().publish(
            (
                symbol_short!("paused"),
                delegation_id,
                delegation.owner.clone(),
                delegation.delegate.clone(),
            ),
            (now, delegation.version),
        );
    }

    /// Owner re-enables a paused delegation.
    pub fn resume_delegation(env: Env, delegation_id: u64) {
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
        delegation.version += 1;
        delegation.resumed_at = Some(now);
        delegation.updated_at = now;

        env.storage().persistent().set(&key, &delegation);

        // Update statistics
        let owner_stats_key = DataKey::OwnerStats(delegation.owner.clone());
        if let Some(mut owner_stats) = env.storage().persistent().get::<_, OwnerStats>(&owner_stats_key) {
            if owner_stats.paused_count > 0 {
                owner_stats.paused_count -= 1;
            }
            owner_stats.active_count += 1;
            env.storage().persistent().set(&owner_stats_key, &owner_stats);
        }

        let delegate_stats_key = DataKey::DelegateStats(delegation.delegate.clone());
        if let Some(mut delegate_stats) = env.storage().persistent().get::<_, DelegateStats>(&delegate_stats_key) {
            delegate_stats.active_received += 1;
            env.storage().persistent().set(&delegate_stats_key, &delegate_stats);
        }

        // Emit Event V2
        env.events().publish(
            (
                symbol_short!("resumed"),
                delegation_id,
                delegation.owner.clone(),
                delegation.delegate.clone(),
            ),
            (now, delegation.version),
        );
    }

    /// Owner updates delegation metadata.
    pub fn update_metadata(env: Env, delegation_id: u64, metadata: DelegationMetadata) {
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
        delegation.version += 1;
        delegation.updated_at = now;

        env.storage().persistent().set(&key, &delegation);

        // Emit Event V2
        env.events().publish(
            (
                symbol_short!("updated"),
                delegation_id,
                delegation.owner.clone(),
                delegation.delegate.clone(),
            ),
            (now, delegation.version),
        );
    }

    /// Explicitly trigger expiry event if delegation is found to be expired
    pub fn check_and_expire(env: Env, delegation_id: u64) -> bool {
        let key = DataKey::Delegation(delegation_id);
        if let Some(delegation) = env.storage().persistent().get::<_, Delegation>(&key) {
            let now = env.ledger().timestamp();
            if let Some(exp) = delegation.expires_at {
                if exp <= now && delegation.status != DelegationStatus::Revoked {
                    // Emit expired event once if we detect it
                    env.events().publish(
                        (
                            symbol_short!("expired"),
                            delegation_id,
                            delegation.owner.clone(),
                            delegation.delegate.clone(),
                        ),
                        (now, delegation.version),
                    );
                    return true;
                }
            }
        }
        false
    }

    // --- Read Functions & Validation APIs ---

    pub fn get_delegation(env: Env, delegation_id: u64) -> Option<Delegation> {
        env.storage()
            .persistent()
            .get::<_, Delegation>(&DataKey::Delegation(delegation_id))
    }

    pub fn get_owner_delegations(env: Env, owner: Address) -> Vec<Delegation> {
        let owner_key = DataKey::OwnerDelegations(owner);
        let list = env
            .storage()
            .persistent()
            .get::<_, Vec<u64>>(&owner_key)
            .unwrap_or_else(|| Vec::new(&env));

        let mut delegations = Vec::new(&env);
        for id in list.iter() {
            if let Some(delegation) = Self::get_delegation(env.clone(), id) {
                delegations.push_back(delegation);
            }
        }
        delegations
    }

    pub fn get_delegate_delegations(env: Env, delegate: Address) -> Vec<Delegation> {
        let delegate_key = DataKey::DelegateDelegations(delegate);
        let list = env
            .storage()
            .persistent()
            .get::<_, Vec<u64>>(&delegate_key)
            .unwrap_or_else(|| Vec::new(&env));

        let mut delegations = Vec::new(&env);
        for id in list.iter() {
            if let Some(delegation) = Self::get_delegation(env.clone(), id) {
                delegations.push_back(delegation);
            }
        }
        delegations
    }

    pub fn delegation_exists(env: Env, delegation_id: u64) -> bool {
        env.storage().persistent().has(&DataKey::Delegation(delegation_id))
    }

    pub fn is_active_delegation(env: Env, delegation_id: u64) -> bool {
        if let Some(delegation) = Self::get_delegation(env.clone(), delegation_id) {
            let now = env.ledger().timestamp();
            let is_expired = delegation.expires_at.is_some() && delegation.expires_at.unwrap() <= now;
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
            delegation.expires_at.is_some() && delegation.expires_at.unwrap() <= now
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

    pub fn get_owner_stats(env: Env, owner: Address) -> Option<OwnerStats> {
        env.storage()
            .persistent()
            .get::<_, OwnerStats>(&DataKey::OwnerStats(owner))
    }

    pub fn get_delegate_stats(env: Env, delegate: Address) -> Option<DelegateStats> {
        env.storage()
            .persistent()
            .get::<_, DelegateStats>(&DataKey::DelegateStats(delegate))
    }

    pub fn get_active_delegation_id(env: Env, owner: Address, delegate: Address) -> Option<u64> {
        let active_key = DataKey::ActiveDelegation(owner, delegate);
        env.storage().persistent().get::<_, u64>(&active_key)
    }

    // --- Administrative Hooks & Helpers ---

    /// Emergency Pause allows the owner to immediately pause a delegation.
    pub fn emergency_pause(env: Env, delegation_id: u64) {
        Self::pause_delegation(env, delegation_id);
    }
}

#[cfg(test)]
mod test;
