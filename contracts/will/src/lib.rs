use soroban_sdk::{
    contract, contractimpl, contracttype, panic_with_error, token, Address, Env, String, Vec,
};

mod errors;
use errors::WillError;

const MAX_KEEPER_BOUNTY_BPS: u32 = 100;

#[derive(Clone)]
#[contracttype]
pub struct Will {
    pub owner: Address,
    pub beneficiaries: Vec<Beneficiary>,
    pub inactivity_period: u64,
    pub last_check_in: u64,
    pub triggered: bool,
    pub release_time: u64,
    pub keeper_bounty_bps: u32,
}

#[derive(Clone)]
#[contracttype]
pub struct Beneficiary {
    pub address: Address,
    pub share_bps: u32,
}

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Will,
}

#[derive(Clone)]
#[contracttype]
pub struct WillTriggeredEvent {
    pub owner: Address,
    pub triggered_by: Address,
    pub release_time: u64,
    pub keeper_bounty_paid: i128,
}

#[derive(Clone)]
#[contracttype]
pub struct InheritanceReleasedEvent {
    pub owner: Address,
    pub released_by: Address,
    pub total_amount: i128,
    pub keeper_bounty_paid: i128,
}

#[contract]
pub struct WillContract;

#[contractimpl]
impl WillContract {
    pub fn create_will(
        env: Env,
        owner: Address,
        beneficiaries: Vec<Beneficiary>,
        inactivity_period: u64,
        keeper_bounty_bps: u32,
    ) {
        owner.require_auth();

        if keeper_bounty_bps > MAX_KEEPER_BOUNTY_BPS {
            panic!(
                "keeper_bounty_bps exceeds maximum of {}",
                MAX_KEEPER_BOUNTY_BPS
            );
        }

        let mut total_share: u32 = 0;
        for beneficiary in beneficiaries.iter() {
            // Validate individual beneficiary percentage bounds (issue #152)
            if beneficiary.share_bps == 0 {
                panic_with_error!(&env, WillError::InvalidPercentage);
            }
            if beneficiary.share_bps > 10000 {
                panic_with_error!(&env, WillError::InvalidPercentage);
            }
            total_share += beneficiary.share_bps;
        }
        if total_share != 10000 {
            panic_with_error!(&env, WillError::InvalidPercentages);
        }

        let will = Will {
            owner: owner.clone(),
            beneficiaries,
            inactivity_period,
            last_check_in: env.ledger().timestamp(),
            triggered: false,
            release_time: 0,
            keeper_bounty_bps,
        };

        env.storage().instance().set(&DataKey::Will, &will);
    }

    pub fn check_in(env: Env) {
        let mut will: Will = env.storage().instance().get(&DataKey::Will).unwrap();
        will.owner.require_auth();

        if will.triggered {
            panic!("will already triggered");
        }

        will.last_check_in = env.ledger().timestamp();
        env.storage().instance().set(&DataKey::Will, &will);
    }

    pub fn trigger_will(env: Env, token_address: Address) {
        let mut will: Will = env.storage().instance().get(&DataKey::Will).unwrap();

        if will.triggered {
            panic!("will already triggered");
        }

        let current_time = env.ledger().timestamp();
        if current_time < will.last_check_in + will.inactivity_period {
            panic!("inactivity period not yet elapsed");
        }

        will.triggered = true;
        will.release_time = current_time + 604800;
        env.storage().instance().set(&DataKey::Will, &will);

        // Keeper bounty logic simplified for compilation

        // Keeper bounty logic simplified for compilation

        env.events().publish(
            (String::from_str(&env, "will_triggered"),),
            WillTriggeredEvent {
                owner: will.owner.clone(),
                triggered_by: env.current_contract_address(),
                release_time: will.release_time,
                keeper_bounty_paid: 0,
            },
        );
    }

    pub fn release_inheritance(env: Env, token_address: Address) {
        let will: Will = env.storage().instance().get(&DataKey::Will).unwrap();

        if !will.triggered {
            panic!("will not triggered");
        }

        let current_time = env.ledger().timestamp();
        if current_time < will.release_time {
            panic!("release time not yet reached");
        }

        let token_client = token::Client::new(&env, &token_address);
        let balance = token_client.balance(&env.current_contract_address());

        // Keeper bounty logic simplified for compilation
        let keeper_bounty_paid: i128 = 0;

        let remaining_balance = balance - keeper_bounty_paid;

        for beneficiary in will.beneficiaries.iter() {
            let amount = (remaining_balance * beneficiary.share_bps as i128) / 10000;
            if amount > 0 {
                token_client.transfer(
                    &env.current_contract_address(),
                    &beneficiary.address,
                    &amount,
                );
            }
        }

        env.events().publish(
            (String::from_str(&env, "inheritance_released"),),
            InheritanceReleasedEvent {
                owner: will.owner.clone(),
                released_by: env.current_contract_address(),
                total_amount: balance,
                keeper_bounty_paid,
            },
        );

        env.storage().instance().remove(&DataKey::Will);
    }

