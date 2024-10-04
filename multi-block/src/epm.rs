use crate::{error::Error, prelude::*, static_types};

const EPM_PALLET_NAME: &str = "ElectionProviderMultiBlock";

#[derive(Copy, Clone, Debug)]
struct EpmConstant {
	epm: &'static str,
	constant: &'static str,
}

impl std::fmt::Display for EpmConstant {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_fmt(format_args!("{}::{}", self.epm, self.constant))
	}
}

impl EpmConstant {
	const fn new(constant: &'static str) -> Self {
		Self { epm: EPM_PALLET_NAME, constant }
	}
	const fn to_parts(self) -> (&'static str, &'static str) {
		(self.epm, self.constant)
	}
}

pub(crate) fn update_metadata_constants(api: &ChainClient) -> Result<(), Error> {
    const PAGES: EpmConstant = EpmConstant::new("Pages");
    const TARGET_SNAPSHOT_PER_BLOCK: EpmConstant = EpmConstant::new("TargetSnapshotPerBlock");
    const VOTER_SNAPSHOT_PER_BLOCK: EpmConstant = EpmConstant::new("VoterSnapshotPerBlock");

    let pages: u32 = read_constant(api, PAGES)?;
    let target_snapshot_per_block: u32 = read_constant(api, TARGET_SNAPSHOT_PER_BLOCK)?;
    let voter_snapshot_per_block: u32 = read_constant(api, VOTER_SNAPSHOT_PER_BLOCK)?;

	fn log_metadata(metadata: EpmConstant, val: impl std::fmt::Display) {
		log::info!(target: LOG_TARGET, "updating metadata constant `{metadata}`: {val}",);
	}

    log_metadata(PAGES, pages);
    log_metadata(TARGET_SNAPSHOT_PER_BLOCK, target_snapshot_per_block);
    log_metadata(VOTER_SNAPSHOT_PER_BLOCK, voter_snapshot_per_block);

    static_types::Pages::set(pages);
    static_types::TargetSnapshotPerBlock::set(target_snapshot_per_block);
    static_types::VoterSnapshotPerBlock::set(voter_snapshot_per_block);

	Ok(())
}

fn read_constant<'a, T: serde::Deserialize<'a>>(
	api: &ChainClient,
	constant: EpmConstant,
) -> Result<T, Error> {
	let (epm_name, constant) = constant.to_parts();

	let val = api
		.constants()
		.at(&subxt::dynamic::constant(epm_name, constant))
		.map_err(|e| invalid_metadata_error(constant.to_string(), e))?
		.to_value()
		.map_err(|e| Error::Subxt(e.into()))?;

	scale_value::serde::from_value::<_, T>(val).map_err(|e| {
		Error::InvalidMetadata(format!("Decoding `{}` failed {}", std::any::type_name::<T>(), e))
	})
}

fn invalid_metadata_error<E: std::error::Error>(item: String, err: E) -> Error {
	Error::InvalidMetadata(format!("{} failed: {}", item, err))
}

