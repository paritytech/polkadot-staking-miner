use std::{
    fs::{File, create_dir_all},
    io::{BufWriter, Read, Write},
    path::Path,
};

use anyhow::{Context, Result, anyhow};
use futures::future::join_all;
use subxt::{OnlineClient, PolkadotConfig, dynamic::Value, utils::AccountId32};
use tokio;

use scale_value::At;

use crate::types::{
    Nominations, NominatorOut, CandidateOut, ElectionData, 
    AccountId, Balance, EraIndex, NominatorData, ValidatorStake,
    SnapshotData, SnapshotResult
};

/// Data fetcher for chain election data
pub struct DataFetcher {
    client: OnlineClient<PolkadotConfig>,
    cache_dir: String,
}

impl DataFetcher {
    /// Create a new data fetcher connected to chain
    pub async fn new(endpoint: &str, cache_dir: &str) -> Result<Self> {
        println!("Connecting to endpoint: {}", endpoint);

        let client = OnlineClient::<PolkadotConfig>::from_url(endpoint)
            .await
            .context("Failed to connect to endpoint")?;

        println!("Successfully connected to endpoint");

        // Ensure cache directory exists
        create_dir_all(cache_dir)
            .context("Failed to create cache directory")?;

        Ok(Self { 
            client,
            cache_dir: cache_dir.to_string(),
        })
    }

    /// Fetch current era information
    pub async fn fetch_current_era(&self) -> Result<EraIndex> {
        println!("Fetching current era information...");

        // Query the Staking pallet's ActiveEra storage
        let active_era_query = subxt::dynamic::storage("Staking", "ActiveEra", vec![]);

        let active_era = self
            .client
            .storage()
            .at_latest()
            .await?
            .fetch(&active_era_query)
            .await?;

        match active_era {
            Some(era_data) => {
                println!("ActiveEra data found, decoding...");
                // Parse the era index from the ActiveEra struct using SCALE codec
                // ActiveEra is a struct with { index: EraIndex, start: Option<Moment> }
                let era_bytes = era_data.encoded();
                let mut cursor = &era_bytes[..];

                // Decode the EraIndex (u32) - this is the first field
                let era_index: EraIndex = parity_scale_codec::Decode::decode(&mut cursor)
                    .context("Failed to decode era index")?;

                println!("Current active era: {}", era_index);
                Ok(era_index)
            }
            None => {
                eprintln!("No ActiveEra data found, using default era 0");
                Ok(0)
            }
        }
    }

    /// Fetch staking parameters (desired validators, etc.)
    pub async fn fetch_staking_params(&self) -> Result<(u32, u32)> {
        println!("Fetching staking parameters...");

        // Fetch desired validators count
        let validator_count_query = subxt::dynamic::storage("Staking", "ValidatorCount", vec![]);

        let validator_count = self
            .client
            .storage()
            .at_latest()
            .await?
            .fetch(&validator_count_query)
            .await?;

        let desired_validators = match validator_count {
            Some(count_data) => {
                println!("ValidatorCount data found, decoding...");
                parity_scale_codec::Decode::decode(&mut &count_data.encoded()[..])
                    .context("Failed to decode validator count")?
            }
            None => {
                eprintln!("No ValidatorCount data found, using default 16");
                16u32
            }
        };

        // For runners-up, use a reasonable default (typically 16)
        let desired_runners_up = 16u32;

        println!(
            "Desired validators: {}, Desired runners-up: {}",
            desired_validators, desired_runners_up
        );
        Ok((desired_validators, desired_runners_up))
    }

