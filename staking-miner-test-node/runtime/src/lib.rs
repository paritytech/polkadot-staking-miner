#![cfg_attr(not(feature = "std"), no_std)]
// `construct_runtime!` does a lot of recursion and requires us to increase the limit to 256.
#![recursion_limit = "256"]

// Make the WASM binary available.
#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

use election_multi_phase::SolutionAccuracyOf;
use frame_election_provider_support::{onchain, ElectionDataProvider, SequentialPhragmen};
use frame_support::{
	dispatch::{DispatchClass, PerDispatchClass, TransactionPriority},
	traits::{ConstU16, ConstU32, Get, U128CurrencyToVote},
};
use frame_system::{limits, EnsureRoot};
use opaque::SessionKeys;
use pallet_grandpa::{
	fg_primitives, AuthorityId as GrandpaId, AuthorityList as GrandpaAuthorityList,
};
use pallet_staking::SessionInterface;
use sp_api::impl_runtime_apis;
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_core::{crypto::KeyTypeId, OpaqueMetadata};
use sp_runtime::{
	create_runtime_str, generic, impl_opaque_keys,
	traits::{
		AccountIdLookup, BlakeTwo256, Block as BlockT, IdentifyAccount, NumberFor, OpaqueKeys,
		Verify,
	},
	transaction_validity::{TransactionSource, TransactionValidity},
	ApplyExtrinsicResult, MultiSignature,
};
use sp_staking::SessionIndex;
use sp_std::prelude::*;
#[cfg(feature = "std")]
use sp_version::NativeVersion;
use sp_version::RuntimeVersion;

// A few exports that help ease life for downstream crates.
pub use frame_support::{
	construct_runtime, parameter_types,
	traits::{KeyOwnerProofSystem, Randomness, StorageInfo},
	weights::{
		constants::{BlockExecutionWeight, ExtrinsicBaseWeight, RocksDbWeight, WEIGHT_PER_SECOND},
		IdentityFee, Weight,
	},
	StorageValue,
};
pub use pallet_balances::Call as BalancesCall;
use pallet_election_provider_multi_phase as election_multi_phase;
use pallet_session::{PeriodicSessions, ShouldEndSession};
pub use pallet_timestamp::Call as TimestampCall;
use pallet_transaction_payment::CurrencyAdapter;
#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;
pub use sp_runtime::{Perbill, Permill};

/// An index to a block.
pub type BlockNumber = u32;

/// Alias to 512-bit hash when used in the context of a transaction signature on the chain.
pub type Signature = MultiSignature;

/// Some way of identifying an account on the chain. We intentionally make it equivalent
/// to the public key of our transaction signing scheme.
pub type AccountId = <<Signature as Verify>::Signer as IdentifyAccount>::AccountId;

/// Balance of an account.
pub type Balance = u128;

/// Index of a transaction in the chain.
pub type Index = u32;

/// A hash of some data used by the chain.
pub type Hash = sp_core::H256;

pub type Moment = u64;

pub const DOLLARS: Balance = 100_000_000_000_000;
pub const CENTS: Balance = DOLLARS / 100;

/// Opaque types. These are used by the CLI to instantiate machinery that don't need to know
/// the specifics of the runtime. They can then be made to be agnostic over specific formats
/// of data like extrinsics, allowing for them to continue syncing the network through upgrades
/// to even the core data structures.
pub mod opaque {
	use super::*;

	pub use sp_runtime::OpaqueExtrinsic as UncheckedExtrinsic;

	/// Opaque block header type.
	pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
	/// Opaque block type.
	pub type Block = generic::Block<Header, UncheckedExtrinsic>;
	/// Opaque block identifier type.
	pub type BlockId = generic::BlockId<Block>;

	impl_opaque_keys! {
		pub struct SessionKeys {
			pub aura: Aura,
			pub grandpa: Grandpa,
		}
	}
}

