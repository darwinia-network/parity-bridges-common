// Copyright 2019-2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

use crate::error::Error;
use crate::finality::finalize_blocks;
use crate::verification::{is_importable_header, verify_clique_variant_header};
use crate::{ChainTime, ChangeToEnact, CliqueVariantConfiguration, PruningStrategy, Storage};
use bp_eth_clique::{CliqueHeader, HeaderId, Receipt};
use sp_std::{collections::btree_map::BTreeMap, prelude::*};

/// Imports bunch of headers and updates blocks finality.
///
/// Transactions receipts are useless for verifying block in clique consensus
/// If successful, returns tuple where first element is the number of useful headers
/// we have imported and the second element is the number of useless headers (duplicate)
/// we have NOT imported.
/// Returns error if fatal error has occured during import. Some valid headers may be
/// imported in this case.
/// TODO: update me (https://github.com/paritytech/parity-bridges-common/issues/415)
#[allow(clippy::too_many_arguments)]
pub fn import_headers<S: Storage, PS: PruningStrategy, CT: ChainTime>(
	storage: &mut S,
	pruning_strategy: &mut PS,
	clique_variant_config: &CliqueVariantConfiguration,
	submitter: Option<S::Submitter>,
	headers: Vec<CliqueHeader>,
	chain_time: &CT,
	finalized_headers: &mut BTreeMap<S::Submitter, u64>,
) -> Result<(u64, u64), Error> {
	let mut useful = 0;
	let mut useless = 0;
	for header in headers {
		let import_result = import_header(
			storage,
			pruning_strategy,
			clique_variant_config,
			submitter.clone(),
			header,
			chain_time,
		);

		match import_result {
			Ok((_, finalized)) => {
				for (_, submitter) in finalized {
					if let Some(submitter) = submitter {
						*finalized_headers.entry(submitter).or_default() += 1;
					}
				}
				useful += 1;
			}
			Err(Error::AncientHeader) | Err(Error::KnownHeader) => useless += 1,
			Err(error) => return Err(error),
		}
	}

	Ok((useful, useless))
}

/// A vector of finalized headers and their submitters.
pub type FinalizedHeaders<S> = Vec<(HeaderId, Option<<S as Storage>::Submitter>)>;

