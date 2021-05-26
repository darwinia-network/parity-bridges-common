use crate::{mock::*, *};
use bp_bsc::BSCHeader;

#[test]
fn utils_should_works() {
	let j_h7706000 = r#"{
		"difficulty": "0x2",
		"extraData": "0xd883010100846765746888676f312e31352e35856c696e7578000000fc3ca6b72465176c461afb316ebc773c61faee85a6515daa295e26495cef6f69dfa69911d9d8e4f3bbadb89b29a97c6effb8a411dabc6adeefaa84f5067c8bbe2d4c407bbe49438ed859fe965b140dcf1aab71a93f349bbafec1551819b8be1efea2fc46ca749aa14430b3230294d12c6ab2aac5c2cd68e80b16b581685b1ded8013785d6623cc18d214320b6bb6475970f657164e5b75689b64b7fd1fa275f334f28e1872b61c6014342d914470ec7ac2975be345796c2b7ae2f5b9e386cd1b50a4550696d957cb4900f03a8b6c8fd93d6f4cea42bbb345dbc6f0dfdb5bec739bb832254baf4e8b4cc26bd2b52b31389b56e98b9f8ccdafcc39f3c7d6ebf637c9151673cbc36b88a6f79b60359f141df90a0c745125b131caaffd12b8f7166496996a7da21cf1f1b04d9b3e26a3d077be807dddb074639cd9fa61b47676c064fc50d62cce2fd7544e0b2cc94692d4a704debef7bcb61328e2d3a739effcd3a99387d015e260eefac72ebea1e9ae3261a475a27bb1028f140bc2a7c843318afdea0a6e3c511bbd10f4519ece37dc24887e11b55dee226379db83cffc681495730c11fdde79ba4c0c0670403d7dfc4c816a313885fe04b850f96f27b2e9fd88b147c882ad7caf9b964abfe6543625fcca73b56fe29d3046831574b0681d52bf5383d6f2187b6276c100",
		"gasLimit": "0x38ff37a",
		"gasUsed": "0x1364017",
		"hash": "0x7e1db1179427e17c11a42019f19a3dddf326b6177b0266749639c85c78c607bb",
		"logsBloom": "0x2c30123db854d838c878e978cd2117896aa092e4ce08f078424e9ec7f2312f1909b35e579fb2702d571a3be04a8f01328e51af205100a7c32e3dd8faf8222fcf03f3545655314abf91c4c0d80cea6aa46f122c2a9c596c6a99d5842786d40667eb195877bbbb128890a824506c81a9e5623d4355e08a16f384bf709bf4db598bbcb88150abcd4ceba89cc798000bdccf5cf4d58d50828d3b7dc2bc5d8a928a32d24b845857da0b5bcf2c5dec8230643d4bec452491ba1260806a9e68a4a530de612e5c2676955a17400ce1d4fd6ff458bc38a8b1826e1c1d24b9516ef84ea6d8721344502a6c732ed7f861bb0ea017d520bad5fa53cfc67c678a2e6f6693c8ee",
		"miner": "0xe9ae3261a475a27bb1028f140bc2a7c843318afd",
		"mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
		"nonce": "0x0000000000000000",
		"number": "0x7594c8",
		"parentHash": "0x5cb4b6631001facd57be810d5d1383ee23a31257d2430f097291d25fc1446d4f",
		"receiptsRoot": "0x1bfba16a9e34a12ff7c4b88be484ccd8065b90abea026f6c1f97c257fdb4ad2b",
		"sha3Uncles": "0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347",
		"stateRoot": "0xa6cd7017374dfe102e82d2b3b8a43dbe1d41cc0e4569f3dc45db6c4e687949ae",
		"timestamp": "0x60ac7137",
		"transactionsRoot": "0x657f5876113ac9abe5cf0460aa8d6b3b53abfc336cea4ab3ee594586f8b584ca"
	  }"#;
	let h: BSCHeader = BSCHeader::from_str_unchecked(&j_h7706000);
	let r = utils::recover_creator(&h);
	assert!(r.is_ok());
	assert_eq!(
		format!("{:#x}", r.unwrap()),
		"0x72b61c6014342d914470ec7ac2975be345796c2b"
	);

	let r_signers = utils::extract_authorities(&h);
	assert!(r_signers.is_ok());
	let mut rs: String = "".to_owned();
	for signer in r_signers.unwrap() {
		rs.push_str(&format!("{:#x}", signer));
		println!("{:#x}", signer)
	}
	let mut expected: String = "".to_owned();
	expected.push_str("0x2465176c461afb316ebc773c61faee85a6515daa");
	expected.push_str("0x295e26495cef6f69dfa69911d9d8e4f3bbadb89b");
	expected.push_str("0x29a97c6effb8a411dabc6adeefaa84f5067c8bbe");
	expected.push_str("0x2d4c407bbe49438ed859fe965b140dcf1aab71a9");
	expected.push_str("0x3f349bbafec1551819b8be1efea2fc46ca749aa1");
	expected.push_str("0x4430b3230294d12c6ab2aac5c2cd68e80b16b581");
	expected.push_str("0x685b1ded8013785d6623cc18d214320b6bb64759");
	expected.push_str("0x70f657164e5b75689b64b7fd1fa275f334f28e18");
	expected.push_str("0x72b61c6014342d914470ec7ac2975be345796c2b");
	expected.push_str("0x7ae2f5b9e386cd1b50a4550696d957cb4900f03a");
	expected.push_str("0x8b6c8fd93d6f4cea42bbb345dbc6f0dfdb5bec73");
	expected.push_str("0x9bb832254baf4e8b4cc26bd2b52b31389b56e98b");
	expected.push_str("0x9f8ccdafcc39f3c7d6ebf637c9151673cbc36b88");
	expected.push_str("0xa6f79b60359f141df90a0c745125b131caaffd12");
	expected.push_str("0xb8f7166496996a7da21cf1f1b04d9b3e26a3d077");
	expected.push_str("0xbe807dddb074639cd9fa61b47676c064fc50d62c");
	expected.push_str("0xce2fd7544e0b2cc94692d4a704debef7bcb61328");
	expected.push_str("0xe2d3a739effcd3a99387d015e260eefac72ebea1");
	expected.push_str("0xe9ae3261a475a27bb1028f140bc2a7c843318afd");
	expected.push_str("0xea0a6e3c511bbd10f4519ece37dc24887e11b55d");
	expected.push_str("0xee226379db83cffc681495730c11fdde79ba4c0c");
	assert_eq!(rs, expected);
}

#[test]
fn initialize_storage_should_works() {
	run_test(|ctx| {
		initialize_storage::<TestRuntime>(&ctx.genesis);
	})
}

// #[test]
// fn verify_and_update_authority_set_unsigned_should_not_work() {
// 	let df = BSCHeader::Default();
// 	run_test(|_|{
// 		ctx
// 	})
// }