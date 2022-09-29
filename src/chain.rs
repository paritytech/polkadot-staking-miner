//! Chain specific types for polkadot, kusama and westend.

// A big chunk of this file, annotated with `SYNC` should be in sync with the values that are used in
// the real runtimes. Unfortunately, no way to get around them now.
// TODO: There might be a way to create a new crate called `polkadot-runtime-configs` in
// polkadot, that only has `const` and `type`s that are used in the runtime, and we can import
// that.

use crate::{helpers::unsigned_solution_tx, prelude::*, static_types};
use codec::{Decode, Encode};
use frame_election_provider_support::traits::NposSolution;
use frame_support::{traits::ConstU32, weights::Weight};
use once_cell::sync::OnceCell;
use pallet_election_provider_multi_phase::{RawSolution, SolutionOrSnapshotSize};
use pallet_transaction_payment::RuntimeDispatchInfo;
use sp_core::Bytes;
use subxt::rpc::rpc_params;

pub static SHARED_CLIENT: OnceCell<SubxtClient> = OnceCell::new();

pub mod westend {
	use super::*;

	// SYNC
	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution16::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = sp_runtime::PerU16,
			MaxVoters = ConstU32::<22500>
		>(16)
	);

	#[derive(Debug)]
	pub struct MinerConfig;
	impl pallet_election_provider_multi_phase::unsigned::MinerConfig for MinerConfig {
		type AccountId = AccountId;
		type MaxLength = static_types::MaxLength;
		type MaxWeight = static_types::MaxWeight;
		type MaxVotesPerVoter = static_types::MaxVotesPerVoter;
		type Solution = NposSolution16;

		// SYNC
		fn solution_weight(
			voters: u32,
			targets: u32,
			active_voters: u32,
			desired_targets: u32,
		) -> Weight {
			// Mock a RawSolution to get the correct weight without having to do the heavy work.
			let raw_solution = RawSolution {
				solution: NposSolution16 {
					votes1: mock_votes(
						active_voters,
						desired_targets.try_into().expect("Desired targets < u16::MAX"),
					),
					..Default::default()
				},
				..Default::default()
			};

			assert_eq!(raw_solution.solution.voter_count(), active_voters as usize);
			assert_eq!(raw_solution.solution.unique_targets().len(), desired_targets as usize);

			get_weight(raw_solution, SolutionOrSnapshotSize { voters, targets })
		}
	}
}

pub mod polkadot {
	use super::*;

	// SYNC
	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution16::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = sp_runtime::PerU16,
			MaxVoters = ConstU32::<22500>
		>(16)
	);

	#[derive(Debug)]
	pub struct MinerConfig;
	impl pallet_election_provider_multi_phase::unsigned::MinerConfig for MinerConfig {
		type AccountId = AccountId;
		type MaxLength = static_types::MaxLength;
		type MaxWeight = static_types::MaxWeight;
		type MaxVotesPerVoter = static_types::MaxVotesPerVoter;
		type Solution = NposSolution16;

		// SYNC
		fn solution_weight(
			voters: u32,
			targets: u32,
			active_voters: u32,
			desired_targets: u32,
		) -> Weight {
			// Mock a RawSolution to get the correct weight without having to do the heavy work.
			let raw = RawSolution {
				solution: NposSolution16 {
					votes1: mock_votes(
						active_voters,
						desired_targets.try_into().expect("Desired targets < u16::MAX"),
					),
					..Default::default()
				},
				..Default::default()
			};

			assert_eq!(raw.solution.voter_count(), active_voters as usize);
			assert_eq!(raw.solution.unique_targets().len(), desired_targets as usize);

			get_weight(raw, SolutionOrSnapshotSize { voters, targets })
		}
	}
}

pub mod kusama {
	use super::*;

	// SYNC
	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution24::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = sp_runtime::PerU16,
			MaxVoters = ConstU32::<12500>
		>(24)
	);

	#[derive(Debug)]
	pub struct MinerConfig;
	impl pallet_election_provider_multi_phase::unsigned::MinerConfig for MinerConfig {
		type AccountId = AccountId;
		type MaxLength = static_types::MaxLength;
		type MaxWeight = static_types::MaxWeight;
		type MaxVotesPerVoter = static_types::MaxVotesPerVoter;
		type Solution = NposSolution24;

		// SYNC
		fn solution_weight(
			voters: u32,
			targets: u32,
			active_voters: u32,
			desired_targets: u32,
		) -> Weight {
			// Mock a RawSolution to get the correct weight without having to do the heavy work.
			let raw = RawSolution {
				solution: NposSolution24 {
					votes1: mock_votes(
						active_voters,
						desired_targets.try_into().expect("Desired targets < u16::MAX"),
					),
					..Default::default()
				},
				..Default::default()
			};

			assert_eq!(raw.solution.voter_count(), active_voters as usize);
			assert_eq!(raw.solution.unique_targets().len(), desired_targets as usize);

			get_weight(raw, SolutionOrSnapshotSize { voters, targets })
		}
	}
}

/// Helper to fetch the weight from a remote node
///
/// Panics: if the RPC call fails or if decoding the response as a `Weight` fails.
fn get_weight<S: Encode + NposSolution>(
	raw_solution: RawSolution<S>,
	witness: SolutionOrSnapshotSize,
) -> Weight {
	let tx = unsigned_solution_tx(raw_solution, witness);

	futures::executor::block_on(async {
		let client = SHARED_CLIENT.get().expect("shared client is configured as start; qed");

		let call_data = {
			let mut buffer = Vec::new();

			let encoded_call = client.tx().call_data(&tx).unwrap();
			let encoded_len = encoded_call.len() as u32;

			buffer.extend(encoded_call);
			encoded_len.encode_to(&mut buffer);

			Bytes(buffer)
		};

		let bytes: Bytes = client
			.rpc()
			.request(
				"state_call",
				rpc_params!["TransactionPaymentCallApi_query_call_info", call_data],
			)
			.await
			.unwrap();

		let info: RuntimeDispatchInfo<Balance> = Decode::decode(&mut bytes.0.as_ref()).unwrap();

		log::trace!(
			target: LOG_TARGET,
			"Received weight of `Solution Extrinsic` from remote node: {:?}",
			info.weight
		);

		info.weight
	})
}

fn mock_votes(voters: u32, desired_targets: u16) -> Vec<(u32, u16)> {
	assert!(voters >= desired_targets as u32);
	(0..voters).zip((0..desired_targets).cycle()).collect()
}

#[cfg(test)]
#[test]
fn mock_votes_works() {
	assert_eq!(mock_votes(3, 2), vec![(0, 0), (1, 1), (2, 0)]);
	assert_eq!(mock_votes(3, 3), vec![(0, 0), (1, 1), (2, 2)]);
}