    /// Fetch all candidate validators from the chain
    pub async fn fetch_candidates(&self) -> Result<Vec<ValidatorStake>> {
        println!("Fetching candidate validators...");

        // --- Build a dynamic storage address for Staking.Validators (AccountId => ValidatorPrefs) ---
        let addr = subxt::dynamic::storage("Staking", "Validators", vec![]);

        // --- Iterate the whole map (subxt pages internally) and collect addresses ---
        let mut iter = self.client.storage().at_latest().await?.iter(addr).await?;

        let mut candidate_accounts: Vec<AccountId> = Vec::new();
        let mut count: usize = 0;

        // First pass: collect all candidate accounts
        while let Some(next) = iter.next().await {
            let kv = match next {
                Ok(kv) => kv,
                Err(e) => return Err(anyhow!("storage iteration error: {e}")),
            };

            // kv.key_bytes is the full storage key:
            //   twox128("Staking") ++ twox128("Validators")  [32 bytes prefix]
            //   ++ hasher(key) (8 or 16 bytes) ++ SCALE(key)
            //
            // For AccountId32, the SCALE-encoded key is the final 32 bytes.
            let key_bytes = kv.key_bytes;
            if key_bytes.len() < 32 {
                return Err(anyhow!(
                    "unexpected key length {} (< 32); cannot decode AccountId",
                    key_bytes.len()
                ));
            }
            let tail = &key_bytes[key_bytes.len() - 32..];
            let arr: [u8; 32] = tail
                .try_into()
                .map_err(|_| anyhow!("failed to slice AccountId32 bytes"))?;
            let account = AccountId::from(arr);

            candidate_accounts.push(account);
            count += 1;

            if count % 100 == 0 {
                println!("Collected {} candidate accounts...", count);
            }
        }

        println!(
            "Total candidate accounts collected: {}",
            candidate_accounts.len()
        );

        // Second pass: fetch stakes in batches
        let stakes = self.fetch_stakes_in_batches(&candidate_accounts).await?;

        // Combine accounts with stakes
        let candidates: Vec<ValidatorStake> = candidate_accounts
            .into_iter()
            .zip(stakes.into_iter())
            .collect();

        // write JSON
        let addrs: Vec<CandidateOut> = candidates
            .iter()
            .map(|entry| CandidateOut {
                stash: entry.0.to_string(),
                stake: entry.1.to_string(),
            })
            .collect();

        let candidates_path = Path::new(&self.cache_dir).join("candidates.json");
        let file = File::create(&candidates_path).context("create candidates.json")?;
        let mut writer = BufWriter::with_capacity(1024 * 1024, file); // 1MB buffer
        let json = serde_json::to_string_pretty(&addrs).context("serialize candidates")?;
        writer.write_all(json.as_bytes()).context("write file")?;
        writer.flush().context("flush writer")?;
        println!("Wrote {} candidates to {}", addrs.len(), candidates_path.display());

        println!("Total registered candidates: {}", addrs.len());

        Ok(candidates)
    }

    /// Fetch stakes in batches for better performance
    async fn fetch_stakes_in_batches(&self, accounts: &[AccountId]) -> Result<Vec<Balance>> {
        const BATCH_SIZE: usize = 1000;
        const MAX_CONCURRENT_BATCHES: usize = 10;

        let mut stakes = Vec::with_capacity(accounts.len());
        let mut batch_handles: Vec<tokio::task::JoinHandle<Result<Vec<Balance>>>> = Vec::new();

        // Process accounts in batches
        for chunk in accounts.chunks(BATCH_SIZE) {
            let chunk = chunk.to_vec();
            let client_clone = self.client.clone();

            let handle = tokio::spawn(async move {
                Self::fetch_stakes_batch_static(&client_clone, &chunk).await
            });

            batch_handles.push(handle);

            // Limit concurrent batches
            if batch_handles.len() >= MAX_CONCURRENT_BATCHES {
                let handle = batch_handles.remove(0);
                match handle.await {
                    Ok(Ok(batch_stakes)) => stakes.extend(batch_stakes),
                    Ok(Err(e)) => return Err(e),
                    Err(e) => return Err(anyhow!("Task join error: {}", e)),
                }
            }
        }

        // Wait for remaining batches
        if !batch_handles.is_empty() {
            let batch_results = join_all(batch_handles).await;
            for result in batch_results {
                match result {
                    Ok(Ok(batch_stakes)) => stakes.extend(batch_stakes),
                    Ok(Err(e)) => return Err(e),
                    Err(e) => return Err(anyhow!("Task join error: {}", e)),
                }
            }
        }

        Ok(stakes)
    }