// To learn more about runtime versioning and what each of the following value means:
//   https://docs.substrate.io/v3/runtime/upgrades#runtime-versioning
#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: create_runtime_str!("polkadot"),
	impl_name: create_runtime_str!("polkadot"),
	authoring_version: 1,
	// The version of the runtime specification. A full node will not attempt to use its native
	//   runtime in substitute for the on-chain Wasm runtime unless all of `spec_name`,
	//   `spec_version`, and `authoring_version` are the same between Wasm and native.
	// This value is set to 100 to notify Polkadot-JS App (https://polkadot.js.org/apps) to use
	//   the compatible custom types.
	spec_version: 100,
	impl_version: 1,
	apis: RUNTIME_API_VERSIONS,
	transaction_version: 1,
	state_version: 1,
};

pub const MILLISECS_PER_BLOCK: u64 = 6000;
pub const SLOT_DURATION: u64 = MILLISECS_PER_BLOCK;

pub const MINUTES: BlockNumber = 60_000 / (MILLISECS_PER_BLOCK as BlockNumber);
pub const HOURS: BlockNumber = MINUTES * 60;
pub const DAYS: BlockNumber = HOURS * 24;

/// The version information used to identify this runtime when compiled natively.
#[cfg(feature = "std")]
pub fn native_version() -> NativeVersion {
	NativeVersion { runtime_version: VERSION, can_author_with: Default::default() }
}

sp_api::decl_runtime_apis! {
	pub trait TestConfigApi {
		fn set_length(len: u32);
		fn set_weight(weight: u64);
	}
}

pub mod block_length {
	use super::*;
	use frame_support::traits::Get;
	use frame_system::limits;

	const BLOCK_LENGTH_KEY: &[u8] = b":test_length:";
	fn get() -> u32 {
		frame_support::storage::unhashed::get(BLOCK_LENGTH_KEY).unwrap_or(4 * 1024)
	}
	pub fn set(len: u32) {
		frame_support::storage::unhashed::put(BLOCK_LENGTH_KEY, &len);
	}

	pub struct ConfigurableBlockLength;
	impl Get<limits::BlockLength> for ConfigurableBlockLength {
		fn get() -> limits::BlockLength {
			limits::BlockLength { max: PerDispatchClass::new(|_| get()) }
		}
	}
}

pub mod block_weight {
	use super::*;
	use frame_system::limits;

	const BLOCK_WEIGHT_KEY: &[u8] = b":test_weight:";
	fn get() -> u64 {
		frame_support::storage::unhashed::get(BLOCK_WEIGHT_KEY)
			.unwrap_or((WEIGHT_PER_SECOND / 100).ref_time())
	}
	pub fn set(len: u64) {
		frame_support::storage::unhashed::put(BLOCK_WEIGHT_KEY, &len);
	}

	pub struct ConfigurableBlockWeight;
	impl Get<limits::BlockWeights> for ConfigurableBlockWeight {
		fn get() -> limits::BlockWeights {
			limits::BlockWeights::simple_max(Weight::from_parts(get(), u64::MAX))
		}
	}
}

parameter_types! {
	pub const Version: RuntimeVersion = VERSION;
	pub const BlockHashCount: BlockNumber = 2400;
	pub const SS58Prefix: u8 = 42;
}

impl frame_system::Config for Runtime {
	type BaseCallFilter = frame_support::traits::Everything;
	type BlockWeights = block_weight::ConfigurableBlockWeight;
	type BlockLength = block_length::ConfigurableBlockLength;
	type AccountId = AccountId;
	type RuntimeCall = RuntimeCall;
	type RuntimeOrigin = RuntimeOrigin;
	type Lookup = AccountIdLookup<AccountId, ()>;
	type Index = Index;
	type BlockNumber = BlockNumber;
	type Hash = Hash;
	type Hashing = BlakeTwo256;
	type Header = generic::Header<BlockNumber, BlakeTwo256>;
	type RuntimeEvent = RuntimeEvent;
	type BlockHashCount = BlockHashCount;
	type DbWeight = RocksDbWeight;
	type Version = Version;
	type PalletInfo = PalletInfo;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type AccountData = pallet_balances::AccountData<Balance>;
	type SystemWeightInfo = ();
	type SS58Prefix = SS58Prefix;
	type OnSetCode = ();
	type MaxConsumers = ConstU32<100>;
}

