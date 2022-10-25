// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! Substrate GRANDPA Pallet
//!
//! This pallet is an on-chain GRANDPA light client for Substrate based chains.
//!
//! This pallet achieves this by trustlessly verifying GRANDPA finality proofs on-chain. Once
//! verified, finalized headers are stored in the pallet, thereby creating a sparse header chain.
//! This sparse header chain can be used as a source of truth for other higher-level applications.
//!
//! The pallet is responsible for tracking GRANDPA validator set hand-offs. We only import headers
//! with justifications signed by the current validator set we know of. The header is inspected for
//! a `ScheduledChanges` digest item, which is then used to update to next validator set.
//!
//! Since this pallet only tracks finalized headers it does not deal with forks. Forks can only
//! occur if the GRANDPA validator set on the bridged chain is either colluding or there is a severe
//! bug causing resulting in an equivocation. Such events are outside the scope of this pallet.
//! Shall the fork occur on the bridged chain governance intervention will be required to
//! re-initialize the bridge and track the right fork.

#![cfg_attr(not(feature = "std"), no_std)]
// Runtime-generated enums
#![allow(clippy::large_enum_variant)]

#[cfg(test)]
mod mock;

/// Module, containing weights for this pallet.
pub mod weights;
pub use weights::WeightInfo;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;

mod extension;

// crates.io
use finality_grandpa::voter_set::VoterSet;
// darwinia-network
use bp_header_chain::{justification::GrandpaJustification, InitializationData};
use bp_runtime::{BlockNumberOf, Chain, HashOf, HasherOf, HeaderOf, OwnedBridgeModule};
// paritytech
use frame_support::{ensure, fail, log};
use frame_system::ensure_signed;
use sp_finality_grandpa::{ConsensusLog, GRANDPA_ENGINE_ID};
use sp_runtime::traits::{Header as HeaderT, Zero};
use sp_std::{boxed::Box, convert::TryInto};

/// The target that will be used when publishing logs related to this pallet.
pub const LOG_TARGET: &str = "runtime::bridge-grandpa";

/// Block number of the bridged chain.
pub type BridgedBlockNumber<T, I> = BlockNumberOf<<T as Config<I>>::BridgedChain>;
/// Block hash of the bridged chain.
pub type BridgedBlockHash<T, I> = HashOf<<T as Config<I>>::BridgedChain>;
/// Hasher of the bridged chain.
pub type BridgedBlockHasher<T, I> = HasherOf<<T as Config<I>>::BridgedChain>;
/// Header of the bridged chain.
pub type BridgedHeader<T, I> = HeaderOf<<T as Config<I>>::BridgedChain>;

#[frame_support::pallet]
pub mod pallet {
	// darwinia-network
	use super::*;
	use bp_runtime::BasicOperatingMode;
	// paritytech
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;

	#[pallet::config]
	pub trait Config<I: 'static = ()>: frame_system::Config {
		/// The chain we are bridging to here.
		type BridgedChain: Chain;

		/// The upper bound on the number of requests allowed by the pallet.
		///
		/// A request refers to an action which writes a header to storage.
		///
		/// Once this bound is reached the pallet will not allow any dispatchables to be called
		/// until the request count has decreased.
		#[pallet::constant]
		type MaxRequests: Get<u32>;

		/// Maximal number of finalized headers to keep in the storage.
		///
		/// The setting is there to prevent growing the on-chain state indefinitely. Note
		/// the setting does not relate to block numbers - we will simply keep as much items
		/// in the storage, so it doesn't guarantee any fixed timeframe for finality headers.
		///
		/// Incautious change of this constant may lead to orphan entries in the runtime storage.
		#[pallet::constant]
		type HeadersToKeep: Get<u32>;