    /// Static version of stake fetching for use in spawned tasks
    async fn fetch_stakes_batch_static(
        client: &OnlineClient<PolkadotConfig>,
        accounts: &[AccountId],
    ) -> Result<Vec<Balance>> {
        let at = client.storage().at_latest().await?;

        let mut futs = Vec::with_capacity(accounts.len());
        for account in accounts {
            let at = at.clone();
            let account = account.clone();

            futs.push(async move {
                let ledger_addr = subxt::dynamic::storage(
                    "Staking",
                    "Ledger",
                    vec![Value::from_bytes(account.0)],
                );

                let mut stake = 0u128;
                if let Some(ledger) = at.fetch(&ledger_addr).await? {
                    if let Some(total) = ledger.to_value()?.at("total").and_then(|v| v.as_u128()) {
                        stake = total;
                    }
                }

                Ok(stake)
            });
        }

        let results: Vec<std::result::Result<Balance, subxt::Error>> = join_all(futs).await;
        let mut stakes = Vec::with_capacity(results.len());

        for result in results {
            stakes.push(result.map_err(|e| anyhow!("Failed to fetch stake: {}", e))?);
        }

        Ok(stakes)
    }

    /// Fetch all elected candidates after the election from the chain for checking the accuracy of the offline prediction
    // pub async fn fetch_active_validators(&self) -> Result<Vec<AccountId>> {
    //     println!("Fetching active validator candidates...");

    //     // Query the Session pallet's Validators storage (current validators)
    //     // Session::Validators returns a Vec<AccountId32> directly, not a map
    //     let validators_query = subxt::dynamic::storage("Session", "Validators", vec![]);

    //     let validators = self
    //         .client
    //         .storage()
    //         .at_latest()
    //         .await?
    //         .fetch(&validators_query)
    //         .await?;

    //     let account_ids = match validators {
    //         Some(validators_data) => {
    //             println!("Active validators data found, decoding...");
    //             // Parse the validators list - Session::Validators is a Vec<AccountId32>
    //             let validators_list: Vec<AccountId> =
    //                 parity_scale_codec::Decode::decode(&mut &validators_data.encoded()[..])
    //                     .context("Failed to decode active validators list")?;

    //             validators_list
    //         }
    //         None => {
    //             eprintln!("No active validators data found");
    //             vec![]
    //         }
    //     };

    //     println!("Found {} active validators", account_ids.len());

    //     // Convert to SS58 for JSON output
    //     let addrs: Vec<String> = account_ids.iter().map(|acc| acc.to_string()).collect();

    //     // Use buffered writer for better I/O performance
    //     let active_validators_path = Path::new(&self.cache_dir).join("active_validators.json");
    //     let file = File::create(&active_validators_path)
    //         .context("Failed to create active_validators.json")?;
    //     let mut writer = BufWriter::new(file);
    //     let json = serde_json::to_string_pretty(&addrs)
    //         .context("Failed to serialize active validators to JSON")?;
    //     writer
    //         .write_all(json.as_bytes())
    //         .context("Failed to write active validators to file")?;
    //     writer.flush().context("Failed to flush writer")?;
    //     println!("Wrote active validators to {}", active_validators_path.display());

    //     println!("Total active validators: {}", addrs.len());

    //     Ok(account_ids)
    // }