impl pallet_randomness_collective_flip::Config for Runtime {}

impl pallet_aura::Config for Runtime {
	type AuthorityId = AuraId;
	type DisabledValidators = ();
	type MaxAuthorities = MaxAuthorities;
}

parameter_types! {
	pub const MaxAuthorities: u32 = 100_000;
}

impl pallet_grandpa::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type KeyOwnerProofSystem = ();
	type KeyOwnerProof =
		<Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(KeyTypeId, GrandpaId)>>::Proof;
	type KeyOwnerIdentification = <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(
		KeyTypeId,
		GrandpaId,
	)>>::IdentificationTuple;
	type HandleEquivocation = ();
	type WeightInfo = ();
	type MaxAuthorities = MaxAuthorities;
}

parameter_types! {
	pub const MinimumPeriod: u64 = SLOT_DURATION / 2;
}

impl pallet_timestamp::Config for Runtime {
	/// A timestamp: milliseconds since the unix epoch.
	type Moment = u64;
	type OnTimestampSet = Aura;
	type MinimumPeriod = MinimumPeriod;
	type WeightInfo = ();
}

parameter_types! {
	pub const ExistentialDeposit: u128 = 500;
	pub const MaxLocks: u32 = 50;
}

impl pallet_balances::Config for Runtime {
	type MaxLocks = MaxLocks;
	type MaxReserves = ();
	type ReserveIdentifier = [u8; 8];
	type Balance = Balance;
	type RuntimeEvent = RuntimeEvent;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = pallet_balances::weights::SubstrateWeight<Runtime>;
}

parameter_types! {
	pub const TransactionByteFee: Balance = 1;
	pub OperationalFeeMultiplier: u8 = 5;
}

impl pallet_transaction_payment::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type OnChargeTransaction = CurrencyAdapter<Balances, ()>;
	type OperationalFeeMultiplier = OperationalFeeMultiplier;
	type WeightToFee = IdentityFee<Balance>;
	type LengthToFee = frame_support::weights::ConstantMultiplier<Balance, TransactionByteFee>;
	type FeeMultiplierUpdate = ();
}

impl pallet_sudo::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
}

pub(crate) mod sudo_key {
	use super::*;
	#[frame_support::storage_alias]
	pub(crate) type Key = StorageValue<Sudo, AccountId>;
}

pub struct SudoAsStakingSessionManager;
impl pallet_session::SessionManager<AccountId> for SudoAsStakingSessionManager {
	fn end_session(end_index: sp_staking::SessionIndex) {
		<Staking as pallet_session::SessionManager<AccountId>>::end_session(end_index)
	}

	fn new_session(new_index: sp_staking::SessionIndex) -> Option<Vec<AccountId>> {
		<Staking as pallet_session::SessionManager<AccountId>>::new_session(new_index).map(
			|validators| {
				if let Some(sudo) = validators.iter().find(|v| v == &&sudo_key::Key::get().unwrap())
				{
					frame_support::log::info!(target: "runtime", "overwriting all validators to sudo: {:?}", sudo);
				} else {
					frame_support::log::warn!(
						target: "runtime",
						"sudo is not even in the validator set {:?}",
						sudo_key::Key::get().unwrap()
					);
				}
				vec![sudo_key::Key::get().unwrap()]
			},
		)
	}

	fn new_session_genesis(new_index: sp_staking::SessionIndex) -> Option<Vec<AccountId>> {
		<Staking as pallet_session::SessionManager<AccountId>>::new_session_genesis(new_index).map(
			|validators| {
				if let Some(sudo) = validators.iter().find(|v| v == &&sudo_key::Key::get().unwrap())
				{
					frame_support::log::info!(target: "runtime", "overwriting all validators to sudo: {:?}", sudo);
				} else {
					frame_support::log::warn!(
						target: "runtime",
						"sudo is not even in the validator set {:?}",
						sudo_key::Key::get().unwrap()
					);
				}
				vec![sudo_key::Key::get().unwrap()]
			},
		)
	}

	fn start_session(start_index: sp_staking::SessionIndex) {
		<Staking as pallet_session::SessionManager<AccountId>>::start_session(start_index)
	}
}

