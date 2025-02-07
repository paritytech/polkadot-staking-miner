use crate::prelude::{Hash, TargetSnapshotPage, VoterSnapshotPage};
use polkadot_sdk::pallet_election_provider_multi_block::unsigned::miner::MinerConfig;
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

type Page = u32;

/// Snapshot of the target and voter pages in the multi-block stuff.
/// 
/// This type is used to store the target and voter snapshots in the multi-block
/// and relies on the hash to verify the snapshot is up-to-date with what's on-chain.
pub struct Snapshot<T: MinerConfig> {
	pub target: Option<(TargetSnapshotPage<T>, Hash)>,
	pub voter: BTreeMap<Page, (VoterSnapshotPage<T>, Hash)>,
	pub n_pages: Page,
}

impl<T: MinerConfig> Snapshot<T> {
	pub fn new(n_pages: Page) -> Self {
		Snapshot { target: None, voter: BTreeMap::new(), n_pages }
	}

	/// Whether the target snapshot needs to be fetched.
	pub fn needs_target_snapshot(&self, hash: Hash) -> bool {
		if let Some((_, target_hash)) = &self.target {
			*target_hash != hash
		} else {
			true
		}
	}

	/// Whether the voter snapshot needs to be fetched.
	pub fn needs_voter_page(&self, page: Page, hash: Hash) -> bool {
		if let Some((_, voter_hash)) = self.voter.get(&page) {
			*voter_hash != hash
		} else {
			true
		}
	}

	/// Set the target snapshot.
	pub fn set_target_snapshot(&mut self, target: TargetSnapshotPage<T>, hash: Hash) {
		self.target = Some((target, hash));
	}

	/// Set a specific voter snapshot.
	pub fn set_voter_page(&mut self, page: Page, voter: VoterSnapshotPage<T>, hash: Hash) {
		assert!(page < self.n_pages, "Page exceeds the maximum number of pages");
		self.voter.insert(page, (voter, hash));
	}

	pub fn set_page_length(&mut self, n_pages: Page) {
		self.n_pages = n_pages;
	}

	/// Clear the snapshot.
	pub fn clear(&mut self) {
		self.target = None;
		self.voter.clear();
	}
}


pub struct SharedSnapshot<T: MinerConfig>(Arc<RwLock<Snapshot<T>>>);

impl<T: MinerConfig> SharedSnapshot<T> {
	pub fn new(n_pages: Page) -> Self {
		SharedSnapshot(Arc::new(RwLock::new(Snapshot::new(n_pages))))
	}

	pub fn read(&self) -> std::sync::RwLockReadGuard<Snapshot<T>> {
		self.0.read().expect("Lock is poisoned")
	}

	pub fn write(&self) -> std::sync::RwLockWriteGuard<Snapshot<T>> {
		self.0.write().expect("Lock is poisoned")
	}
}

impl<T: MinerConfig> Clone for SharedSnapshot<T> {
	fn clone(&self) -> Self {
		SharedSnapshot(self.0.clone())
	}
}