    /// Fetch all nominators/voters along with their nominations and stakes from the chain
    pub async fn fetch_nominators(&self) -> Result<Vec<NominatorData>> {
        let curr_time = std::time::SystemTime::now();

        // Build an iterator over Staking::Nominators at latest block
        let query = subxt::dynamic::storage("Staking", "Nominators", vec![]);
        let mut iter = self.client.storage().at_latest().await?.iter(query).await?;

        let mut count: usize = 0;

        let mut nominators: Vec<(AccountId32, Nominations)> = Vec::new();

        while let Some(Ok(kv)) = iter.next().await {
            // Recover stash AccountId32 from the storage key bytes:
            // For Twox64Concat maps, the original key is at the end (last 32 bytes for AccountId32).
            let key_bytes = kv.key_bytes.as_slice();
            let start = key_bytes.len().saturating_sub(32);
            let stash_arr: [u8; 32] = key_bytes[start..].try_into().expect("32 bytes");
            let stash = AccountId32(stash_arr);

            // Decode the nominations into a typed struct.
            let nominations: Nominations =
                parity_scale_codec::Decode::decode(&mut &kv.value.encoded()[..])?;

            nominators.push((stash.clone(), nominations));

            count += 1;
            if count % 1000 == 0 {
                println!("Processed {} nominators...", count);
            }
        }

        // fetch the stakes of the nominators
        let nominator_stashes: Vec<AccountId32> =
            nominators.iter().map(|entry| entry.0.clone()).collect();
        let nominator_stakes = self.fetch_stakes_in_batches(&nominator_stashes).await?;

        let results: Vec<NominatorOut> = nominators
            .into_iter()
            .zip(nominator_stakes.into_iter())
            .map(|((stash, nominations), active_stake)| NominatorOut {
                stash: stash.to_string(),
                active_stake: active_stake.to_string(),
                targets: nominations.targets.iter().map(|a| a.to_string()).collect(),
                submitted_in: nominations.submitted_in,
                suppressed: nominations.suppressed,
            })
            .collect();

        println!(
            "time taken to fetch nominators: {} minutes",
            curr_time.elapsed().unwrap().as_secs_f64() / 60.0
        );

        // write JSON with optimized serialization
        let nominators_path = Path::new(&self.cache_dir).join("nominators.json");
        let file = File::create(&nominators_path).context("create nominators.json")?;
        let mut writer = BufWriter::with_capacity(1024 * 1024, file); // 1MB buffer
        let json = serde_json::to_string_pretty(&results).context("serialize nominators")?;
        writer.write_all(json.as_bytes()).context("write file")?;
        writer.flush().context("flush writer")?;
        println!("Wrote {} nominators to {}", results.len(), nominators_path.display());

        println!("{count}");

        // Pre-allocate with known capacity for better performance
        let mut nominators: Vec<NominatorData> = Vec::with_capacity(results.len());

        for nominator in results {
            // Convert SS58 addresses back to AccountId32
            let stash = nominator
                .stash
                .parse::<AccountId>()
                .map_err(|e| anyhow!("Failed to parse stash SS58 address: {}", e))?;

            // Pre-allocate targets vector
            let mut targets = Vec::with_capacity(nominator.targets.len());
            for addr in nominator.targets {
                let target = addr
                    .parse::<AccountId>()
                    .map_err(|e| anyhow!("Failed to parse target SS58 address: {}", e))?;
                targets.push(target);
            }

            // Parse stake from string to u64
            let stake = nominator
                .active_stake
                .parse::<u64>()
                .map_err(|e| anyhow!("Failed to parse active stake: {}", e))?;

            nominators.push((stash, stake, targets));
        }

        Ok(nominators)
    }

    /// Fetch the metadata from a chain
    pub async fn fetch_metadata(&self) -> Result<()> {
        println!("in fetch metadata");

        let metadata = self.client.metadata();

        for pallet in metadata.pallets() {
            println!("pallet: {:?}", pallet.name());
        }

        Ok(())
    }

