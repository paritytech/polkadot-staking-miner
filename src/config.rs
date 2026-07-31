//! Subxt configuration for the staking miner.
//!
//! Follows subxt's own Asset Hub recipe (`subxt/examples/config_assethub.rs`): wrap
//! [`SubstrateConfig`], drop the account index from the address type, and forward the rest.
//!
//! The one addition on top is the transaction extensions: Asset Hub runtimes carry extensions
//! subxt does not implement. Most cost nothing, because subxt's `frame-decode` skips extensions
//! whose value type is empty and encodes `None` for `Option<_>` ones, but any other shape makes
//! signing fail outright with `TransactionExtensions(NotFound(..))`.

use codec::Encode;
use scale_info::PortableRegistry;
use subxt::{
	config::{
		ClientState, Config as ConfigT, DefaultExtrinsicParamsBuilder, HashFor, PolkadotConfig,
		SubstrateConfig, TransactionExtension, TransactionExtensions, transaction_extensions,
	},
	error::TransactionExtensionError,
	ext::frame_decode,
	metadata::ArcMetadata,
};

/// Subxt config used by the staking miner on every supported chain.
#[derive(Debug, Clone, Default)]
pub struct StakingMinerConfig(SubstrateConfig);

impl ConfigT for StakingMinerConfig {
	type Address = <PolkadotConfig as ConfigT>::Address;

	type AccountId = <SubstrateConfig as ConfigT>::AccountId;
	type Signature = <SubstrateConfig as ConfigT>::Signature;
	type Header = <SubstrateConfig as ConfigT>::Header;
	type Hasher = <SubstrateConfig as ConfigT>::Hasher;
	type AssetId = <SubstrateConfig as ConfigT>::AssetId;
	type TransactionExtensions = StakingMinerTransactionExtensions;

	fn genesis_hash(&self) -> Option<HashFor<Self>> {
		self.0.genesis_hash()
	}

	fn spec_and_transaction_version_for_block_number(
		&self,
		block_number: u64,
	) -> Option<(u32, u32)> {
		self.0.spec_and_transaction_version_for_block_number(block_number)
	}

	fn metadata_for_spec_version(&self, spec_version: u32) -> Option<ArcMetadata> {
		self.0.metadata_for_spec_version(spec_version)
	}

	fn set_metadata_for_spec_version(&self, spec_version: u32, metadata: ArcMetadata) {
		self.0.set_metadata_for_spec_version(spec_version, metadata)
	}

	// `legacy_types_for_spec_version` is deliberately not forwarded: we never set legacy types on
	// the inner config, so the trait's `None` default is what we want. Asset Hub has no pre-V14
	// blocks to decode, and this way subxt errors instead of mis-decoding if one ever shows up.
}

/// Subxt's default transaction extensions, plus the ones the miner implements itself.
///
/// The first nine mirror subxt's `DefaultTransactionExtensions`, so that its own implementations
/// are reused verbatim. Keep them in step with subxt: dropping one of ours silently loses it, but
/// a new default of theirs breaks [`ExtrinsicParamsBuilder::build`] at compile time.
pub type StakingMinerTransactionExtensions = (
	transaction_extensions::VerifySignature<StakingMinerConfig>,
	transaction_extensions::CheckSpecVersion,
	transaction_extensions::CheckTxVersion,
	transaction_extensions::CheckNonce,
	transaction_extensions::CheckGenesis<StakingMinerConfig>,
	transaction_extensions::CheckMortality<StakingMinerConfig>,
	transaction_extensions::ChargeAssetTxPayment<StakingMinerConfig>,
	transaction_extensions::ChargeTransactionPayment,
	transaction_extensions::CheckMetadataHash,
	RestrictOrigins,
);

/// Parameters for [`StakingMinerTransactionExtensions`].
pub type ExtrinsicParams =
	<StakingMinerTransactionExtensions as TransactionExtensions<StakingMinerConfig>>::Params;

/// The `RestrictOrigins` transaction extension of `pallet-origin-restriction`.
///
/// Its value is a `bool` enabling the restriction check, which is the shape `frame-decode` can
/// neither skip nor default, so a chain carrying this extension cannot be signed for without it.
///
/// Always enabled: `false` asserts "no restriction check needed" and a restricted origin sending it
/// is rejected with `InvalidTransaction::Call`.
///
/// TODO: this can be greatly simplified once https://github.com/paritytech/subxt/issues/2265 is fixed.
/// What is missing before that, is a way to hand subxt a value for an extension it does
/// not implement. Note that this workaround does hardcode that shape, though. Were the extension
/// ever reshaped to `Option<_>`, this would keep writing a bare `bool` where the runtime expects
/// the new encoding. A reshape to `()` needs nothing instead, since an empty value never reaches
/// this impl.
#[derive(Debug)]
pub struct RestrictOrigins(bool);

impl<T: ConfigT> TransactionExtension<T> for RestrictOrigins {
	type Decoded = bool;
	type Params = ();

	fn new(
		_client: &ClientState<T>,
		_params: Self::Params,
	) -> Result<Self, TransactionExtensionError> {
		Ok(Self(true))
	}
}

impl frame_decode::extrinsics::TransactionExtension<PortableRegistry> for RestrictOrigins {
	const NAME: &str = "RestrictOrigins";

	fn encode_value_to(
		&self,
		_type_id: u32,
		_type_resolver: &PortableRegistry,
		out: &mut Vec<u8>,
	) -> Result<(), frame_decode::extrinsics::TransactionExtensionError> {
		self.0.encode_to(out);
		Ok(())
	}

	fn encode_implicit_to(
		&self,
		_type_id: u32,
		_type_resolver: &PortableRegistry,
		_out: &mut Vec<u8>,
	) -> Result<(), frame_decode::extrinsics::TransactionExtensionError> {
		Ok(())
	}
}

/// Mirrors subxt's [`DefaultExtrinsicParamsBuilder`], extended with the parameters of the
/// extensions the miner adds on top of subxt's defaults.
#[derive(Default)]
pub struct ExtrinsicParamsBuilder(DefaultExtrinsicParamsBuilder<StakingMinerConfig>);

impl ExtrinsicParamsBuilder {
	/// Provide a specific nonce for the submitter of the extrinsic.
	pub fn nonce(self, nonce: u64) -> Self {
		Self(self.0.nonce(nonce))
	}

	/// Make the transaction mortal for the given number of blocks from the current one.
	pub fn mortal(self, for_n_blocks: u64) -> Self {
		Self(self.0.mortal(for_n_blocks))
	}

	/// Build the parameters.
	pub fn build(self) -> ExtrinsicParams {
		let default = self.0.build();
		(
			default.0,
			default.1,
			default.2,
			default.3,
			default.4,
			default.5,
			default.6,
			default.7,
			default.8,
			// RestrictOrigins takes no parameters.
			(),
		)
	}
}
