#![cfg(feature = "test-bpf")]
pub mod common;
pub mod utils;

use common::*;
use utils::setup_functions::*;
use utils::helpers::DirtyClone;

use mpl_testing_utils::{solana::airdrop, utils::Metadata};
use solana_sdk::{compute_budget::ComputeBudgetInstruction, signer::Signer, sysvar};
use std::{assert_eq, time::SystemTime};
use solana_program::instruction::AccountMeta;
use mpl_token_metadata::state::{TokenStandard, PrintSupply};

use mpl_token_metadata::{
    pda::{find_metadata_account, find_token_record_account},
    processor::{AuthorizationData, DelegateScenario, TransferScenario},
    state::{Operation, TokenDelegateRole},
};

use mpl_token_auth_rules::{
    instruction::{builders::CreateOrUpdateBuilder, CreateOrUpdateArgs, InstructionBuilder},
    payload::Payload,
    pda::find_rule_set_address,
    state::{Rule, RuleSetV1},
};

use mpl_auction_house::pda::{
        find_auction_house_address, find_auction_house_fee_account_address,
        find_auction_house_treasury_address, find_auctioneer_pda,
        find_auctioneer_trade_state_address, find_escrow_payment_address,
        find_program_as_signer_address, find_trade_state_address,
};

use mpl_auctioneer::pda::{find_auctioneer_authority_seeds, find_listing_config_address};

use rmp_serde::Serializer;
use serde::Serialize;

#[tokio::test]
async fn sell_success() {
    let mut context = auctioneer_program_test().start_with_context().await;
    // Payer Wallet
    let (ah, ahkey, _) = existing_auction_house_test_context(&mut context)
        .await
        .unwrap();
    let test_metadata = Metadata::new();
    let owner_pubkey = &test_metadata.token.pubkey();
    airdrop(&mut context, owner_pubkey, TEN_SOL).await.unwrap();
    test_metadata
        .create(
            &mut context,
            "Test".to_string(),
            "TST".to_string(),
            "uri".to_string(),
            None,
            10,
            false,
            1,
        )
        .await
        .unwrap();
    let ((acc, _listing_config_address), sell_tx) = sell(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        (SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs()) as i64,
        (SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs()
            + 60) as i64,
        None,
        None,
        None,
        None,
        None,
    );

    context
        .banks_client
        .process_transaction(sell_tx)
        .await
        .unwrap();
    let sts = context
        .banks_client
        .get_account(acc.seller_trade_state)
        .await
        .expect("Error Getting Trade State")
        .expect("Trade State Empty");
    assert_eq!(sts.data.len(), 1);
}