    /// Fetch election data (tries snapshot first, fallback to Staking pallet if not available)
    pub async fn fetch_election_data_with_snapshot(&self) -> Result<ElectionData> {
        println!("Attempting to fetch snapshot data...");
        
        match self.try_fetch_snapshot_data().await {
            Ok(SnapshotResult::Available(snapshot_data)) => {
                println!("Snapshot data available, using snapshot for prediction");
                self.convert_snapshot_to_election_data(snapshot_data).await
            }
            Ok(SnapshotResult::NotAvailable) => {
                println!("Snapshot data not available, falling back to Staking pallet");
                self.fetch_election_data_staking_only().await
            }
            Err(e) => {
                println!("Error fetching snapshot data: {}, falling back to Staking pallet", e);
                self.fetch_election_data_staking_only().await
            }
        }
    }

    /// Try to fetch snapshot data from multi-block election pallet
    pub async fn try_fetch_snapshot_data(&self) -> Result<SnapshotResult> {
        println!("Checking for multi-block election snapshot data...");
        
        // Try to fetch current round from multi-block election pallet
        let current_round_query = subxt::dynamic::storage("MultiBlockElection", "Round", vec![]);
        let current_round_result = self.client.storage().at_latest().await?.fetch(&current_round_query).await;
        
        let current_round = match current_round_result {
            Ok(Some(round_data)) => {
                let round: u32 = parity_scale_codec::Decode::decode(&mut &round_data.encoded()[..])
                    .context("Failed to decode current round")?;
                println!("Found current round: {}", round);
                round
            }
            Ok(None) => {
                println!("No current round found in multi-block election pallet");
                return Ok(SnapshotResult::NotAvailable);
            }
            Err(e) => {
                println!("Multi-block election pallet not available: {}", e);
                return Ok(SnapshotResult::NotAvailable);
            }
        };

        // Try to fetch desired targets
        let desired_targets_query = subxt::dynamic::storage("MultiBlockElection", "DesiredTargets", vec![Value::u128(current_round as u128)]);
        let desired_targets_result = self.client.storage().at_latest().await?.fetch(&desired_targets_query).await;
        
        let desired_targets = match desired_targets_result {
            Ok(Some(targets_data)) => {
                let targets: u32 = parity_scale_codec::Decode::decode(&mut &targets_data.encoded()[..])
                    .context("Failed to decode desired targets")?;
                println!("Found desired targets: {}", targets);
                targets
            }
            Ok(None) => {
                println!("No desired targets found for round {}", current_round);
                return Ok(SnapshotResult::NotAvailable);
            }
            Err(e) => {
                println!("Error fetching desired targets: {}", e);
                return Ok(SnapshotResult::NotAvailable);
            }
        };

        // Try to fetch target snapshot
        let target_snapshot = self.fetch_target_snapshot(current_round).await?;
        if target_snapshot.is_empty() {
            println!("No target snapshot found for round {}", current_round);
            return Ok(SnapshotResult::NotAvailable);
        }

        // Try to fetch voter snapshots
        let voter_snapshots = self.fetch_voter_snapshots(current_round).await?;
        if voter_snapshots.is_empty() {
            println!("No voter snapshots found for round {}", current_round);
            return Ok(SnapshotResult::NotAvailable);
        }

        println!("Successfully fetched snapshot data: {} targets, {} voters", 
                 target_snapshot.len(), voter_snapshots.len());

        Ok(SnapshotResult::Available(SnapshotData {
            current_round,
            desired_targets,
            target_snapshot,
            voter_snapshots,
        }))
    }

