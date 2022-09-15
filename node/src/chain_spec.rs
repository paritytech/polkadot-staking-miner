use std::str::FromStr;

use node_template_runtime::{
	opaque::SessionKeys, AccountId, AuraConfig, Balance, BalancesConfig, GenesisConfig,
	GrandpaConfig, MaxNominations, SessionConfig, Signature, StakingConfig, SudoConfig,
	SystemConfig, WASM_BINARY,
};
use pallet_staking::StakerStatus;
use rand::{distributions::Alphanumeric, rngs::OsRng, seq::SliceRandom, Rng};
use sc_service::ChainType;
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_core::{sr25519, Pair, Public};
use sp_finality_grandpa::AuthorityId as GrandpaId;
use sp_runtime::traits::{IdentifyAccount, Verify};

lazy_static::lazy_static! {
	static ref NOMINATORS: u32 = std::env::var("N").unwrap().parse().unwrap();
	static ref CANDIDATES: u32 = std::env::var("C").unwrap().parse().unwrap();
	static ref VALIDATORS: u32 = std::env::var("V").unwrap().parse().unwrap();
	static ref NOMINATION_DEGREE: NominationDegree = NominationDegree::from_str(std::env::var("ND").unwrap().as_ref()).unwrap();
}

#[derive(Debug, Clone, Copy)]
pub enum NominationDegree {
	Partial,
	Full,
}

impl FromStr for NominationDegree {
	type Err = ();

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		Ok(match &s[..] {
			"partial" => Self::Partial,
			"full" => Self::Full,
			_ => panic!("wrong nomination-degree."),
		})
	}
}

/// Specialized `ChainSpec`. This is a specialization of the general Substrate ChainSpec type.
pub type ChainSpec = sc_service::GenericChainSpec<GenesisConfig>;

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

	Ok(ChainSpec::from_genesis(
		// Name
		"Development",
		// ID
		"dev",
		ChainType::Development,
		move || testnet_genesis(wasm_binary, true),
		vec![],
		None,
		None,
		None,
		None,
		None,
	))
}

fn session_keys(aura: AuraId, grandpa: GrandpaId) -> SessionKeys {
	SessionKeys { grandpa, aura }
}

/// Configure initial storage state for FRAME modules.
fn testnet_genesis(wasm_binary: &[u8], _enable_println: bool) -> GenesisConfig {
	let rand_str =
		|| -> String { OsRng.sample_iter(&Alphanumeric).take(32).map(char::from).collect() };

	let nominators: u32 = *NOMINATORS;
	let validators: u32 = *VALIDATORS;
	let candidates: u32 = *CANDIDATES;
	let nomination_degree: NominationDegree = *NOMINATION_DEGREE;

	let min_balance = node_template_runtime::voter_bags::EXISTENTIAL_WEIGHT as Balance;
	let stash_min: Balance = min_balance;
	let stash_max: Balance = **node_template_runtime::voter_bags::THRESHOLDS
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

	let initial_authorities = vec![authority_keys_from_seed("Alice")]
		.into_iter()
		.chain(
			(nominators..nominators + candidates)
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
			let count = match nomination_degree {
				NominationDegree::Full => (rng2.gen::<usize>() % limit).max(1),
				NominationDegree::Partial => limit,
			};

			let nominations = initial_authorities
				.as_slice()
				.choose_multiple(&mut rng2, count)
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

	GenesisConfig {
		system: SystemConfig { code: wasm_binary.to_vec() },
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
		grandpa: GrandpaConfig { authorities: vec![] },
		sudo: SudoConfig { key: Some(root_key) },
		transaction_payment: Default::default(),
	}
}