fn get_last_election() -> BlockNumber {
	frame_support::storage::unhashed::get("last_election".as_bytes()).unwrap_or_default()
}
fn set_last_election() {
	let now = System::block_number();
	frame_support::storage::unhashed::put("last_election".as_bytes(), &now)
}

pub struct PeriodicSessionUntilSolutionQueued<const PERIOD: BlockNumber>;
impl<const PERIOD: BlockNumber> ShouldEndSession<BlockNumber>
	for PeriodicSessionUntilSolutionQueued<PERIOD>
{
	fn should_end_session(_: BlockNumber) -> bool {
		// can still be the normal periodic sessions.
		let now = System::block_number();
		let last_election = get_last_election();
		let will_change = ElectionProviderMultiPhase::queued_solution().is_some() ||
			(now - last_election) > PERIOD;
		if will_change {
			set_last_election()
		}
		will_change
	}
}

const SESSION: BlockNumber = 1 * MINUTES;

impl<const PERIOD: BlockNumber> frame_support::traits::EstimateNextSessionRotation<BlockNumber>
	for PeriodicSessionUntilSolutionQueued<PERIOD>
{
	fn average_session_length() -> BlockNumber {
		PERIOD
	}

	fn estimate_current_session_progress(_: BlockNumber) -> (Option<Permill>, Weight) {
		let now = System::block_number();
		let since = now - get_last_election();
		(Some(Permill::from_rational(since, PERIOD)), Weight::from_ref_time(0))
	}

	fn estimate_next_session_rotation(_: BlockNumber) -> (Option<BlockNumber>, Weight) {
		(Some(get_last_election() + PERIOD), Weight::from_ref_time(0))
	}
}

impl pallet_session::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type ValidatorId = <Self as frame_system::Config>::AccountId;
	type ValidatorIdOf = pallet_staking::StashOf<Self>;
	// type ShouldEndSession = PeriodicSessionUntilSolutionQueued<{ MINUTES * 10 }>;
	// type NextSessionRotation = PeriodicSessionUntilSolutionQueued<{ MINUTES * 10 }>;
	type ShouldEndSession = PeriodicSessions<ConstU32<{ SESSION }>, ()>;
	type NextSessionRotation = PeriodicSessions<ConstU32<{ SESSION }>, ()>;
	type SessionManager = SudoAsStakingSessionManager;
	type SessionHandler = <SessionKeys as OpaqueKeys>::KeyTypeIdProviders;
	type Keys = SessionKeys;
	type WeightInfo = pallet_session::weights::SubstrateWeight<Runtime>;
}

use sp_runtime::curve::PiecewiseLinear;
pallet_staking_reward_curve::build! {
	const REWARD_CURVE: PiecewiseLinear<'static> = curve!(
		min_inflation: 0_025_000,
		max_inflation: 0_100_000,
		ideal_stake: 0_500_000,
		falloff: 0_050_000,
		max_piece_count: 40,
		test_precision: 0_005_000,
	);
}

parameter_types! {
	pub const SessionsPerEra: sp_staking::SessionIndex = 1;
	pub const BondingDuration: sp_staking::EraIndex = 24 * 28;
	pub const SlashDeferDuration: sp_staking::EraIndex = 24 * 7; // 1/4 the bonding duration.
	pub const RewardCurve: &'static PiecewiseLinear<'static> = &REWARD_CURVE;
	pub const MaxNominatorRewardedPerValidator: u32 = 256;
	pub const OffendingValidatorsThreshold: Perbill = Perbill::from_percent(17);
	pub Lookahead: BlockNumber = 5u32.into();
	pub HistoryDepth: u32 = 84;
}
pub struct StakingBenchmarkingConfig;
impl pallet_staking::BenchmarkingConfig for StakingBenchmarkingConfig {
	type MaxNominators = ConstU32<1000>;
	type MaxValidators = ConstU32<1000>;
}

impl SessionInterface<AccountId> for Runtime {
	fn disable_validator(validator_index: u32) -> bool {
		<pallet_session::Pallet<Runtime>>::disable_index(validator_index)
	}