    /// Fetch target snapshot (validators) from multi-block election pallet
    async fn fetch_target_snapshot(&self, round: u32) -> Result<Vec<AccountId>> {
        println!("Fetching target snapshot for round {}", round);
        
        // Try to fetch target snapshot - this is a simplified approach
        // In a real implementation, you'd need to handle pagination
        let target_snapshot_query = subxt::dynamic::storage("MultiBlockElection", "TargetSnapshot", vec![
            Value::u128(0), // page 0
            Value::u128(round as u128)
        ]);
        
        let target_snapshot_result = self.client.storage().at_latest().await?.fetch(&target_snapshot_query).await;
        
        match target_snapshot_result {
            Ok(Some(snapshot_data)) => {
                // Decode the snapshot data - this is a simplified approach
                // The actual structure would depend on the multi-block election implementation
                let snapshot: Vec<AccountId> = parity_scale_codec::Decode::decode(&mut &snapshot_data.encoded()[..])
                    .context("Failed to decode target snapshot")?;
                println!("Fetched {} targets from snapshot", snapshot.len());
                Ok(snapshot)
            }
            Ok(None) => {
                println!("No target snapshot found for round {}", round);
                Ok(vec![])
            }
            Err(e) => {
                println!("Error fetching target snapshot: {}", e);
                Ok(vec![])
            }
        }
    }

    /// Fetch voter snapshots (nominators) from multi-block election pallet
    async fn fetch_voter_snapshots(&self, round: u32) -> Result<Vec<(AccountId, Balance, Vec<AccountId>)>> {
        println!("Fetching voter snapshots for round {}", round);
        
        let mut all_voters = Vec::new();
        
        // Try to fetch voter snapshots with pagination
        // This is a simplified approach - in reality you'd need to handle multiple pages
        for page in 0..10 { // Try up to 10 pages
            let voter_snapshot_query = subxt::dynamic::storage("MultiBlockElection", "VoterSnapshot", vec![
                Value::u128(page),
                Value::u128(round as u128)
            ]);
            
            let voter_snapshot_result = self.client.storage().at_latest().await?.fetch(&voter_snapshot_query).await;
            
            match voter_snapshot_result {
                Ok(Some(snapshot_data)) => {
                    // Decode the voter snapshot data
                    // This is a simplified approach - the actual structure would be more complex
                    let snapshot: Vec<(AccountId, u64, Vec<AccountId>)> = parity_scale_codec::Decode::decode(&mut &snapshot_data.encoded()[..])
                        .context("Failed to decode voter snapshot")?;
                    
                    for (account_id, stake, nominations) in snapshot {
                        all_voters.push((account_id, stake as Balance, nominations));
                    }
                }
                Ok(None) => {
                    // No more pages
                    break;
                }
                Err(e) => {
                    println!("Error fetching voter snapshot page {}: {}", page, e);
                    break;
                }
            }
        }
        
        println!("Fetched {} voters from snapshots", all_voters.len());
        Ok(all_voters)
    }

    /// Convert snapshot data to ElectionData format
    async fn convert_snapshot_to_election_data(&self, snapshot_data: SnapshotData) -> Result<ElectionData> {
        println!("Converting snapshot data to election data format...");
        
        // Convert target snapshot to candidates (with dummy stakes for now)
        let candidates: Vec<ValidatorStake> = snapshot_data.target_snapshot
            .clone()
            .into_iter()
            .map(|account_id| (account_id, 0)) // We don't have self stake from snapshot
            .collect();
        
        // Convert voter snapshots to nominators
        let nominators: Vec<NominatorData> = snapshot_data.voter_snapshots
            .into_iter()
            .map(|(account_id, stake, targets)| (account_id, stake as u64, targets))
            .collect();
        
        // Get current era (we still need this from the Staking pallet)
        let active_era = self.fetch_current_era().await?;
        
        // Get staking parameters (we still need this from the Staking pallet)
        let (_, desired_runners_up) = self.fetch_staking_params().await?;
        
        println!("Converted snapshot data: {} candidates, {} nominators", 
                 candidates.len(), nominators.len());
        
        Ok(ElectionData {
            active_era,
            desired_validators: snapshot_data.desired_targets,
            desired_runners_up,
            candidates,
            nominators
        })
    }

    /// Fetch all required data for election prediction (tries snapshot first, fallback to Staking pallet)
    pub async fn fetch_election_data(&self) -> Result<ElectionData> {
        // Use the snapshot-enabled method by default
        self.fetch_election_data_with_snapshot().await
    }