#[tokio::test]
async fn sell_pnft_success() {
    let mut context = auctioneer_program_test().start_with_context().await;
    // Payer Wallet
    let (ah, ahkey, authority) = existing_auction_house_test_context(&mut context)
        .await
        .unwrap();
    let payer = context.payer.dirty_clone();
    let (rule_set, auth_data) = create_sale_delegate_rule_set(&mut context, payer).await;

    let test_metadata = Metadata::new();
    let owner_pubkey = &test_metadata.token.pubkey();
    airdrop(&mut context, owner_pubkey, TEN_SOL).await.unwrap();
    test_metadata
        .create_via_builder(
            &mut context,
            "Test".to_string(),
            "TST".to_string(),
            "uri".to_string(),
            None,
            10,
            false,
            None,
            None,
            true,
            TokenStandard::ProgrammableNonFungible,
            None,
            Some(rule_set),
            Some(0),
            Some(PrintSupply::Zero),
        )
        .await
        .unwrap();

    test_metadata
        .mint_via_builder(&mut context, 1, Some(auth_data))
        .await
        .unwrap();

    let token =
        get_associated_token_address(&test_metadata.token.pubkey(), &test_metadata.mint.pubkey());
    let (seller_trade_state, sts_bump) = find_auctioneer_trade_state_address(
        &test_metadata.token.pubkey(),
        &ahkey,
        &token,
        &ah.treasury_mint,
        &test_metadata.mint.pubkey(),
        1,
    );

    let (free_seller_trade_state, free_sts_bump) = find_trade_state_address(
        &test_metadata.token.pubkey(),
        &ahkey,
        &token,
        &ah.treasury_mint,
        &test_metadata.mint.pubkey(),
        0,
        1,
    );

    let (listing_config_address, _list_bump) = find_listing_config_address(
        &test_metadata.token.pubkey(),
        &ahkey,
        &token,
        &ah.treasury_mint,
        &test_metadata.mint.pubkey(),
        1,
    );

    let (pas, pas_bump) = find_program_as_signer_address();
    let pas_token = get_associated_token_address(&pas, &test_metadata.mint.pubkey());
    let (auctioneer_authority, aa_bump) = find_auctioneer_authority_seeds(&ahkey);
    let (auctioneer_pda, _) = find_auctioneer_pda(&ahkey, &auctioneer_authority);

    let mut accounts = mpl_auctioneer::accounts::AuctioneerSell {
        auction_house_program: mpl_auction_house::id(),
        listing_config: listing_config_address,
        wallet: test_metadata.token.pubkey(),
        token_account: token,
        metadata: test_metadata.pubkey,
        authority: ah.authority,
        auction_house: ahkey,
        auction_house_fee_account: ah.auction_house_fee_account,
        seller_trade_state,
        free_seller_trade_state,
        token_program: spl_token::id(),
        system_program: solana_program::system_program::id(),
        program_as_signer: pas,
        rent: sysvar::rent::id(),
        auctioneer_authority,
        ah_auctioneer_pda: auctioneer_pda,
    }
    .to_account_metas(None);

    let (delegate_record, _) = find_token_record_account(&test_metadata.mint.pubkey(), &pas_token);

    accounts.push(AccountMeta {
        pubkey: mpl_token_metadata::id(),
        is_signer: false,
        is_writable: true,
    });
    accounts.push(AccountMeta {
        pubkey: delegate_record,
        is_signer: false,
        is_writable: true,
    });
    accounts.push(AccountMeta {
        pubkey: test_metadata.token_record,
        is_signer: false,
        is_writable: true,
    });
    accounts.push(AccountMeta {
        pubkey: test_metadata.mint.pubkey(),
        is_signer: false,
        is_writable: false,
    });
    accounts.push(AccountMeta {
        pubkey: test_metadata.master_edition,
        is_signer: false,
        is_writable: false,
    });
    accounts.push(AccountMeta {
        pubkey: mpl_token_auth_rules::id(),
        is_signer: false,
        is_writable: false,
    });
    accounts.push(AccountMeta {
        pubkey: rule_set,
        is_signer: false,
        is_writable: false,
    });
    accounts.push(AccountMeta {
        pubkey: sysvar::instructions::id(),
        is_signer: false,
        is_writable: false,
    });

    let data = mpl_auctioneer::instruction::Sell {
        trade_state_bump: sts_bump,
        free_trade_state_bump: free_sts_bump,
        program_as_signer_bump: pas_bump,
        auctioneer_authority_bump: aa_bump,
        token_size: 1,
        start_time: (SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs()) as i64,
        end_time: (SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs()
            + 60) as i64,
        reserve_price: None,
        min_bid_increment: None,
        time_ext_period: None,
        time_ext_delta: None,
        allow_high_bid_cancel: None,
    }
    .data();

    let instruction = Instruction {
        program_id: mpl_auctioneer::id(),
        data,
        accounts: accounts,
    };

    let compute_ix = ComputeBudgetInstruction::set_compute_unit_limit(350_000);

    let tx = Transaction::new_signed_with_payer(
        &[compute_ix, instruction],
        Some(&authority.pubkey()),
        &[&authority],
        context.last_blockhash,
    );

    context
        .banks_client
        .process_transaction(tx)
        .await
        .unwrap();

    let sts = context
        .banks_client
        .get_account(seller_trade_state)
        .await
        .expect("Error Getting Trade State")
        .expect("Trade State Empty");
    assert_eq!(sts.data.len(), 1);
}