		/// Weights gathered through benchmarking.
		type WeightInfo: WeightInfo;
	}

	#[pallet::pallet]
	#[pallet::without_storage_info]
	pub struct Pallet<T, I = ()>(PhantomData<(T, I)>);

	#[pallet::hooks]
	impl<T: Config<I>, I: 'static> Hooks<BlockNumberFor<T>> for Pallet<T, I> {
		fn on_initialize(_n: T::BlockNumber) -> frame_support::weights::Weight {
			<RequestCount<T, I>>::mutate(|count| *count = count.saturating_sub(1));

			(0_u64)
				.saturating_add(T::DbWeight::get().reads(1))
				.saturating_add(T::DbWeight::get().writes(1))
		}
	}

	impl<T: Config<I>, I: 'static> OwnedBridgeModule<T> for Pallet<T, I> {
		type OperatingMode = BasicOperatingMode;
		type OperatingModeStorage = PalletOperatingMode<T, I>;
		type OwnerStorage = PalletOwner<T, I>;

		const LOG_TARGET: &'static str = LOG_TARGET;
	}

	#[pallet::call]
	impl<T: Config<I>, I: 'static> Pallet<T, I> {
		/// Verify a target header is finalized according to the given finality proof.
		///
		/// It will use the underlying storage pallet to fetch information about the current
		/// authorities and best finalized header in order to verify that the header is finalized.
		///
		/// If successful in verification, it will write the target header to the underlying storage
		/// pallet.
		#[pallet::weight(T::WeightInfo::submit_finality_proof(
			justification.commit.precommits.len().try_into().unwrap_or(u32::MAX),
			justification.votes_ancestries.len().try_into().unwrap_or(u32::MAX),
		))]
		pub fn submit_finality_proof(
			origin: OriginFor<T>,
			finality_target: Box<BridgedHeader<T, I>>,
			justification: GrandpaJustification<BridgedHeader<T, I>>,
		) -> DispatchResultWithPostInfo {
			// TODO: FIX ME https://github.com/paritytech/substrate/pull/10242
			// Self::ensure_not_halted().map_err(Error::<T, I>::BridgeModule)?;
			ensure!(!Self::is_halted(), Error::<T, I>::Halted);

			let _ = ensure_signed(origin)?;

			ensure!(Self::request_count() < T::MaxRequests::get(), <Error<T, I>>::TooManyRequests);

			let (hash, number) = (finality_target.hash(), finality_target.number());
			log::trace!(
				target: LOG_TARGET,
				"Going to try and finalize header {:?}",
				finality_target
			);

			let best_finalized = BestFinalized::<T, I>::get();
			let best_finalized =
				best_finalized.and_then(|(_, hash)| ImportedHeaders::<T, I>::get(hash));
			let best_finalized = match best_finalized {
				Some(best_finalized) => best_finalized,
				None => {
					log::error!(
						target: LOG_TARGET,
						"Cannot finalize header {:?} because pallet is not yet initialized",
						finality_target,
					);
					fail!(<Error<T, I>>::NotInitialized);
				},
			};

			// We do a quick check here to ensure that our header chain is making progress and isn't
			// "travelling back in time" (which could be indicative of something bad, e.g a
			// hard-fork).
			ensure!(best_finalized.number() < number, <Error<T, I>>::OldHeader);

			let authority_set = <CurrentAuthoritySet<T, I>>::get();
			let set_id = authority_set.set_id;
			verify_justification::<T, I>(&justification, hash, *number, authority_set)?;

			let is_authorities_change_enacted =
				try_enact_authority_change::<T, I>(&finality_target, set_id)?;
			<RequestCount<T, I>>::mutate(|count| *count += 1);
			insert_header::<T, I>(*finality_target, hash);
			log::info!(
				target: LOG_TARGET,
				"Successfully imported finalized header with hash {:?}!",
				hash
			);

			// mandatory header is a header that changes authorities set. The pallet can't go
			// further without importing this header. So every bridge MUST import mandatory headers.
			//
			// We don't want to charge extra costs for mandatory operations. So relayer is not
			// paying fee for mandatory headers import transactions.
			let is_mandatory_header = is_authorities_change_enacted;
			let pays_fee = if is_mandatory_header { Pays::No } else { Pays::Yes };

			Ok(pays_fee.into())
		}

		/// Bootstrap the bridge pallet with an initial header and authority set from which to sync.
		///
		/// The initial configuration provided does not need to be the genesis header of the bridged
		/// chain, it can be any arbitrary header. You can also provide the next scheduled set
		/// change if it is already know.
		///
		/// This function is only allowed to be called from a trusted origin and writes to storage
		/// with practically no checks in terms of the validity of the data. It is important that
		/// you ensure that valid data is being passed in.
		#[pallet::weight((T::DbWeight::get().reads_writes(2, 5), DispatchClass::Operational))]
		pub fn initialize(
			origin: OriginFor<T>,
			init_data: super::InitializationData<BridgedHeader<T, I>>,
		) -> DispatchResultWithPostInfo {
			Self::ensure_owner_or_root(origin)?;

			let init_allowed = !<BestFinalized<T, I>>::exists();
			ensure!(init_allowed, <Error<T, I>>::AlreadyInitialized);
			initialize_bridge::<T, I>(init_data.clone());

			log::info!(
				target: LOG_TARGET,
				"Pallet has been initialized with the following parameters: {:?}",
				init_data
			);

			Ok(().into())
		}

		/// Change `PalletOwner`.
		///
		/// May only be called either by root, or by `PalletOwner`.
		#[pallet::weight((T::DbWeight::get().reads_writes(1, 1), DispatchClass::Operational))]
		pub fn set_owner(origin: OriginFor<T>, new_owner: Option<T::AccountId>) -> DispatchResult {
			<Self as OwnedBridgeModule<_>>::set_owner(origin, new_owner)
		}

		/// Halt or resume all pallet operations.
		///
		/// May only be called either by root, or by `PalletOwner`.
		#[pallet::weight((T::DbWeight::get().reads_writes(1, 1), DispatchClass::Operational))]
		pub fn set_operating_mode(
			origin: OriginFor<T>,
			operating_mode: BasicOperatingMode,
		) -> DispatchResult {
			<Self as OwnedBridgeModule<_>>::set_operating_mode(origin, operating_mode)
		}
	}

	/// The current number of requests which have written to storage.
	///
	/// If the `RequestCount` hits `MaxRequests`, no more calls will be allowed to the pallet until
	/// the request capacity is increased.
	///
	/// The `RequestCount` is decreased by one at the beginning of every block. This is to ensure
	/// that the pallet can always make progress.
	#[pallet::storage]
	#[pallet::getter(fn request_count)]
	pub(super) type RequestCount<T: Config<I>, I: 'static = ()> = StorageValue<_, u32, ValueQuery>;

	/// Hash of the header used to bootstrap the pallet.
	#[pallet::storage]
	pub(super) type InitialHash<T: Config<I>, I: 'static = ()> =
		StorageValue<_, BridgedBlockHash<T, I>, ValueQuery>;

	/// Hash of the best finalized header.
	#[pallet::storage]
	pub type BestFinalized<T: Config<I>, I: 'static = ()> =
		StorageValue<_, (BridgedBlockNumber<T, I>, BridgedBlockHash<T, I>), OptionQuery>;

	/// A ring buffer of imported hashes. Ordered by the insertion time.
	#[pallet::storage]
	pub(super) type ImportedHashes<T: Config<I>, I: 'static = ()> =
		StorageMap<_, Identity, u32, BridgedBlockHash<T, I>>;

	/// Current ring buffer position.
	#[pallet::storage]
	pub(super) type ImportedHashesPointer<T: Config<I>, I: 'static = ()> =
		StorageValue<_, u32, ValueQuery>;

	/// Headers which have been imported into the pallet.
	#[pallet::storage]
	pub type ImportedHeaders<T: Config<I>, I: 'static = ()> =
		StorageMap<_, Identity, BridgedBlockHash<T, I>, BridgedHeader<T, I>>;

	/// The current GRANDPA Authority set.
	#[pallet::storage]
	pub(super) type CurrentAuthoritySet<T: Config<I>, I: 'static = ()> =
		StorageValue<_, bp_header_chain::AuthoritySet, ValueQuery>;

	/// Optional pallet owner.
	///
	/// Pallet owner has a right to halt all pallet operations and then resume it. If it is
	/// `None`, then there are no direct ways to halt/resume pallet operations, but other
	/// runtime methods may still be used to do that (i.e. democracy::referendum to update halt
	/// flag directly or call the `halt_operations`).
	#[pallet::storage]
	pub type PalletOwner<T: Config<I>, I: 'static = ()> =
		StorageValue<_, T::AccountId, OptionQuery>;

	/// The current operating mode of the pallet.
	///
	/// Depending on the mode either all, or no transactions will be allowed.
	#[pallet::storage]
	pub type PalletOperatingMode<T: Config<I>, I: 'static = ()> =
		StorageValue<_, BasicOperatingMode, ValueQuery>;

	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config<I>, I: 'static = ()> {
		/// Optional module owner account.
		pub owner: Option<T::AccountId>,
		/// Optional module initialization data.
		pub init_data: Option<super::InitializationData<BridgedHeader<T, I>>>,
	}

	#[cfg(feature = "std")]
	impl<T: Config<I>, I: 'static> Default for GenesisConfig<T, I> {
		fn default() -> Self {
			Self { owner: None, init_data: None }
		}
	}

	#[pallet::genesis_build]
	impl<T: Config<I>, I: 'static> GenesisBuild<T, I> for GenesisConfig<T, I> {
		fn build(&self) {
			if let Some(ref owner) = self.owner {
				<PalletOwner<T, I>>::put(owner);
			}

			if let Some(init_data) = self.init_data.clone() {
				initialize_bridge::<T, I>(init_data);
			} else {
				// Since the bridge hasn't been initialized we shouldn't allow anyone to perform
				// transactions.
				<PalletOperatingMode<T, I>>::put(BasicOperatingMode::Halted);
			}
		}
	}

	#[pallet::error]
	pub enum Error<T, I = ()> {
		/// The given justification is invalid for the given header.
		InvalidJustification,
		/// The authority set from the underlying header chain is invalid.
		InvalidAuthoritySet,
		/// There are too many requests for the current window to handle.
		TooManyRequests,
		/// The header being imported is older than the best finalized header known to the pallet.
		OldHeader,
		/// The header is unknown to the pallet.
		UnknownHeader,
		/// The scheduled authority set change found in the header is unsupported by the pallet.
		///
		/// This is the case for non-standard (e.g forced) authority set changes.
		UnsupportedScheduledChange,
		/// The pallet is not yet initialized.
		NotInitialized,
		/// The pallet has already been initialized.
		AlreadyInitialized,
		/// The storage proof doesn't contains storage root. So it is invalid for given header.
		StorageRootMismatch,
		// TODO: FIX ME https://github.com/paritytech/substrate/pull/10242
		// BridgeModule(bp_runtime::OwnedBridgeModuleError),
		Halted,
	}

	/// Check the given header for a GRANDPA scheduled authority set change. If a change
	/// is found it will be enacted immediately.
	///
	/// This function does not support forced changes, or scheduled changes with delays
	/// since these types of changes are indicative of abnormal behavior from GRANDPA.
	///
	/// Returned value will indicate if a change was enacted or not.
	pub(crate) fn try_enact_authority_change<T: Config<I>, I: 'static>(
		header: &BridgedHeader<T, I>,
		current_set_id: sp_finality_grandpa::SetId,
	) -> Result<bool, sp_runtime::DispatchError> {
		let mut change_enacted = false;

		// We don't support forced changes - at that point governance intervention is required.
		ensure!(
			super::find_forced_change(header).is_none(),
			<Error<T, I>>::UnsupportedScheduledChange
		);

		if let Some(change) = super::find_scheduled_change(header) {
			// GRANDPA only includes a `delay` for forced changes, so this isn't valid.
			ensure!(change.delay == Zero::zero(), <Error<T, I>>::UnsupportedScheduledChange);

			// TODO [#788]: Stop manually increasing the `set_id` here.
			let next_authorities = bp_header_chain::AuthoritySet {
				authorities: change.next_authorities,
				set_id: current_set_id + 1,
			};

			// Since our header schedules a change and we know the delay is 0, it must also enact
			// the change.
			<CurrentAuthoritySet<T, I>>::put(&next_authorities);
			change_enacted = true;

			log::info!(
				target: LOG_TARGET,
				"Transitioned from authority set {} to {}! New authorities are: {:?}",
				current_set_id,
				current_set_id + 1,
				next_authorities,
			);
		};

		Ok(change_enacted)
	}

	/// Verify a GRANDPA justification (finality proof) for a given header.
	///
	/// Will use the GRANDPA current authorities known to the pallet.
	///
	/// If successful it returns the decoded GRANDPA justification so we can refund any weight which
	/// was overcharged in the initial call.
	pub(crate) fn verify_justification<T: Config<I>, I: 'static>(
		justification: &GrandpaJustification<BridgedHeader<T, I>>,
		hash: BridgedBlockHash<T, I>,
		number: BridgedBlockNumber<T, I>,
		authority_set: bp_header_chain::AuthoritySet,
	) -> Result<(), sp_runtime::DispatchError> {
		use bp_header_chain::justification::verify_justification;

		let voter_set =
			VoterSet::new(authority_set.authorities).ok_or(<Error<T, I>>::InvalidAuthoritySet)?;
		let set_id = authority_set.set_id;

		Ok(verify_justification::<BridgedHeader<T, I>>(
			(hash, number),
			set_id,
			&voter_set,
			justification,
		)
		.map_err(|e| {
			log::error!(
				target: LOG_TARGET,
				"Received invalid justification for {:?}: {:?}",
				hash,
				e,
			);
			<Error<T, I>>::InvalidJustification
		})?)
	}

	/// Import a previously verified header to the storage.
	///
	/// Note this function solely takes care of updating the storage and pruning old entries,
	/// but does not verify the validity of such import.
	pub(crate) fn insert_header<T: Config<I>, I: 'static>(
		header: BridgedHeader<T, I>,
		hash: BridgedBlockHash<T, I>,
	) {
		let index = <ImportedHashesPointer<T, I>>::get();
		let pruning = <ImportedHashes<T, I>>::try_get(index);
		<BestFinalized<T, I>>::put((*header.number(), hash));
		<ImportedHeaders<T, I>>::insert(hash, header);
		<ImportedHashes<T, I>>::insert(index, hash);

		// Update ring buffer pointer and remove old header.
		<ImportedHashesPointer<T, I>>::put((index + 1) % T::HeadersToKeep::get());
		if let Ok(hash) = pruning {
			log::debug!(target: LOG_TARGET, "Pruning old header: {:?}.", hash);
			<ImportedHeaders<T, I>>::remove(hash);
		}
	}

	/// Since this writes to storage with no real checks this should only be used in functions that
	/// were called by a trusted origin.
	pub(crate) fn initialize_bridge<T: Config<I>, I: 'static>(
		init_params: super::InitializationData<BridgedHeader<T, I>>,
	) {
		let super::InitializationData { header, authority_list, set_id, operating_mode } =
			init_params;

		let initial_hash = header.hash();
		<InitialHash<T, I>>::put(initial_hash);
		<ImportedHashesPointer<T, I>>::put(0);
		insert_header::<T, I>(*header, initial_hash);

		let authority_set = bp_header_chain::AuthoritySet::new(authority_list, set_id);
		<CurrentAuthoritySet<T, I>>::put(authority_set);

		<PalletOperatingMode<T, I>>::put(operating_mode);
	}

	#[cfg(feature = "runtime-benchmarks")]
	pub(crate) fn bootstrap_bridge<T: Config<I>, I: 'static>(
		init_params: super::InitializationData<BridgedHeader<T, I>>,
	) {
		let start_number = *init_params.header.number();
		let end_number = start_number + T::HeadersToKeep::get().into();
		initialize_bridge::<T, I>(init_params);

		let mut number = start_number;
		while number < end_number {
			number += sp_runtime::traits::One::one();
			let header = <BridgedHeader<T, I>>::new(
				number,
				Default::default(),
				Default::default(),
				Default::default(),
				Default::default(),
			);
			let hash = header.hash();
			insert_header::<T, I>(header, hash);
		}
	}
}
pub use pallet::*;

