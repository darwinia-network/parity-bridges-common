use bp_messages::MessageNonce;
use std::time::Duration;

use codec::Encode;
use frame_support::weights::Weight;
use relay_substrate_client::{
	Chain, ChainBase, ChainWithBalances, ChainWithMessages, SignParam, TransactionSignScheme,
	UnsignedTransaction,
};
use sp_core::{storage::StorageKey, Pair};
use sp_runtime::{generic::SignedPayload, traits::IdentifyAccount};

pub mod runtime;

/// Pangolin header id.
pub type HeaderId = relay_utils::HeaderId<bp_pangolin::Hash, bp_pangolin::BlockNumber>;

/// Rialto header type used in headers sync.
pub type SyncHeader = relay_substrate_client::SyncHeader<bp_pangolin::Header>;

/// Millau chain definition.
#[derive(Debug, Clone, Copy)]
pub struct PangolinChain;

impl ChainBase for PangolinChain {
	type BlockNumber = bp_pangolin::BlockNumber;
	type Hash = bp_pangolin::Hash;
	type Hasher = bp_pangolin::Hashing;
	type Header = bp_pangolin::Header;

	type AccountId = bp_pangolin::AccountId;
	type Balance = bp_pangolin::Balance;
	type Index = bp_pangolin::Nonce;
	type Signature = bp_pangolin::Signature;

	fn max_extrinsic_size() -> u32 {
		bp_pangolin::Pangolin::max_extrinsic_size()
	}

	fn max_extrinsic_weight() -> Weight {
		bp_pangolin::Pangolin::max_extrinsic_weight()
	}
}

impl Chain for PangolinChain {
	const NAME: &'static str = "Pangolin";
	const TOKEN_ID: Option<&'static str> = Some("polkadot");
	const BEST_FINALIZED_HEADER_ID_METHOD: &'static str =
		bp_pangolin::BEST_FINALIZED_PANGOLIN_HEADER_METHOD;
	const AVERAGE_BLOCK_INTERVAL: Duration =
		Duration::from_millis(bp_pangolin::MILLISECS_PER_BLOCK);
	const STORAGE_PROOF_OVERHEAD: u32 = bp_pangolin::EXTRA_STORAGE_PROOF_SIZE;
	const MAXIMAL_ENCODED_ACCOUNT_ID_SIZE: u32 = bp_pangolin::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE;

	type SignedBlock = bp_pangolin::SignedBlock;
	type Call = crate::runtime::Call;
	type WeightToFee = bp_pangolin::WeightToFee;
}

impl ChainWithMessages for PangolinChain {
	const WITH_CHAIN_MESSAGES_PALLET_NAME: &'static str =
		bp_pangolin::WITH_PANGOLIN_MESSAGES_PALLET_NAME;
	const TO_CHAIN_MESSAGE_DETAILS_METHOD: &'static str =
		bp_pangolin::TO_PANGOLIN_MESSAGE_DETAILS_METHOD;
	const TO_CHAIN_LATEST_GENERATED_NONCE_METHOD: &'static str =
		bp_pangolin::TO_PANGOLIN_LATEST_GENERATED_NONCE_METHOD;
	const TO_CHAIN_LATEST_RECEIVED_NONCE_METHOD: &'static str =
		bp_pangolin::TO_PANGOLIN_LATEST_RECEIVED_NONCE_METHOD;
	const FROM_CHAIN_LATEST_RECEIVED_NONCE_METHOD: &'static str =
		bp_pangolin::FROM_PANGOLIN_LATEST_RECEIVED_NONCE_METHOD;
	const FROM_CHAIN_LATEST_CONFIRMED_NONCE_METHOD: &'static str =
		bp_pangolin::FROM_PANGOLIN_LATEST_CONFIRMED_NONCE_METHOD;
	const FROM_CHAIN_UNREWARDED_RELAYERS_STATE: &'static str =
		bp_pangolin::FROM_PANGOLIN_UNREWARDED_RELAYERS_STATE;
	const PAY_INBOUND_DISPATCH_FEE_WEIGHT_AT_CHAIN: Weight =
		bp_pangolin::PAY_INBOUND_DISPATCH_FEE_WEIGHT;
	const MAX_UNREWARDED_RELAYERS_IN_CONFIRMATION_TX: MessageNonce =
		bp_pangolin::MAX_UNREWARDED_RELAYERS_IN_CONFIRMATION_TX;
	const MAX_UNCONFIRMED_MESSAGES_IN_CONFIRMATION_TX: MessageNonce =
		bp_pangolin::MAX_UNCONFIRMED_MESSAGES_IN_CONFIRMATION_TX;
	type WeightInfo = ();
}

impl ChainWithBalances for PangolinChain {
	fn account_info_storage_key(account_id: &Self::AccountId) -> StorageKey {
		StorageKey(bp_pangolin::account_info_storage_key(account_id))
	}
}

impl TransactionSignScheme for PangolinChain {
	type Chain = PangolinChain;
	type AccountKeyPair = sp_core::sr25519::Pair;
	type SignedTransaction = crate::runtime::UncheckedExtrinsic;

	fn sign_transaction(param: SignParam<Self>) -> Self::SignedTransaction {
		let raw_payload = SignedPayload::new(
			param.unsigned.call.clone(),
			bp_pangolin::SignedExtensions::new(
				param.spec_version,
				param.transaction_version,
				param.era,
				param.genesis_hash,
				param.unsigned.nonce,
				param.unsigned.tip,
			),
		)
		.expect("SignedExtension never fails.");

		let signature = raw_payload.using_encoded(|payload| param.signer.sign(payload));
		let signer: sp_runtime::MultiSigner = param.signer.public().into();
		let (call, extra, _) = raw_payload.deconstruct();

		bp_pangolin::UncheckedExtrinsic::new_signed(
			call,
			sp_runtime::MultiAddress::Id(signer.into_account()),
			signature.into(),
			extra,
		)
	}

	fn is_signed(tx: &Self::SignedTransaction) -> bool {
		tx.signature.is_some()
	}

	fn is_signed_by(signer: &Self::AccountKeyPair, tx: &Self::SignedTransaction) -> bool {
		tx.signature
			.as_ref()
			.map(|(address, _, _)| {
				let account_id: bp_pangolin::AccountId = (*signer.public().as_array_ref()).into();
				*address == bp_pangolin::Address::from(account_id)
			})
			.unwrap_or(false)
	}

	fn parse_transaction(tx: Self::SignedTransaction) -> Option<UnsignedTransaction<Self::Chain>> {
		let extra = &tx.signature.as_ref()?.2;
		Some(UnsignedTransaction { call: tx.function, nonce: extra.nonce(), tip: extra.tip() })
	}
}
