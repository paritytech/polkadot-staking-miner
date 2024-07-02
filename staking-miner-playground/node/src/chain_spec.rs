use pallet_staking::StakerStatus;
use runtime::{
	opaque::SessionKeys, AccountId, AuraConfig, Balance, BalancesConfig, GrandpaConfig,
	RuntimeGenesisConfig, SessionConfig, Signature, StakingConfig, SudoConfig, SystemConfig,
	WASM_BINARY,
};
use sc_service::ChainType;
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_consensus_grandpa::AuthorityId as GrandpaId;
use sp_core::{sr25519, Pair, Public};
use sp_runtime::{
	traits::{IdentifyAccount, Verify},
	AccountId32,
};

// Ideally, we should test with N=22500, C=1500, V=300 by default
//
// https://github.com/paritytech/polkadot-staking-miner/issues/774
const NOMINATORS_LEN: u32 = 700;
const CANDIDATES_LEN: u32 = 200;
const VALIDATORS_LEN: u32 = 20;

/// Specialized `ChainSpec`. This is a specialization of the general Substrate ChainSpec type.
pub type ChainSpec = sc_service::GenericChainSpec<RuntimeGenesisConfig>;

/// Generate a crypto pair from seed.
pub fn get_from_seed<TPublic: Public>(seed: &str) -> <TPublic::Pair as Pair>::Public {
	TPublic::Pair::from_string(&format!("//{}", seed), None)
		.expect("static values are valid; qed")
		.public()
}

type AccountPublic = <Signature as Verify>::Signer;

/// Generate an account ID from seed.
pub fn get_account_id_from_seed<TPublic: Public>(seed: &str) -> AccountId
where
	AccountPublic: From<<TPublic::Pair as Pair>::Public>,
{
	AccountPublic::from(get_from_seed::<TPublic>(seed)).into_account()
}

/// Generate an Aura authority key.
pub fn authority_keys_from_seed(s: &str) -> (AccountId, AuraId, GrandpaId) {
	(
		// used as both stash and controller.
		get_account_id_from_seed::<sr25519::Public>(s),
		get_from_seed::<AuraId>(s),
		get_from_seed::<GrandpaId>(s),
	)
}

pub fn development_config() -> Result<ChainSpec, String> {
	let wasm_binary = WASM_BINARY.ok_or_else(|| "Development wasm not available".to_string())?;

	let chain_spec = ChainSpec::builder(wasm_binary, Default::default())
		.with_genesis_config_patch(testnet_genesis())
		.with_chain_type(ChainType::Development)
		.build();

	Ok(chain_spec)
}

fn session_keys(aura: AuraId, grandpa: GrandpaId) -> SessionKeys {
	SessionKeys { grandpa, aura }
}

/// Configure initial storage state for FRAME modules.
fn testnet_genesis() -> serde_json::Value {
	let stash_max: Balance = **runtime::voter_bags::THRESHOLDS
		.iter()
		.skip(100)
		.take(1)
		.collect::<Vec<_>>()
		.first()
		.unwrap() as u128;
	let endowment: Balance = stash_max * 2;

	let initial_nominators = nominators();
	let initial_authorities = authorities();
	let stakers = stakers();

	let root_key = authority_keys_from_seed("Alice").0;

	let endowed_accounts = initial_authorities
		.iter()
		.map(|x| x.0.clone())
		.chain(initial_nominators.iter().cloned())
		.collect::<Vec<_>>();

	let genesis = RuntimeGenesisConfig {
		system: SystemConfig::default(),
		balances: BalancesConfig {
			balances: endowed_accounts.iter().cloned().map(|k| (k, endowment)).collect(),
		},
		session: SessionConfig {
			keys: initial_authorities
				.iter()
				.map(|x| (x.0.clone(), x.0.clone(), session_keys(x.1.clone(), x.2.clone())))
				.collect::<Vec<_>>(),
		},
		staking: StakingConfig {
			stakers,
			validator_count: VALIDATORS_LEN,
			minimum_validator_count: VALIDATORS_LEN / 2,
			..Default::default()
		},
		aura: AuraConfig { authorities: vec![] },
		grandpa: GrandpaConfig::default(),
		sudo: SudoConfig { key: Some(root_key) },
		transaction_payment: Default::default(),
	};

	serde_json::to_value(&genesis).expect("Valid ChainSpec; qed")
}

fn stakers() -> Vec<(AccountId32, AccountId32, u128, StakerStatus<AccountId32>)> {
	let file = std::fs::File::open("stakers.json").expect("`stakers.json` not found");
	serde_json::from_reader(file).expect("Failed to parse `stakers.json`")
}

fn nominators() -> Vec<AccountId> {
	let file = std::fs::File::open("nominators.json").expect("`nominators.json` not found");
	let n: Vec<_> = serde_json::from_reader(file).expect("Failed to parse `nominators.json`");
	assert_eq!(n.len(), NOMINATORS_LEN as usize);
	n
}

fn authorities() -> Vec<(AccountId, AuraId, GrandpaId)> {
	let file = std::fs::File::open("authorities.json").expect("`authorities.json` not found");
	let a: Vec<_> = serde_json::from_reader(file).expect("Failed to parse `authorities.json`");
	assert_eq!(a.len(), CANDIDATES_LEN as usize);
	a
}