	fn validators() -> Vec<AccountId> {
		<pallet_session::Pallet<Runtime>>::validators()
	}

	fn prune_historical_up_to(_: SessionIndex) {
		unimplemented!("we don't give a damn about historical session data here.");
	}
}

impl pallet_staking::Config for Runtime {
	type MaxNominations = MaxNominations;
	type Currency = Balances;
	type UnixTime = Timestamp;
	type CurrencyToVote = U128CurrencyToVote;
	type CurrencyBalance = Balance;
	type OnStakerSlash = ();
	type MaxUnlockingChunks = ConstU32<16>;
	type RewardRemainder = ();
	type RuntimeEvent = RuntimeEvent;
	type Slash = ();
	type Reward = ();
	type SessionsPerEra = SessionsPerEra;
	type BondingDuration = BondingDuration;
	type SlashDeferDuration = SlashDeferDuration;
	/// A super-majority of the council can cancel the slash.
	type SlashCancelOrigin = EnsureRoot<AccountId>;
	type SessionInterface = Self;
	type EraPayout = pallet_staking::ConvertCurve<RewardCurve>;
	type NextNewSession = Session;
	type MaxNominatorRewardedPerValidator = MaxNominatorRewardedPerValidator;
	type OffendingValidatorsThreshold = OffendingValidatorsThreshold;
	type ElectionProvider = ElectionProviderMultiPhase;
	type GenesisElectionProvider = onchain::OnChainExecution<OnChainSeqPhragmen>;
	// Alternatively, use pallet_staking::UseNominatorsMap<Runtime> to just use the nominators map.
	// Note that the aforementioned does not scale to a very large number of nominators.
	type VoterList = BagsList;
	type WeightInfo = pallet_staking::weights::SubstrateWeight<Runtime>;
	type BenchmarkingConfig = StakingBenchmarkingConfig;
	type HistoryDepth = HistoryDepth;
	type TargetList = pallet_staking::UseValidatorsMap<Self>;
}

parameter_types! {
	pub const StakingUnsignedPriority: TransactionPriority = TransactionPriority::max_value() / 2;

	// signed config
	pub const SignedRewardBase: Balance = 1 * DOLLARS;
	pub const SignedDepositBase: Balance = 1 * DOLLARS;
	pub const SignedDepositByte: Balance = 1 * CENTS;

	// miner configs
	pub const ElectionUnsignedPriority: TransactionPriority = StakingUnsignedPriority::get() - 1u64;
	pub MinerMaxWeight: Weight = Perbill::from_rational(8u32, 10) *
		<Runtime as frame_system::Config>::BlockWeights::get()
		.get(DispatchClass::Normal)
		.max_total
		.unwrap();
	pub MinerMaxLength: u32 = Perbill::from_rational(8u32, 10) *
		*(<<Runtime as frame_system::Config>::BlockLength as Get<limits::BlockLength>>::get())
		.max
		.get(DispatchClass::Normal);
}

mod solution_16 {
	use super::*;

	frame_election_provider_support::generate_solution_type!(
	#[compact]
	pub struct NposSolution16::<
		VoterIndex = u32,
		TargetIndex = u16,
		Accuracy = sp_runtime::PerU16,
		MaxVoters = MaxElectingVoters,
		>(16)
	);
}
mod solution_24 {
	use super::*;

	frame_election_provider_support::generate_solution_type!(
	#[compact]
	pub struct NposSolution24::<
		VoterIndex = u32,
		TargetIndex = u16,
		Accuracy = sp_runtime::PerU16,
		MaxVoters = MaxElectingVoters,
		>(24)
	);
}

use solution_16::NposSolution16;
use solution_24::NposSolution24;
type FinalSolution = NposSolution16;

parameter_types! {
	pub MaxNominations: u32 = <
		FinalSolution
		as
		frame_election_provider_support::NposSolution
	>::LIMIT as u32;
	pub const MaxElectingVoters: u32 = 25_000;
	pub MaxOnChainElectingVoters: u32 = 5000;
	pub MaxOnChainElectableTargets: u16 = 1250;
	// The maximum winners that can be elected by the Election pallet which is equivalent to the
	// maximum active validators the staking pallet can have.
	pub MaxActiveValidators: u32 = 1000;

}