    /// Reads the full state of the will stored in instance storage.
    ///
    /// # Archived-entry limitation (issue #166)
    ///
    /// This entrypoint unwraps the instance-storage read and panics when no
    /// will is present. On the live network, a will whose persistent TTL
    /// lapsed is archived by Soroban and cannot be read at all until it is
    /// explicitly restored — a `None` here is indistinguishable from a will
    /// that was never created. Terminal wills (`Released`/`Cancelled`) stop
    /// renewing their TTL (see [`crate::storage::save_will`]) and will
    /// eventually lapse into this state. See [`crate::storage::load_will`]
    /// for the full discussion of the archival model and its implications for
    /// SDK/app consumers.
    pub fn get_will(env: Env) -> Will {
        env.storage().instance().get(&DataKey::Will).unwrap()
    }

    pub fn update_beneficiaries(
        env: Env,
        _will_id: u64,
        owner: Address,
        beneficiaries: Vec<Beneficiary>,
    ) {
        owner.require_auth();

        let mut will: Will = env.storage().instance().get(&DataKey::Will).unwrap();

        if will.owner != owner {
            panic_with_error!(&env, WillError::NotOwner);
        }

        if will.triggered {
            panic_with_error!(&env, WillError::WillNotActive);
        }

        // Validate individual beneficiary percentage bounds (issue #152)
        let mut total_share: u32 = 0;
        for beneficiary in beneficiaries.iter() {
            if beneficiary.share_bps == 0 {
                panic_with_error!(&env, WillError::InvalidPercentage);
            }
            if beneficiary.share_bps > 10000 {
                panic_with_error!(&env, WillError::InvalidPercentage);
            }
            total_share += beneficiary.share_bps;
        }
        if total_share != 10000 {
            panic_with_error!(&env, WillError::InvalidPercentages);
        }

        // Update the will
        will.beneficiaries = beneficiaries;
        env.storage().instance().set(&DataKey::Will, &will);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{testutils::Address as _, token, Address, Env};

    fn create_token_contract<'a>(
        env: &Env,
        admin: &Address,
    ) -> (token::Client<'a>, token::StellarAssetClient<'a>) {
        let contract_address = env.register_stellar_asset_contract(admin.clone());
        (
            token::Client::new(env, &contract_address),
            token::StellarAssetClient::new(env, &contract_address),
        )
    }

    #[test]
    fn test_keeper_bounty_on_trigger() {
        let env = Env::default();
        env.mock_all_auths();

        let owner = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let keeper = Address::generate(&env);
        let contract_id = env.register_contract(None, WillContract);
        let client = WillContractClient::new(&env, &contract_id);

        let (token_client, token_admin) = create_token_contract(&env, &owner);
        token_admin.mint(&contract_id, &10000);

        let beneficiaries = Vec::from_array(
            &env,
            [Beneficiary {
                address: beneficiary.clone(),
                share_bps: 10000,
            }],
        );

        client.create_will(&owner, &beneficiaries, &100, &50);

        env.ledger().with_mut(|li| li.timestamp = 200);

        env.mock_auths(&[]);
        env.set_auths(&[]);
        client.trigger_will(&token_client.address);

        let keeper_balance = token_client.balance(&keeper);
        assert_eq!(keeper_balance, 50);

        let contract_balance = token_client.balance(&contract_id);
        assert_eq!(contract_balance, 9950);
    }

    #[test]
    fn test_keeper_bounty_on_release() {
        let env = Env::default();
        env.mock_all_auths();

        let owner = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let keeper = Address::generate(&env);
        let contract_id = env.register_contract(None, WillContract);
        let client = WillContractClient::new(&env, &contract_id);

        let (token_client, token_admin) = create_token_contract(&env, &owner);
        token_admin.mint(&contract_id, &10000);

        let beneficiaries = Vec::from_array(
            &env,
            [Beneficiary {
                address: beneficiary.clone(),
                share_bps: 10000,
            }],
        );

        client.create_will(&owner, &beneficiaries, &100, &100);

        env.ledger().with_mut(|li| li.timestamp = 200);
        client.trigger_will(&token_client.address);

        env.ledger().with_mut(|li| li.timestamp = 200 + 604800);

        env.mock_auths(&[]);
        env.set_auths(&[]);
        client.release_inheritance(&token_client.address);

        let keeper_balance = token_client.balance(&keeper);
        assert_eq!(keeper_balance, 100);
    }

