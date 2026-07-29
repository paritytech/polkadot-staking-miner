//! Subxt configuration for the staking miner.
//!
//! Identical to [`subxt::PolkadotConfig`] apart from the transaction extensions: Asset Hub runtimes
//! carry extensions that subxt does not implement. Most cost nothing, because subxt's
//! `frame-decode` skips extensions whose value type is empty and encodes `None` for `Option<_>`
//! ones, but any other shape makes signing fail outright with
//! `TransactionExtensions(NotFound(..))`.

use codec::Encode;
use scale_info::PortableRegistry;
use scale_info_legacy::TypeRegistrySet;
use subxt::{
	PolkadotConfig,
	config::{
		ClientState, Config as ConfigT, DefaultExtrinsicParamsBuilder, HashFor,
		TransactionExtension, TransactionExtensions, transaction_extensions,
	},
	error::TransactionExtensionError,
	ext::frame_decode,
	metadata::ArcMetadata,
};

/// Subxt config used by the staking miner on every supported chain.
#[derive(Debug, Clone, Default)]
pub struct StakingMinerConfig(PolkadotConfig);

impl ConfigT for StakingMinerConfig {
	type AccountId = <PolkadotConfig as ConfigT>::AccountId;
	type Address = <PolkadotConfig as ConfigT>::Address;
	type Signature = <PolkadotConfig as ConfigT>::Signature;
	type Header = <PolkadotConfig as ConfigT>::Header;
	type Hasher = <PolkadotConfig as ConfigT>::Hasher;
	type AssetId = <PolkadotConfig as ConfigT>::AssetId;
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

	fn legacy_types_for_spec_version(&'_ self, spec_version: u32) -> Option<TypeRegistrySet<'_>> {
		self.0.legacy_types_for_spec_version(spec_version)
	}
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
/// Always enabled: the pallet rejects a restricted origin's transaction when the extension is
/// disabled, and enabling it merely costs some pre-dispatch weight for origins that are not
/// restricted at all — which is every origin the miner signs with.
///
/// A `bool` value is a legitimate shape for a transaction extension; what is missing is a way to
/// hand subxt a value for an extension it does not implement, so this is the fix rather than a
/// workaround. It does hardcode that shape, though. Were the extension ever reshaped to
/// `Option<_>`, this would keep writing a bare `bool` where the runtime expects the new encoding,
/// so check the value type is still a `bool` whenever a runtime upgrade touches it — a reshape to
/// `()` needs nothing, since an empty value never reaches this impl.
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