/// The numbers configured here could always be more than the the maximum limits of staking pallet
/// to ensure election snapshot will not run out of memory. For now, we set them to smaller values
/// since the staking is bounded and the weight pipeline takes hours for this single pallet.
pub struct ElectionProviderBenchmarkConfig;
impl election_multi_phase::BenchmarkingConfig for ElectionProviderBenchmarkConfig {
	const VOTERS: [u32; 2] = [1000, 2000];
	const TARGETS: [u32; 2] = [500, 1000];
	const ACTIVE_VOTERS: [u32; 2] = [500, 800];
	const DESIRED_TARGETS: [u32; 2] = [200, 400];
	const SNAPSHOT_MAXIMUM_VOTERS: u32 = 1000;
	const MINER_MAXIMUM_VOTERS: u32 = 1000;
	const MAXIMUM_TARGETS: u32 = 300;
}

pub struct OnChainSeqPhragmen;
impl onchain::Config for OnChainSeqPhragmen {
	type System = Runtime;
	type Solver = SequentialPhragmen<
		AccountId,
		pallet_election_provider_multi_phase::SolutionAccuracyOf<Runtime>,
	>;
	type DataProvider = <Runtime as pallet_election_provider_multi_phase::Config>::DataProvider;
	type WeightInfo = frame_election_provider_support::weights::SubstrateWeight<Runtime>;
	type MaxWinners = <Runtime as pallet_election_provider_multi_phase::Config>::MaxWinners;
	type VotersBound = MaxOnChainElectingVoters;
	type TargetsBound = MaxOnChainElectableTargets;
}

impl pallet_election_provider_multi_phase::MinerConfig for Runtime {
	type AccountId = AccountId;
	type MaxLength = MinerMaxLength;
	type MaxWeight = MinerMaxWeight;
	type Solution = NposSolution16;
	type MaxVotesPerVoter =
	<<Self as pallet_election_provider_multi_phase::Config>::DataProvider as ElectionDataProvider>::MaxVotesPerVoter;

	// The unsigned submissions have to respect the weight of the submit_unsigned call, thus their
	// weight estimate function is wired to this call's weight.
	fn solution_weight(v: u32, t: u32, a: u32, d: u32) -> Weight {
		<
			<Self as pallet_election_provider_multi_phase::Config>::WeightInfo
			as
			pallet_election_provider_multi_phase::WeightInfo
		>::submit_unsigned(v, t, a, d)
	}
}

impl<C> frame_system::offchain::SendTransactionTypes<C> for Runtime
where
	RuntimeCall: From<C>,
{
	type Extrinsic = UncheckedExtrinsic;
	type OverarchingCall = RuntimeCall;
}

pub struct IncPerRound<const S: u32, const I: u32>;
impl<const S: u32, const I: u32> frame_support::traits::Get<u32> for IncPerRound<S, I> {
	fn get() -> u32 {
		S + (ElectionProviderMultiPhase::round() * I)
	}
}

impl pallet_election_provider_multi_phase::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type EstimateCallFee = TransactionPayment;
	type MinerConfig = Self;
	type SignedMaxRefunds = ();
	type UnsignedPhase = ConstU32<{ SESSION / 2 }>;
	type SignedPhase = ConstU32<{ SESSION / 2 }>;
	type BetterUnsignedThreshold = ();
	type BetterSignedThreshold = ();
	type OffchainRepeat = ();
	type MinerTxPriority = ElectionUnsignedPriority;
	type SignedMaxSubmissions = ConstU32<10>;
	type SignedRewardBase = SignedRewardBase;
	type SignedDepositBase = SignedDepositBase;
	type SignedDepositByte = SignedDepositByte;
	type SignedDepositWeight = ();
	type SignedMaxWeight = MinerMaxWeight;
	type SlashHandler = (); // burn slashes
	type RewardHandler = (); // nothing to do upon rewards
	type DataProvider = Staking;
	type Fallback = onchain::OnChainExecution<OnChainSeqPhragmen>;
	type GovernanceFallback = onchain::OnChainExecution<OnChainSeqPhragmen>;
	type Solver = SequentialPhragmen<AccountId, SolutionAccuracyOf<Self>, ()>;
	type WeightInfo = pallet_election_provider_multi_phase::weights::SubstrateWeight<Self>;
	type ForceOrigin = EnsureRoot<Self::AccountId>;
	type MaxElectableTargets = ConstU16<{ u16::MAX }>;
	type MaxElectingVoters = MaxElectingVoters;
	type MaxWinners = MaxActiveValidators;
	// type MaxElectingVoters = IncPerRound<10_000, 1000>;
	type BenchmarkingConfig = ElectionProviderBenchmarkConfig;
}