/// Imports given header and updates blocks finality (if required).
///
/// Transactions receipts are useless here
///
/// Returns imported block id and list of all finalized headers.
/// TODO: update me (https://github.com/paritytech/parity-bridges-common/issues/415)
#[allow(clippy::too_many_arguments)]
pub fn import_header<S: Storage, PS: PruningStrategy, CT: ChainTime>(
	storage: &mut S,
	pruning_strategy: &mut PS,
	clique_variant_config: &CliqueVariantConfiguration,
	submitter: Option<S::Submitter>,
	header: CliqueHeader,
	chain_time: &CT,
) -> Result<(HeaderId, FinalizedHeaders<S>), Error> {
	// first check that we are able to import this header at all
	let (header_id, finalized_id) = is_importable_header(storage, &header)?;

	// verify header
	let import_context = verify_clique_variant_header(storage, clique_variant_config, submitter, &header, chain_time)?;

	// verify validator
	// Retrieve the parent state
	// TODO how to init snapshot?
	let parent_state = Snapshot::new().retrieve(storage, &header.parent_hash, clique_variant_config)?;
	// Try to apply current state, apply() will further check signer and recent signer.
	let mut new_state = parent_state.clone();
	new_state.apply(header, header.number() % clique_variant_config.epoch_length == 0)?;
	new_state.calc_next_timestamp(header.timestamp(), clique_variant_config.period)?;
	new_state.verify(header)?;

	let finalized_blocks = finalize_blocks(
		storage,
		finalized_id,
		header_id,
		import_context.submitter(),
		&header,
		clique_variant_config.two_thirds_majority_transition,
	)?;

	// NOTE: we can't return Err() from anywhere below this line
	// (because otherwise we'll have inconsistent storage if transaction will fail)

	// and finally insert the block
	let (best_id, best_total_difficulty) = storage.best_block();
	let total_difficulty = import_context.total_difficulty() + header.difficulty;
	let is_best = total_difficulty > best_total_difficulty;
	storage.insert_header(import_context.into_import_header(is_best, header_id, header, total_difficulty));

	// compute upper border of updated pruning range
	let new_best_block_id = if is_best { header_id } else { best_id };
	let new_best_finalized_block_id = finalized_blocks.finalized_headers.last().map(|(id, _)| *id);
	let pruning_upper_bound = pruning_strategy.pruning_upper_bound(
		new_best_block_id.number,
		new_best_finalized_block_id
			.map(|id| id.number)
			.unwrap_or(finalized_id.number),
	);

	// now mark finalized headers && prune old headers
	storage.finalize_and_prune_headers(new_best_finalized_block_id, pruning_upper_bound);

	Ok((header_id, finalized_blocks.finalized_headers))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{
		run_test, secret_to_address, test_clique_variant_config, test_validators_config, validator,
		validators_addresses, validators_change_receipt, HeaderBuilder, KeepSomeHeadersBehindBest, TestRuntime,
		GAS_LIMIT,
	};
	use crate::validators::ValidatorsSource;
	use crate::DefaultInstance;
	use crate::{BlocksToPrune, BridgeStorage, Headers, PruningRange};
	use frame_support::{StorageMap, StorageValue};
	use secp256k1::SecretKey;

	const TOTAL_VALIDATORS: usize = 3;

	#[test]
	fn rejects_finalized_block_competitors() {
		run_test(TOTAL_VALIDATORS, |_| {
			let mut storage = BridgeStorage::<TestRuntime>::new();
			storage.finalize_and_prune_headers(
				Some(HeaderId {
					number: 100,
					..Default::default()
				}),
				0,
			);
			assert_eq!(
				import_header(
					&mut storage,
					&mut KeepSomeHeadersBehindBest::default(),
					&test_clique_variant_config(),
					&test_validators_config(),
					None,
					Default::default(),
					&(),
					None,
				),
				Err(Error::AncientHeader),
			);
		});
	}

	#[test]
	fn rejects_known_header() {
		run_test(TOTAL_VALIDATORS, |ctx| {
			let mut storage = BridgeStorage::<TestRuntime>::new();
			let header = HeaderBuilder::with_parent(&ctx.genesis).sign_by(&validator(1));
			assert_eq!(
				import_header(
					&mut storage,
					&mut KeepSomeHeadersBehindBest::default(),
					&test_clique_variant_config(),
					&test_validators_config(),
					None,
					header.clone(),
					&(),
					None,
				)
				.map(|_| ()),
				Ok(()),
			);
			assert_eq!(
				import_header(
					&mut storage,
					&mut KeepSomeHeadersBehindBest::default(),
					&test_clique_variant_config(),
					&test_validators_config(),
					None,
					header,
					&(),
					None,
				)
				.map(|_| ()),
				Err(Error::KnownHeader),
			);
		});
	}

	#[test]
	fn import_header_works() {
		run_test(TOTAL_VALIDATORS, |ctx| {
			let mut storage = BridgeStorage::<TestRuntime>::new();
			let header = HeaderBuilder::with_parent(&ctx.genesis).sign_by(&validator(1));
			let hash = header.compute_hash();
			assert_eq!(
				import_header(
					&mut storage,
					&mut KeepSomeHeadersBehindBest::default(),
					&test_clique_variant_config(),
					&validators_config,
					None,
					header,
					&(),
					None
				)
				.map(|_| ()),
				Ok(()),
			);

			// check that new validators will be used for next header
			let imported_header = Headers::<TestRuntime>::get(&hash).unwrap();
			assert_eq!(
				imported_header.next_validators_set_id,
				1, // new set is enacted from config
			);
		});
	}

	fn import_custom_block<S: Storage>(
		storage: &mut S,
		validators: &[SecretKey],
		header: CliqueHeader,
	) -> Result<HeaderId, Error> {
		let id = header.compute_id();
		import_header(
			storage,
			&mut KeepSomeHeadersBehindBest::default(),
			&test_clique_variant_config(),
			None,
			header,
			&(),
			None,
		)
		.map(|_| id)
	}

	#[test]
	fn import_of_non_best_block_may_finalize_blocks() {
		run_test(TOTAL_VALIDATORS, |ctx| {
			let mut storage = BridgeStorage::<TestRuntime>::new();

			// insert headers (H1, validator1), (H2, validator1), (H3, validator1)
			// making H3 the best header, without finalizing anything (we need 2 signatures)
			let mut expected_best_block = Default::default();
			for i in 1..4 {
				let step = 1 + i * TOTAL_VALIDATORS as u64;
				expected_best_block = import_custom_block(
					&mut storage,
					&ctx.validators,
					HeaderBuilder::with_parent_number(i - 1)
						.step(step)
						.sign_by_set(&ctx.validators),
				)
				.unwrap();
			}
			let (best_block, best_difficulty) = storage.best_block();
			assert_eq!(best_block, expected_best_block);
			assert_eq!(storage.finalized_block(), ctx.genesis.compute_id());

			// insert headers (H1', validator1), (H2', validator2), finalizing H2, even though H3
			// has better difficulty than H2' (because there are more steps involved)
			let mut expected_finalized_block = Default::default();
			let mut parent_hash = ctx.genesis.compute_hash();
			for i in 1..3 {
				let step = i;
				let id = import_custom_block(
					&mut storage,
					&ctx.validators,
					HeaderBuilder::with_parent_hash(parent_hash)
						.step(step)
						.gas_limit((GAS_LIMIT + 1).into())
						.sign_by_set(&ctx.validators),
				)
				.unwrap();
				parent_hash = id.hash;
				if i == 1 {
					expected_finalized_block = id;
				}
			}
			let (new_best_block, new_best_difficulty) = storage.best_block();
			assert_eq!(new_best_block, expected_best_block);
			assert_eq!(new_best_difficulty, best_difficulty);
			assert_eq!(storage.finalized_block(), expected_finalized_block);
		});
	}

	#[test]
	fn append_to_unfinalized_fork_fails() {
		const VALIDATORS: u64 = 5;
		run_test(VALIDATORS as usize, |ctx| {
			let mut storage = BridgeStorage::<TestRuntime>::new();

			// header1, authored by validator[2] is best common block between two competing forks
			let header1 = import_custom_block(
				&mut storage,
				&ctx.validators,
				HeaderBuilder::with_parent_number(0)
					.step(2)
					.sign_by_set(&ctx.validators),
			)
			.unwrap();
			assert_eq!(storage.best_block().0, header1);
			assert_eq!(storage.finalized_block().number, 0);

			// validator[3] has authored header2 (nothing is finalized yet)
			let header2 = import_custom_block(
				&mut storage,
				&ctx.validators,
				HeaderBuilder::with_parent_number(1)
					.step(3)
					.sign_by_set(&ctx.validators),
			)
			.unwrap();
			assert_eq!(storage.best_block().0, header2);
			assert_eq!(storage.finalized_block().number, 0);

			// validator[4] has authored header3 (header1 is finalized)
			let header3 = import_custom_block(
				&mut storage,
				&ctx.validators,
				HeaderBuilder::with_parent_number(2)
					.step(4)
					.sign_by_set(&ctx.validators),
			)
			.unwrap();
			assert_eq!(storage.best_block().0, header3);
			assert_eq!(storage.finalized_block(), header1);

			// validator[4] has authored 4 blocks: header2'...header5' (header1 is still finalized)
			let header2_1 = import_custom_block(
				&mut storage,
				&ctx.validators,
				HeaderBuilder::with_parent_number(1)
					.gas_limit((GAS_LIMIT + 1).into())
					.step(4)
					.sign_by_set(&ctx.validators),
			)
			.unwrap();
			let header3_1 = import_custom_block(
				&mut storage,
				&ctx.validators,
				HeaderBuilder::with_parent_hash(header2_1.hash)
					.step(4 + VALIDATORS)
					.sign_by_set(&ctx.validators),
			)
			.unwrap();
			let header4_1 = import_custom_block(
				&mut storage,
				&ctx.validators,
				HeaderBuilder::with_parent_hash(header3_1.hash)
					.step(4 + VALIDATORS * 2)
					.sign_by_set(&ctx.validators),
			)
			.unwrap();
			let header5_1 = import_custom_block(
				&mut storage,
				&ctx.validators,
				HeaderBuilder::with_parent_hash(header4_1.hash)
					.step(4 + VALIDATORS * 3)
					.sign_by_set(&ctx.validators),
			)
			.unwrap();
			assert_eq!(storage.best_block().0, header5_1);
			assert_eq!(storage.finalized_block(), header1);

			// when we import header4 { parent = header3 }, authored by validator[0], header2 is finalized
			let header4 = import_custom_block(
				&mut storage,
				&ctx.validators,
				HeaderBuilder::with_parent_number(3)
					.step(5)
					.sign_by_set(&ctx.validators),
			)
			.unwrap();
			assert_eq!(storage.best_block().0, header5_1);
			assert_eq!(storage.finalized_block(), header2);

			// when we import header5 { parent = header4 }, authored by validator[1], header3 is finalized
			let header5 = import_custom_block(
				&mut storage,
				&ctx.validators,
				HeaderBuilder::with_parent_hash(header4.hash)
					.step(6)
					.sign_by_set(&ctx.validators),
			)
			.unwrap();
			assert_eq!(storage.best_block().0, header5);
			assert_eq!(storage.finalized_block(), header3);

			// import of header2'' { parent = header1 } fails, because it has number < best_finalized
			assert_eq!(
				import_custom_block(
					&mut storage,
					&ctx.validators,
					HeaderBuilder::with_parent_number(1)
						.gas_limit((GAS_LIMIT + 1).into())
						.step(3)
						.sign_by_set(&ctx.validators)
				),
				Err(Error::AncientHeader),
			);

			// import of header6' should also fail because we're trying to append to fork thas
			// has forked before finalized block
			assert_eq!(
				import_custom_block(
					&mut storage,
					&ctx.validators,
					HeaderBuilder::with_parent_number(5)
						.gas_limit((GAS_LIMIT + 1).into())
						.step(5 + VALIDATORS * 4)
						.sign_by_set(&ctx.validators),
				),
				Err(Error::TryingToFinalizeSibling),
			);
		});
	}
}