    /// Fetch all required data for election prediction using only Staking pallet (legacy method)
    pub async fn fetch_election_data_staking_only(&self) -> Result<ElectionData> {
        println!("Fetching all election data from Chain using Staking pallet...");

        let (desired_validators, desired_runners_up) = self.fetch_staking_params().await?;
        let active_era = self.fetch_current_era().await?;
        let candidates = self.fetch_candidates().await?;
        let nominators = self.fetch_nominators().await?;

        println!("Election data fetched successfully:");
        println!("  - Active era: {}", active_era);
        println!("  - Desired validators: {}", desired_validators);
        println!("  - Desired runners-up: {}", desired_runners_up);
        println!("  - Validator candidates: {}", candidates.len());
        println!("  - Nominators: {}", nominators.len());

        Ok(ElectionData {
            active_era,
            desired_validators,
            desired_runners_up,
            candidates,
            nominators
        })
    }

    pub async fn read_election_data_from_files(&self) -> Result<ElectionData> {
        // Read candidates.json
        let candidates_path = Path::new(&self.cache_dir).join("candidates.json");
        let mut candidates_file = File::open(&candidates_path)
            .context(format!("candidates.json not found at {}", candidates_path.display()))?;
        let mut candidates_content = String::new();
        candidates_file
            .read_to_string(&mut candidates_content)
            .context("Failed to read candidates.json")?;

        let candidates_out: Vec<CandidateOut> =
            serde_json::from_str(&candidates_content)
                .context("Failed to parse candidates.json")?;

        let mut candidates_from_file = vec![];
        for candidate in candidates_out {
            // Convert SS58 addresses back to AccountId32
            let stash = candidate
                .stash
                .parse::<AccountId>()
                .map_err(|e| anyhow!("Failed to parse stash SS58 address: {}", e))?;

            // Parse stake from string to u128
            let stake = candidate
                .stake
                .parse::<u128>()
                .map_err(|e| anyhow!("Failed to parse active stake: {}", e))?;

            candidates_from_file.push((stash, stake));
        }

        // Read nominators.json - it contains NominatorOut structs, not tuples
        let nominators_path = Path::new(&self.cache_dir).join("nominators.json");
        let mut nominators_file = File::open(&nominators_path)
            .context(format!("nominators.json not found at {}", nominators_path.display()))?;
        let mut nominators_content = String::new();
        nominators_file
            .read_to_string(&mut nominators_content)
            .context("Failed to read nominators.json")?;

        // Parse as NominatorOut structs first
        let nominators_out: Vec<NominatorOut> =
            serde_json::from_str(&nominators_content)
                .context("Failed to parse nominators.json")?;

        // Convert NominatorOut to the expected tuple format
        let mut nominators_from_file = Vec::new();
        for nominator in nominators_out {
            // Convert SS58 addresses back to AccountId32
            let stash = nominator
                .stash
                .parse::<AccountId>()
                .map_err(|e| anyhow!("Failed to parse stash SS58 address: {}", e))?;
            let targets: Result<Vec<AccountId32>, _> = nominator
                .targets
                .iter()
                .map(|addr| addr.parse::<AccountId32>())
                .collect();
            let targets =
                targets.map_err(|e| anyhow!("Failed to parse target SS58 address: {}", e))?;

            // Parse stake from string to u64
            let stake = nominator
                .active_stake
                .parse::<u64>()
                .map_err(|e| anyhow!("Failed to parse active stake: {}", e))?;

            nominators_from_file.push((stash, stake, targets));
        }

        // fetch required data to run election
        let (desired_validators, desired_runners_up) = self.fetch_staking_params().await?;
        let active_era = self.fetch_current_era().await?;

        Ok(ElectionData {
            active_era,
            desired_validators,
            desired_runners_up,
            candidates: candidates_from_file,
            nominators: nominators_from_file
        })
    }
}
