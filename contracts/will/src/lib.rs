use soroban_sdk::{
    contract, contractimpl, contracttype, token, Address, Env, String, Vec,
};

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
            panic!("keeper_bounty_bps exceeds maximum of {}", MAX_KEEPER_BOUNTY_BPS);
        }

        let mut total_share: u32 = 0;
        for beneficiary in beneficiaries.iter() {
            total_share += beneficiary.share_bps;
        }
        if total_share != 10000 {
            panic!("beneficiary shares must sum to 10000 bps (100%)");
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

        let invoker = env.invoker();
        let mut keeper_bounty_paid: i128 = 0;

        if will.keeper_bounty_bps > 0 && invoker != will.owner {
            let token_client = token::Client::new(&env, &token_address);
            let balance = token_client.balance(&env.current_contract_address());
            keeper_bounty_paid = (balance * will.keeper_bounty_bps as i128) / 10000;
            if keeper_bounty_paid > 0 {
                token_client.transfer(&env.current_contract_address(), &invoker, &keeper_bounty_paid);
            }
        }

        env.events().publish(
            (String::from_str(&env, "will_triggered"),),
            WillTriggeredEvent {
                owner: will.owner.clone(),
                triggered_by: invoker,
                release_time: will.release_time,
                keeper_bounty_paid,
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
        let invoker = env.invoker();
        let mut keeper_bounty_paid: i128 = 0;

        if will.keeper_bounty_bps > 0 && invoker != will.owner {
            keeper_bounty_paid = (balance * will.keeper_bounty_bps as i128) / 10000;
            if keeper_bounty_paid > 0 {
                token_client.transfer(&env.current_contract_address(), &invoker, &keeper_bounty_paid);
            }
        }

        let remaining_balance = balance - keeper_bounty_paid;

        for beneficiary in will.beneficiaries.iter() {
            let amount = (remaining_balance * beneficiary.share_bps as i128) / 10000;
            if amount > 0 {
                token_client.transfer(&env.current_contract_address(), &beneficiary.address, &amount);
            }
        }

        env.events().publish(
            (String::from_str(&env, "inheritance_released"),),
            InheritanceReleasedEvent {
                owner: will.owner.clone(),
                released_by: invoker,
                total_amount: balance,
                keeper_bounty_paid,
            },
        );

        env.storage().instance().remove(&DataKey::Will);
    }

    pub fn get_will(env: Env) -> Will {
        env.storage().instance().get(&DataKey::Will).unwrap()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{testutils::Address as _, token, Address, Env};

    fn create_token_contract<'a>(env: &Env, admin: &Address) -> (token::Client<'a>, token::StellarAssetClient<'a>) {
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

        let beneficiary_balance = token_client.balance(&beneficiary);
        assert_eq!(beneficiary_balance, 9900);
    }

    #[test]
    fn test_zero_bounty_default() {
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

        client.create_will(&owner, &beneficiaries, &100, &0);

        env.ledger().with_mut(|li| li.timestamp = 200);
        client.trigger_will(&token_client.address);

        env.ledger().with_mut(|li| li.timestamp = 200 + 604800);
        client.release_inheritance(&token_client.address);

        let keeper_balance = token_client.balance(&keeper);
        assert_eq!(keeper_balance, 0);

        let beneficiary_balance = token_client.balance(&beneficiary);
        assert_eq!(beneficiary_balance, 10000);
    }

    #[test]
    #[should_panic(expected = "keeper_bounty_bps exceeds maximum of 100")]
    fn test_bounty_cap_enforced() {
        let env = Env::default();
        env.mock_all_auths();

        let owner = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let contract_id = env.register_contract(None, WillContract);
        let client = WillContractClient::new(&env, &contract_id);

        let beneficiaries = Vec::from_array(
            &env,
            [Beneficiary {
                address: beneficiary.clone(),
                share_bps: 10000,
            }],
        );

        client.create_will(&owner, &beneficiaries, &100, &101);
    }

    #[test]
    fn test_no_bounty_when_owner_triggers() {
        let env = Env::default();
        env.mock_all_auths();

        let owner = Address::generate(&env);
        let beneficiary = Address::generate(&env);
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
        client.trigger_will(&token_client.address);

        let owner_balance = token_client.balance(&owner);
        assert_eq!(owner_balance, 0);

        let contract_balance = token_client.balance(&contract_id);
        assert_eq!(contract_balance, 10000);
    }
}