impl<T: Config<I>, I: 'static> Pallet<T, I> {
	/// Get the best finalized header the pallet knows of.
	///
	/// Returns a dummy header if there is no best header. This can only happen
	/// if the pallet has not been initialized yet.
	pub fn best_finalized() -> Option<BridgedHeader<T, I>> {
		let (_, hash) = <BestFinalized<T, I>>::get()?;
		<ImportedHeaders<T, I>>::get(hash)
	}

	/// Check if a particular header is known to the bridge pallet.
	pub fn is_known_header(hash: BridgedBlockHash<T, I>) -> bool {
		<ImportedHeaders<T, I>>::contains_key(hash)
	}

	/// Verify that the passed storage proof is valid, given it is crafted using
	/// known finalized header. If the proof is valid, then the `parse` callback
	/// is called and the function returns its result.
	pub fn parse_finalized_storage_proof<R>(
		hash: BridgedBlockHash<T, I>,
		storage_proof: sp_trie::StorageProof,
		parse: impl FnOnce(bp_runtime::StorageProofChecker<BridgedBlockHasher<T, I>>) -> R,
	) -> Result<R, sp_runtime::DispatchError> {
		let header = <ImportedHeaders<T, I>>::get(hash).ok_or(Error::<T, I>::UnknownHeader)?;
		let storage_proof_checker =
			bp_runtime::StorageProofChecker::new(*header.state_root(), storage_proof)
				.map_err(|_| Error::<T, I>::StorageRootMismatch)?;

		Ok(parse(storage_proof_checker))
	}
}