pub mod voter_bags;
parameter_types! {
	pub const BagThresholds: &'static [u64] = &voter_bags::THRESHOLDS;
}

impl pallet_bags_list::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type ScoreProvider = Staking;
	type Score = u64;
	type WeightInfo = pallet_bags_list::weights::SubstrateWeight<Runtime>;
	type BagThresholds = BagThresholds;
}

// Create the runtime by composing the FRAME pallets that were previously configured.
construct_runtime!(
	pub enum Runtime where
		Block = Block,
		NodeBlock = opaque::Block,
		UncheckedExtrinsic = UncheckedExtrinsic
	{
		System: frame_system,
		RandomnessCollectiveFlip: pallet_randomness_collective_flip,
		Timestamp: pallet_timestamp,
		Sudo: pallet_sudo,
		Aura: pallet_aura,
		Grandpa: pallet_grandpa,
		Balances: pallet_balances,
		Staking: pallet_staking,
		BagsList: pallet_bags_list,
		Session: pallet_session,
		TransactionPayment: pallet_transaction_payment,
		ElectionProviderMultiPhase: election_multi_phase,
	}
);

/// The address format for describing accounts.
pub type Address = sp_runtime::MultiAddress<AccountId, ()>;
/// Block header type as expected by this runtime.
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
/// Block type as expected by this runtime.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;
/// The SignedExtension to the basic transaction logic.
pub type SignedExtra = (
	frame_system::CheckSpecVersion<Runtime>,
	frame_system::CheckTxVersion<Runtime>,
	frame_system::CheckGenesis<Runtime>,
	frame_system::CheckEra<Runtime>,
	frame_system::CheckNonce<Runtime>,
	frame_system::CheckWeight<Runtime>,
	pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
);
/// Unchecked extrinsic type as expected by this runtime.
pub type UncheckedExtrinsic =
	generic::UncheckedExtrinsic<Address, RuntimeCall, Signature, SignedExtra>;
/// Executive: handles dispatch to the various modules.
pub type Executive = frame_executive::Executive<
	Runtime,
	Block,
	frame_system::ChainContext<Runtime>,
	Runtime,
	AllPalletsWithSystem,
>;

