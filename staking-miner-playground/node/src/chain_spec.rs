use pallet_staking::StakerStatus;
use rand::{distributions::Alphanumeric, rngs::OsRng, seq::SliceRandom, Rng};
use runtime::{
	opaque::SessionKeys, AccountId, AuraConfig, Balance, BalancesConfig, GrandpaConfig,
	MaxNominations, RuntimeGenesisConfig, SessionConfig, Signature, StakingConfig, SudoConfig,
	SystemConfig, WASM_BINARY,
};
use sc_service::ChainType;
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_consensus_grandpa::AuthorityId as GrandpaId;
use sp_core::{sr25519, Pair, Public};
use sp_runtime::traits::{IdentifyAccount, Verify};

lazy_static::lazy_static! {
	// Ideally, we should test with N=22500, C=1500, V=300 which is the limit on polkadot.
	static ref NOMINATORS: u32 = std::env::var("N").unwrap_or("700".to_string()).parse().unwrap();
	static ref CANDIDATES: u32 = std::env::var("C").unwrap_or("200".to_string()).parse().unwrap();
	static ref VALIDATORS: u32 = std::env::var("V").unwrap_or("20".to_string()).parse().unwrap();
}

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
	let rand_str =
		|| -> String { OsRng.sample_iter(&Alphanumeric).take(32).map(char::from).collect() };

	let nominators: u32 = *NOMINATORS;
	let validators: u32 = *VALIDATORS;
	let candidates: u32 = *CANDIDATES;

	let min_balance = runtime::voter_bags::EXISTENTIAL_WEIGHT as Balance;
	let stash_min: Balance = min_balance;
	let stash_max: Balance = **runtime::voter_bags::THRESHOLDS
		.iter()
		.skip(100)
		.take(1)
		.collect::<Vec<_>>()
		.first()
		.unwrap() as u128;
	let endowment: Balance = stash_max * 2;

	println!(
		"nominators {:?} / validators {:?} / candidates {:?} / maxNomination {}.",
		nominators,
		validators,
		candidates,
		MaxNominations::get()
	);

	let initial_nominators = (0..nominators)
		.map(|_| rand_str())
		.map(|seed| get_account_id_from_seed::<sr25519::Public>(seed.as_str()))
		.collect::<Vec<_>>();

	let initial_authorities = [authority_keys_from_seed("Alice")]
		.into_iter()
		.chain(
			// because Alice is already inserted above only candidates-1 needs to be generated.
			(0..candidates - 1)
				.map(|_| rand_str())
				.map(|seed| authority_keys_from_seed(seed.as_str())),
		)
		.collect::<Vec<_>>();

	let root_key = authority_keys_from_seed("Alice").0;

	let endowed_accounts = initial_authorities
		.iter()
		.map(|x| x.0.clone())
		.chain(initial_nominators.iter().cloned())
		.collect::<Vec<_>>();

	let rng1 = rand::thread_rng();
	let mut rng2 = rand::thread_rng();
	let stakers = initial_authorities
		.iter()
		.map(|x| {
			(
				x.0.clone(),
				x.0.clone(),
				rng1.clone().gen_range(stash_min..=stash_max),
				StakerStatus::Validator,
			)
		})
		.chain(initial_nominators.iter().map(|x| {
			let limit = (MaxNominations::get() as usize).min(initial_authorities.len());

			let nominations = initial_authorities
				.as_slice()
				.choose_multiple(&mut rng2, limit)
				.into_iter()
				.map(|choice| choice.0.clone())
				.collect::<Vec<_>>();
			(
				x.clone(),
				x.clone(),
				rng2.gen_range(stash_min..=stash_max),
				StakerStatus::Nominator(nominations),
			)
		}))
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
			validator_count: validators,
			minimum_validator_count: validators / 2,
			..Default::default()
		},
		aura: AuraConfig { authorities: vec![] },
		grandpa: GrandpaConfig::default(),
		sudo: SudoConfig { key: Some(root_key) },
		transaction_payment: Default::default(),
	};

	serde_json::to_value(&genesis).expect("Valid ChainSpec; qed")
}