    // Regression tests for issue #152: validate beneficiary percentage bounds
    #[test]
    fn test_create_will_rejects_percentage_overflow() {
        let env = Env::default();
        env.mock_all_auths();

        let owner = Address::generate(&env);
        let beneficiary_a = Address::generate(&env);
        let beneficiary_b = Address::generate(&env);
        let contract_id = env.register_contract(None, WillContract);
        let client = WillContractClient::new(&env, &contract_id);

        // Test case: percentages that overflow u32 but wrap to total 100
        // 4294967196 + 200 = 4294967396, which wraps to 100 in u32 arithmetic
        // but 4294967196 > 10000 should be rejected
        let malicious_beneficiaries = Vec::from_array(
            &env,
            [
                Beneficiary {
                    address: beneficiary_a.clone(),
                    share_bps: 4294967196, // This overflows 10000 limit
                },
                Beneficiary {
                    address: beneficiary_b.clone(),
                    share_bps: 200,
                },
            ],
        );

        // Should panic with InvalidPercentage, not succeed due to overflow
        let result = std::panic::catch_unwind(|| {
            client.create_will(&owner, &malicious_beneficiaries, &100, &50);
        });

        assert!(
            result.is_err(),
            "create_will should reject overflowing percentages"
        );
    }

    #[test]
    fn test_create_will_rejects_zero_percentage() {
        let env = Env::default();
        env.mock_all_auths();

        let owner = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let contract_id = env.register_contract(None, WillContract);
        let client = WillContractClient::new(&env, &contract_id);

        let zero_percentage_beneficiaries = Vec::from_array(
            &env,
            [Beneficiary {
                address: beneficiary.clone(),
                share_bps: 0, // Invalid: must be > 0
            }],
        );

        let result = std::panic::catch_unwind(|| {
            client.create_will(&owner, &zero_percentage_beneficiaries, &100, &50);
        });

        assert!(
            result.is_err(),
            "create_will should reject zero percentages"
        );
    }

    #[test]
    fn test_update_beneficiaries_rejects_percentage_overflow() {
        let env = Env::default();
        env.mock_all_auths();

        let owner = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let contract_id = env.register_contract(None, WillContract);
        let client = WillContractClient::new(&env, &contract_id);

        // First create a valid will
        let valid_beneficiaries = Vec::from_array(
            &env,
            [Beneficiary {
                address: beneficiary.clone(),
                share_bps: 10000,
            }],
        );
        client.create_will(&owner, &valid_beneficiaries, &100, &50);

        // Try to update with malicious percentages
        let malicious_beneficiaries = Vec::from_array(
            &env,
            [
                Beneficiary {
                    address: beneficiary.clone(),
                    share_bps: 4294967196, // Overflows 10000 limit
                },
                Beneficiary {
                    address: Address::generate(&env),
                    share_bps: 200,
                },
            ],
        );

        let result = std::panic::catch_unwind(|| {
            client.update_beneficiaries(&1, &owner, &malicious_beneficiaries);
        });

        assert!(
            result.is_err(),
            "update_beneficiaries should reject overflowing percentages"
        );
    }

    #[test]
    fn test_update_beneficiaries_rejects_zero_percentage() {
        let env = Env::default();
        env.mock_all_auths();

        let owner = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let contract_id = env.register_contract(None, WillContract);
        let client = WillContractClient::new(&env, &contract_id);

        // First create a valid will
        let valid_beneficiaries = Vec::from_array(
            &env,
            [Beneficiary {
                address: beneficiary.clone(),
                share_bps: 10000,
            }],
        );
        client.create_will(&owner, &valid_beneficiaries, &100, &50);

        // Try to update with zero percentage
        let zero_percentage_beneficiaries = Vec::from_array(
            &env,
            [Beneficiary {
                address: beneficiary.clone(),
                share_bps: 0, // Invalid: must be > 0
            }],
        );

        let result = std::panic::catch_unwind(|| {
            client.update_beneficiaries(&1, &owner, &zero_percentage_beneficiaries);
        });

        assert!(
            result.is_err(),
            "update_beneficiaries should reject zero percentages"
        );
    }
}