impl_runtime_apis! {
	impl sp_api::Core<Block> for Runtime {
		fn version() -> RuntimeVersion {
			VERSION
		}

		fn execute_block(block: Block) {
			Executive::execute_block(block);
		}

		fn initialize_block(header: &<Block as BlockT>::Header) {
			Executive::initialize_block(header)
		}
	}

	impl sp_api::Metadata<Block> for Runtime {
		fn metadata() -> OpaqueMetadata {
			OpaqueMetadata::new(Runtime::metadata().into())
		}
	}

	impl sp_block_builder::BlockBuilder<Block> for Runtime {
		fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyExtrinsicResult {
			Executive::apply_extrinsic(extrinsic)
		}

		fn finalize_block() -> <Block as BlockT>::Header {
			Executive::finalize_block()
		}

		fn inherent_extrinsics(data: sp_inherents::InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
			data.create_extrinsics()
		}

		fn check_inherents(
			block: Block,
			data: sp_inherents::InherentData,
		) -> sp_inherents::CheckInherentsResult {
			data.check_extrinsics(&block)
		}
	}

	impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block> for Runtime {
		fn validate_transaction(
			source: TransactionSource,
			tx: <Block as BlockT>::Extrinsic,
			block_hash: <Block as BlockT>::Hash,
		) -> TransactionValidity {
			Executive::validate_transaction(source, tx, block_hash)
		}
	}

	impl sp_offchain::OffchainWorkerApi<Block> for Runtime {
		fn offchain_worker(header: &<Block as BlockT>::Header) {
			Executive::offchain_worker(header)
		}
	}

	impl sp_consensus_aura::AuraApi<Block, AuraId> for Runtime {
		fn slot_duration() -> sp_consensus_aura::SlotDuration {
			sp_consensus_aura::SlotDuration::from_millis(Aura::slot_duration())
		}

		fn authorities() -> Vec<AuraId> {
			Aura::authorities().into_inner()
		}
	}

	impl sp_session::SessionKeys<Block> for Runtime {
		fn generate_session_keys(seed: Option<Vec<u8>>) -> Vec<u8> {
			opaque::SessionKeys::generate(seed)
		}

		fn decode_session_keys(
			encoded: Vec<u8>,
		) -> Option<Vec<(Vec<u8>, KeyTypeId)>> {
			opaque::SessionKeys::decode_into_raw_public_keys(&encoded)
		}
	}

	impl fg_primitives::GrandpaApi<Block> for Runtime {
		fn grandpa_authorities() -> GrandpaAuthorityList {
			Grandpa::grandpa_authorities()
		}

		fn current_set_id() -> fg_primitives::SetId {
			Grandpa::current_set_id()
		}

		fn submit_report_equivocation_unsigned_extrinsic(
			_equivocation_proof: fg_primitives::EquivocationProof<
				<Block as BlockT>::Hash,
				NumberFor<Block>,
			>,
			_key_owner_proof: fg_primitives::OpaqueKeyOwnershipProof,
		) -> Option<()> {
			None
		}

		fn generate_key_ownership_proof(
			_set_id: fg_primitives::SetId,
			_authority_id: GrandpaId,
		) -> Option<fg_primitives::OpaqueKeyOwnershipProof> {
			// NOTE: this is the only implementation possible since we've
			// defined our key owner proof type as a bottom type (i.e. a type
			// with no values).
			None
		}
	}

	impl frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Index> for Runtime {
		fn account_nonce(account: AccountId) -> Index {
			System::account_nonce(account)
		}
	}

	impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<Block, Balance> for Runtime {
		fn query_info(
			uxt: <Block as BlockT>::Extrinsic,
			len: u32,
		) -> pallet_transaction_payment_rpc_runtime_api::RuntimeDispatchInfo<Balance> {
			TransactionPayment::query_info(uxt, len)
		}
		fn query_fee_details(
			uxt: <Block as BlockT>::Extrinsic,
			len: u32,
		) -> pallet_transaction_payment::FeeDetails<Balance> {
			TransactionPayment::query_fee_details(uxt, len)
		}
	}

	impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentCallApi<Block, Balance, RuntimeCall>
		for Runtime
	{
		fn query_call_info(call: RuntimeCall, len: u32) -> pallet_transaction_payment_rpc_runtime_api::RuntimeDispatchInfo<Balance> {
			TransactionPayment::query_call_info(call, len)
		}
		fn query_call_fee_details(call: RuntimeCall, len: u32) -> pallet_transaction_payment::FeeDetails<Balance> {
			TransactionPayment::query_call_fee_details(call, len)
		}
	}

	impl crate::TestConfigApi<Block> for Runtime {
		fn set_length(len: u32) {
			block_length::set(len)
		}
		fn set_weight(weight: u64) {
			block_weight::set(weight)
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	impl frame_benchmarking::Benchmark<Block> for Runtime {
		fn benchmark_metadata(extra: bool) -> (
			Vec<frame_benchmarking::BenchmarkList>,
			Vec<frame_support::traits::StorageInfo>,
		) {
			unimplemented!();
		}

		fn dispatch_benchmark(
			config: frame_benchmarking::BenchmarkConfig
		) -> Result<Vec<frame_benchmarking::BenchmarkBatch>, sp_runtime::RuntimeString> {
			unimplemented!();
		}
	}
}