pub(crate) fn find_scheduled_change<H: HeaderT>(
	header: &H,
) -> Option<sp_finality_grandpa::ScheduledChange<H::Number>> {
	use sp_runtime::generic::OpaqueDigestItemId;

	let id = OpaqueDigestItemId::Consensus(&GRANDPA_ENGINE_ID);

	let filter_log = |log: ConsensusLog<H::Number>| match log {
		ConsensusLog::ScheduledChange(change) => Some(change),
		_ => None,
	};

	// find the first consensus digest with the right ID which converts to
	// the right kind of consensus log.
	header.digest().convert_first(|l| l.try_to(id).and_then(filter_log))
}

/// Checks the given header for a consensus digest signaling a **forced** scheduled change and
/// extracts it.
pub(crate) fn find_forced_change<H: HeaderT>(
	header: &H,
) -> Option<(H::Number, sp_finality_grandpa::ScheduledChange<H::Number>)> {
	use sp_runtime::generic::OpaqueDigestItemId;

	let id = OpaqueDigestItemId::Consensus(&GRANDPA_ENGINE_ID);

	let filter_log = |log: ConsensusLog<H::Number>| match log {
		ConsensusLog::ForcedChange(delay, change) => Some((delay, change)),
		_ => None,
	};

	// find the first consensus digest with the right ID which converts to
	// the right kind of consensus log.
	header.digest().convert_first(|l| l.try_to(id).and_then(filter_log))
}

/// (Re)initialize bridge with given header for using it in `pallet-bridge-messages` benchmarks.
#[cfg(feature = "runtime-benchmarks")]
pub fn initialize_for_benchmarks<T: Config<I>, I: 'static>(header: BridgedHeader<T, I>) {
	initialize_bridge::<T, I>(InitializationData {
		header: Box::new(header),
		authority_list: sp_std::vec::Vec::new(), /* we don't verify any proofs in external
		                                          * benchmarks */
		set_id: 0,
		operating_mode: bp_runtime::BasicOperatingMode::Normal,
	});
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{run_test, test_header, Origin, TestHeader, TestNumber, TestRuntime};
	use bp_runtime::BasicOperatingMode;
	use bp_test_utils::{
		authority_list, generate_owned_bridge_module_tests, make_default_justification,
		make_justification_for_header, JustificationGeneratorParams, ALICE, BOB,
	};
	use codec::Encode;
	use frame_support::{
		assert_err, assert_noop, assert_ok, storage::generator::StorageValue,
		weights::PostDispatchInfo,
	};
	use sp_runtime::{Digest, DigestItem, DispatchError};

	fn initialize_substrate_bridge() {
		assert_ok!(init_with_origin(Origin::root()));
	}

	fn init_with_origin(
		origin: Origin,
	) -> Result<
		InitializationData<TestHeader>,
		sp_runtime::DispatchErrorWithPostInfo<PostDispatchInfo>,
	> {
		let genesis = test_header(0);

		let init_data = InitializationData {
			header: Box::new(genesis),
			authority_list: authority_list(),
			set_id: 1,
			operating_mode: BasicOperatingMode::Normal,
		};

		Pallet::<TestRuntime>::initialize(origin, init_data.clone()).map(|_| init_data)
	}

	fn submit_finality_proof(header: u8) -> frame_support::dispatch::DispatchResultWithPostInfo {
		let header = test_header(header.into());
		let justification = make_default_justification(&header);

		Pallet::<TestRuntime>::submit_finality_proof(
			Origin::signed(1),
			Box::new(header),
			justification,
		)
	}

	fn next_block() {
		use frame_support::traits::OnInitialize;

		let current_number = frame_system::Pallet::<TestRuntime>::block_number();
		frame_system::Pallet::<TestRuntime>::set_block_number(current_number + 1);
		let _ = Pallet::<TestRuntime>::on_initialize(current_number);
	}

	fn change_log(delay: u64) -> Digest {
		let consensus_log =
			ConsensusLog::<TestNumber>::ScheduledChange(sp_finality_grandpa::ScheduledChange {
				next_authorities: vec![(ALICE.into(), 1), (BOB.into(), 1)],
				delay,
			});

		Digest { logs: vec![DigestItem::Consensus(GRANDPA_ENGINE_ID, consensus_log.encode())] }
	}

	fn forced_change_log(delay: u64) -> Digest {
		let consensus_log = ConsensusLog::<TestNumber>::ForcedChange(
			delay,
			sp_finality_grandpa::ScheduledChange {
				next_authorities: vec![(ALICE.into(), 1), (BOB.into(), 1)],
				delay,
			},
		);

		Digest { logs: vec![DigestItem::Consensus(GRANDPA_ENGINE_ID, consensus_log.encode())] }
	}

	#[test]
	fn init_root_or_owner_origin_can_initialize_pallet() {
		run_test(|| {
			assert_noop!(init_with_origin(Origin::signed(1)), DispatchError::BadOrigin);
			assert_ok!(init_with_origin(Origin::root()));

			// Reset storage so we can initialize the pallet again
			BestFinalized::<TestRuntime>::kill();
			PalletOwner::<TestRuntime>::put(2);
			assert_ok!(init_with_origin(Origin::signed(2)));
		})
	}

	#[test]
	fn init_storage_entries_are_correctly_initialized() {
		run_test(|| {
			assert_eq!(BestFinalized::<TestRuntime>::get(), None,);
			assert_eq!(Pallet::<TestRuntime>::best_finalized(), None);

			let init_data = init_with_origin(Origin::root()).unwrap();

			assert!(<ImportedHeaders<TestRuntime>>::contains_key(init_data.header.hash()));
			assert_eq!(BestFinalized::<TestRuntime>::get().unwrap().1, init_data.header.hash());
			assert_eq!(
				CurrentAuthoritySet::<TestRuntime>::get().authorities,
				init_data.authority_list
			);
			assert_eq!(PalletOperatingMode::<TestRuntime>::get(), BasicOperatingMode::Normal);
		})
	}

	#[test]
	fn init_can_only_initialize_pallet_once() {
		run_test(|| {
			initialize_substrate_bridge();
			assert_noop!(
				init_with_origin(Origin::root()),
				<Error<TestRuntime>>::AlreadyInitialized
			);
		})
	}

	#[test]
	fn pallet_rejects_transactions_if_halted() {
		run_test(|| {
			initialize_substrate_bridge();

			assert_ok!(Pallet::<TestRuntime>::set_operating_mode(
				Origin::root(),
				BasicOperatingMode::Halted
			));
			assert_noop!(
				submit_finality_proof(1),
				// TODO: FIX ME https://github.com/paritytech/substrate/pull/10242
				// Error::<TestRuntime>::BridgeModule(bp_runtime::OwnedBridgeModuleError::Halted)
				Error::<TestRuntime>::Halted
			);

			assert_ok!(Pallet::<TestRuntime>::set_operating_mode(
				Origin::root(),
				BasicOperatingMode::Normal
			));
			assert_ok!(submit_finality_proof(1));
		})
	}

	#[test]
	fn pallet_rejects_header_if_not_initialized_yet() {
		run_test(|| {
			assert_noop!(submit_finality_proof(1), Error::<TestRuntime>::NotInitialized);
		});
	}

	#[test]
	fn succesfully_imports_header_with_valid_finality() {
		run_test(|| {
			initialize_substrate_bridge();
			assert_ok!(
				submit_finality_proof(1),
				PostDispatchInfo {
					actual_weight: None,
					pays_fee: frame_support::weights::Pays::Yes,
				},
			);

			let header = test_header(1);
			assert_eq!(<BestFinalized<TestRuntime>>::get().unwrap().1, header.hash());
			assert!(<ImportedHeaders<TestRuntime>>::contains_key(header.hash()));
		})
	}

	#[test]
	fn rejects_justification_that_skips_authority_set_transition() {
		run_test(|| {
			initialize_substrate_bridge();

			let header = test_header(1);

			let params =
				JustificationGeneratorParams::<TestHeader> { set_id: 2, ..Default::default() };
			let justification = make_justification_for_header(params);

			assert_err!(
				Pallet::<TestRuntime>::submit_finality_proof(
					Origin::signed(1),
					Box::new(header),
					justification,
				),
				<Error<TestRuntime>>::InvalidJustification
			);
		})
	}

	#[test]
	fn does_not_import_header_with_invalid_finality_proof() {
		run_test(|| {
			initialize_substrate_bridge();

			let header = test_header(1);
			let mut justification = make_default_justification(&header);
			justification.round = 42;

			assert_err!(
				Pallet::<TestRuntime>::submit_finality_proof(
					Origin::signed(1),
					Box::new(header),
					justification,
				),
				<Error<TestRuntime>>::InvalidJustification
			);
		})
	}

	#[test]
	fn disallows_invalid_authority_set() {
		run_test(|| {
			let genesis = test_header(0);

			let invalid_authority_list = vec![(ALICE.into(), u64::MAX), (BOB.into(), u64::MAX)];
			let init_data = InitializationData {
				header: Box::new(genesis),
				authority_list: invalid_authority_list,
				set_id: 1,
				operating_mode: BasicOperatingMode::Normal,
			};

			assert_ok!(Pallet::<TestRuntime>::initialize(Origin::root(), init_data));

			let header = test_header(1);
			let justification = make_default_justification(&header);

			assert_err!(
				Pallet::<TestRuntime>::submit_finality_proof(
					Origin::signed(1),
					Box::new(header),
					justification,
				),
				<Error<TestRuntime>>::InvalidAuthoritySet
			);
		})
	}

	#[test]
	fn importing_header_ensures_that_chain_is_extended() {
		run_test(|| {
			initialize_substrate_bridge();

			assert_ok!(submit_finality_proof(4));
			assert_err!(submit_finality_proof(3), Error::<TestRuntime>::OldHeader);
			assert_ok!(submit_finality_proof(5));
		})
	}

	#[test]
	fn importing_header_enacts_new_authority_set() {
		run_test(|| {
			initialize_substrate_bridge();

			let next_set_id = 2;
			let next_authorities = vec![(ALICE.into(), 1), (BOB.into(), 1)];

			// Need to update the header digest to indicate that our header signals an authority set
			// change. The change will be enacted when we import our header.
			let mut header = test_header(2);
			header.digest = change_log(0);

			// Create a valid justification for the header
			let justification = make_default_justification(&header);

			// Let's import our test header
			assert_ok!(
				Pallet::<TestRuntime>::submit_finality_proof(
					Origin::signed(1),
					Box::new(header.clone()),
					justification
				),
				PostDispatchInfo {
					actual_weight: None,
					pays_fee: frame_support::weights::Pays::No,
				},
			);

			// Make sure that our header is the best finalized
			assert_eq!(<BestFinalized<TestRuntime>>::get().unwrap().1, header.hash());
			assert!(<ImportedHeaders<TestRuntime>>::contains_key(header.hash()));

			// Make sure that the authority set actually changed upon importing our header
			assert_eq!(
				<CurrentAuthoritySet<TestRuntime>>::get(),
				bp_header_chain::AuthoritySet::new(next_authorities, next_set_id),
			);
		})
	}

	#[test]
	fn importing_header_rejects_header_with_scheduled_change_delay() {
		run_test(|| {
			initialize_substrate_bridge();

			// Need to update the header digest to indicate that our header signals an authority set
			// change. However, the change doesn't happen until the next block.
			let mut header = test_header(2);
			header.digest = change_log(1);

			// Create a valid justification for the header
			let justification = make_default_justification(&header);

			// Should not be allowed to import this header
			assert_err!(
				Pallet::<TestRuntime>::submit_finality_proof(
					Origin::signed(1),
					Box::new(header),
					justification
				),
				<Error<TestRuntime>>::UnsupportedScheduledChange
			);
		})
	}

	#[test]
	fn importing_header_rejects_header_with_forced_changes() {
		run_test(|| {
			initialize_substrate_bridge();

			// Need to update the header digest to indicate that it signals a forced authority set
			// change.
			let mut header = test_header(2);
			header.digest = forced_change_log(0);

			// Create a valid justification for the header
			let justification = make_default_justification(&header);

			// Should not be allowed to import this header
			assert_err!(
				Pallet::<TestRuntime>::submit_finality_proof(
					Origin::signed(1),
					Box::new(header),
					justification
				),
				<Error<TestRuntime>>::UnsupportedScheduledChange
			);
		})
	}

	#[test]
	fn parse_finalized_storage_proof_rejects_proof_on_unknown_header() {
		run_test(|| {
			assert_noop!(
				Pallet::<TestRuntime>::parse_finalized_storage_proof(
					Default::default(),
					sp_trie::StorageProof::new(vec![]),
					|_| (),
				),
				Error::<TestRuntime>::UnknownHeader,
			);
		});
	}

	#[test]
	fn parse_finalized_storage_accepts_valid_proof() {
		run_test(|| {
			let (state_root, storage_proof) = bp_runtime::craft_valid_storage_proof();

			let mut header = test_header(2);
			header.set_state_root(state_root);

			let hash = header.hash();
			<BestFinalized<TestRuntime>>::put((2, hash));
			<ImportedHeaders<TestRuntime>>::insert(hash, header);

			assert_ok!(
				Pallet::<TestRuntime>::parse_finalized_storage_proof(hash, storage_proof, |_| (),),
				(),
			);
		});
	}

	#[test]
	fn rate_limiter_disallows_imports_once_limit_is_hit_in_single_block() {
		run_test(|| {
			initialize_substrate_bridge();

			assert_ok!(submit_finality_proof(1));
			assert_ok!(submit_finality_proof(2));
			assert_err!(submit_finality_proof(3), <Error<TestRuntime>>::TooManyRequests);
		})
	}

	#[test]
	fn rate_limiter_invalid_requests_do_not_count_towards_request_count() {
		run_test(|| {
			let submit_invalid_request = || {
				let header = test_header(1);
				let mut invalid_justification = make_default_justification(&header);
				invalid_justification.round = 42;

				Pallet::<TestRuntime>::submit_finality_proof(
					Origin::signed(1),
					Box::new(header),
					invalid_justification,
				)
			};

			initialize_substrate_bridge();

			for _ in 0..<TestRuntime as Config>::MaxRequests::get() + 1 {
				// Notice that the error here *isn't* `TooManyRequests`
				assert_err!(submit_invalid_request(), <Error<TestRuntime>>::InvalidJustification);
			}

			// Can still submit `MaxRequests` requests afterwards
			assert_ok!(submit_finality_proof(1));
			assert_ok!(submit_finality_proof(2));
			assert_err!(submit_finality_proof(3), <Error<TestRuntime>>::TooManyRequests);
		})
	}

	#[test]
	fn rate_limiter_allows_request_after_new_block_has_started() {
		run_test(|| {
			initialize_substrate_bridge();
			assert_ok!(submit_finality_proof(1));
			assert_ok!(submit_finality_proof(2));

			next_block();
			assert_ok!(submit_finality_proof(3));
		})
	}

	#[test]
	fn rate_limiter_disallows_imports_once_limit_is_hit_across_different_blocks() {
		run_test(|| {
			initialize_substrate_bridge();
			assert_ok!(submit_finality_proof(1));
			assert_ok!(submit_finality_proof(2));

			next_block();
			assert_ok!(submit_finality_proof(3));
			assert_err!(submit_finality_proof(4), <Error<TestRuntime>>::TooManyRequests);
		})
	}

	#[test]
	fn rate_limiter_allows_max_requests_after_long_time_with_no_activity() {
		run_test(|| {
			initialize_substrate_bridge();
			assert_ok!(submit_finality_proof(1));
			assert_ok!(submit_finality_proof(2));

			next_block();
			next_block();

			next_block();
			assert_ok!(submit_finality_proof(5));
			assert_ok!(submit_finality_proof(7));
		})
	}

	#[test]
	fn should_prune_headers_over_headers_to_keep_parameter() {
		run_test(|| {
			initialize_substrate_bridge();
			assert_ok!(submit_finality_proof(1));
			let first_header = Pallet::<TestRuntime>::best_finalized().unwrap();
			next_block();

			assert_ok!(submit_finality_proof(2));
			next_block();
			assert_ok!(submit_finality_proof(3));
			next_block();
			assert_ok!(submit_finality_proof(4));
			next_block();
			assert_ok!(submit_finality_proof(5));
			next_block();

			assert_ok!(submit_finality_proof(6));

			assert!(
				!Pallet::<TestRuntime>::is_known_header(first_header.hash()),
				"First header should be pruned."
			);
		})
	}

	#[test]
	fn storage_keys_computed_properly() {
		assert_eq!(
			PalletOperatingMode::<TestRuntime>::storage_value_final_key().to_vec(),
			bp_header_chain::storage_keys::pallet_operating_mode_key("Grandpa").0,
		);

		assert_eq!(
			BestFinalized::<TestRuntime>::storage_value_final_key().to_vec(),
			bp_header_chain::storage_keys::best_finalized_key("Grandpa").0,
		);
	}

	generate_owned_bridge_module_tests!(BasicOperatingMode::Normal, BasicOperatingMode::Halted);
}
