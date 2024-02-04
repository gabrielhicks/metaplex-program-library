#![cfg(feature = "test-bpf")]
pub mod common;
pub mod utils;

use common::*;
use utils::setup_functions::*;

use anchor_lang::{InstructionData, ToAccountMetas};
use mpl_testing_utils::{solana::airdrop, utils::Metadata};
use solana_sdk::{
    account::Account as SolanaAccount, compute_budget::ComputeBudgetInstruction, signer::Signer,
};

use std::{assert_eq, time::SystemTime};

use solana_program::{
    instruction::{AccountMeta, Instruction},
    system_program, sysvar,
};

use solana_program::program_pack::Pack;

use mpl_token_metadata::{
    pda::find_token_record_account,
    state::{TokenStandard, PrintSupply, Creator},
};
use solana_sdk::{pubkey::Pubkey, signature::Keypair, transaction::Transaction};
use spl_associated_token_account::get_associated_token_address;
use spl_token::state::Account;

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

use crate::utils::helpers::DirtyClone;
use rmp_serde::Serializer;
use serde::Serialize;

#[tokio::test]
async fn execute_sale_early_failure() {
    let mut context = auctioneer_program_test().start_with_context().await;
    // Payer Wallet
    let (ah, ahkey, authority) = existing_auction_house_test_context(&mut context)
        .await
        .unwrap();
    let test_metadata = Metadata::new();
    airdrop(&mut context, &test_metadata.token.pubkey(), 10_000_000_000)
        .await
        .unwrap();
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
    let ((sell_acc, listing_config_address), sell_tx) = sell(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        (SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs()
            - 60) as i64,
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

    let buyer = Keypair::new();
    airdrop(&mut context, &buyer.pubkey(), 10_000_000_000)
        .await
        .unwrap();
    let (bid_acc, buy_tx) = buy(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        &test_metadata.token.pubkey(),
        &buyer,
        &sell_acc.wallet,
        &listing_config_address,
        1_000_000_000,
    );
    context
        .banks_client
        .process_transaction(buy_tx)
        .await
        .unwrap();

    let buyer_token_account =
        get_associated_token_address(&buyer.pubkey(), &test_metadata.mint.pubkey());

    let (auctioneer_authority, aa_bump) = find_auctioneer_authority_seeds(&ahkey);
    let (auctioneer_pda, _) = find_auctioneer_pda(&ahkey, &auctioneer_authority);
    let accounts = mpl_auctioneer::accounts::AuctioneerExecuteSale {
        auction_house_program: mpl_auction_house::id(),
        listing_config: listing_config_address,
        buyer: buyer.pubkey(),
        seller: test_metadata.token.pubkey(),
        auction_house: ahkey,
        metadata: test_metadata.pubkey,
        token_account: sell_acc.token_account,
        authority: ah.authority,
        seller_trade_state: sell_acc.seller_trade_state,
        buyer_trade_state: bid_acc.buyer_trade_state,
        token_program: spl_token::id(),
        free_trade_state: sell_acc.free_seller_trade_state,
        seller_payment_receipt_account: test_metadata.token.pubkey(),
        buyer_receipt_token_account: buyer_token_account,
        escrow_payment_account: bid_acc.escrow_payment_account,
        token_mint: test_metadata.mint.pubkey(),
        auction_house_fee_account: ah.auction_house_fee_account,
        auction_house_treasury: ah.auction_house_treasury,
        treasury_mint: ah.treasury_mint,
        program_as_signer: sell_acc.program_as_signer,
        system_program: system_program::id(),
        ata_program: spl_associated_token_account::id(),
        rent: sysvar::rent::id(),
        auctioneer_authority,
        ah_auctioneer_pda: auctioneer_pda,
    }
    .to_account_metas(None);
    let (_, free_sts_bump) = find_trade_state_address(
        &test_metadata.token.pubkey(),
        &ahkey,
        &sell_acc.token_account,
        &ah.treasury_mint,
        &test_metadata.mint.pubkey(),
        0,
        1,
    );
    let (_, escrow_bump) = find_escrow_payment_address(&ahkey, &buyer.pubkey());
    let (_, pas_bump) = find_program_as_signer_address();

    let instruction = Instruction {
        program_id: mpl_auctioneer::id(),
        data: mpl_auctioneer::instruction::ExecuteSale {
            escrow_payment_bump: escrow_bump,
            free_trade_state_bump: free_sts_bump,
            program_as_signer_bump: pas_bump,
            auctioneer_authority_bump: aa_bump,
            token_size: 1,
            buyer_price: 100_000_000,
        }
        .data(),
        accounts,
    };
    airdrop(&mut context, &ah.auction_house_fee_account, 10_000_000_000)
        .await
        .unwrap();

    let early_tx = Transaction::new_signed_with_payer(
        &[instruction],
        Some(&authority.pubkey()),
        &[&authority],
        context.last_blockhash,
    );
    let buyer_token_before = &context
        .banks_client
        .get_account(buyer_token_account)
        .await
        .unwrap();
    assert!(buyer_token_before.is_none());

    let result = context
        .banks_client
        .process_transaction(early_tx)
        .await
        .unwrap_err();
    assert_error!(result, AUCTION_ACTIVE);
}

#[tokio::test]
async fn execute_sale_success() {
    let mut context = auctioneer_program_test().start_with_context().await;
    // Payer Wallet
    let (ah, ahkey, authority) = existing_auction_house_test_context(&mut context)
        .await
        .unwrap();
    let test_metadata = Metadata::new();
    airdrop(&mut context, &test_metadata.token.pubkey(), 10_000_000_000)
        .await
        .unwrap();
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
    let ((sell_acc, listing_config_address), sell_tx) = sell(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        (SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs()
            - 60) as i64,
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

    let buyer = Keypair::new();
    airdrop(&mut context, &buyer.pubkey(), 10_000_000_000)
        .await
        .unwrap();
    let (bid_acc, buy_tx) = buy(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        &test_metadata.token.pubkey(),
        &buyer,
        &sell_acc.wallet,
        &listing_config_address,
        100_000_000,
    );
    context
        .banks_client
        .process_transaction(buy_tx)
        .await
        .unwrap();
    let buyer_token_account =
        get_associated_token_address(&buyer.pubkey(), &test_metadata.mint.pubkey());

    context.warp_to_slot(120 * 400).unwrap();

    let (auctioneer_authority, _aa_bump) = find_auctioneer_authority_seeds(&ahkey);
    let (auctioneer_pda, _) = find_auctioneer_pda(&ahkey, &auctioneer_authority);
    let accounts = mpl_auctioneer::accounts::AuctioneerExecuteSale {
        auction_house_program: mpl_auction_house::id(),
        listing_config: listing_config_address,
        buyer: buyer.pubkey(),
        seller: test_metadata.token.pubkey(),
        authority: ah.authority,
        auction_house: ahkey,
        metadata: test_metadata.pubkey,
        token_account: sell_acc.token_account,
        seller_trade_state: sell_acc.seller_trade_state,
        buyer_trade_state: bid_acc.buyer_trade_state,
        token_program: spl_token::id(),
        free_trade_state: sell_acc.free_seller_trade_state,
        seller_payment_receipt_account: test_metadata.token.pubkey(),
        buyer_receipt_token_account: buyer_token_account,
        escrow_payment_account: bid_acc.escrow_payment_account,
        token_mint: test_metadata.mint.pubkey(),
        auction_house_fee_account: ah.auction_house_fee_account,
        auction_house_treasury: ah.auction_house_treasury,
        treasury_mint: ah.treasury_mint,
        program_as_signer: sell_acc.program_as_signer,
        system_program: system_program::id(),
        ata_program: spl_associated_token_account::id(),
        rent: sysvar::rent::id(),
        auctioneer_authority,
        ah_auctioneer_pda: auctioneer_pda,
    }
    .to_account_metas(None);
    let (_, free_sts_bump) = find_trade_state_address(
        &test_metadata.token.pubkey(),
        &ahkey,
        &sell_acc.token_account,
        &ah.treasury_mint,
        &test_metadata.mint.pubkey(),
        0,
        1,
    );
    let (_, escrow_bump) = find_escrow_payment_address(&ahkey, &buyer.pubkey());
    let (_, pas_bump) = find_program_as_signer_address();
    let (_, aa_bump) = find_auctioneer_authority_seeds(&ahkey);

    let instruction = Instruction {
        program_id: mpl_auctioneer::id(),
        data: mpl_auctioneer::instruction::ExecuteSale {
            escrow_payment_bump: escrow_bump,
            free_trade_state_bump: free_sts_bump,
            program_as_signer_bump: pas_bump,
            auctioneer_authority_bump: aa_bump,
            token_size: 1,
            buyer_price: 100_000_000,
        }
        .data(),
        accounts,
    };
    airdrop(&mut context, &ah.auction_house_fee_account, 10_000_000_000)
        .await
        .unwrap();

    let compute_ix = ComputeBudgetInstruction::set_compute_unit_limit(350_000);

    let tx = Transaction::new_signed_with_payer(
        &[compute_ix, instruction],
        Some(&authority.pubkey()),
        &[&authority],
        context.last_blockhash,
    );
    let seller_before = context
        .banks_client
        .get_account(test_metadata.token.pubkey())
        .await
        .unwrap()
        .unwrap();
    let buyer_token_before = &context
        .banks_client
        .get_account(buyer_token_account)
        .await
        .unwrap();
    assert!(buyer_token_before.is_none());

    let listing_config_account = context
        .banks_client
        .get_account(listing_config_address)
        .await
        .unwrap()
        .unwrap();

    context.banks_client.process_transaction(tx).await.unwrap();

    let seller_after = context
        .banks_client
        .get_account(test_metadata.token.pubkey())
        .await
        .unwrap()
        .unwrap();
    let buyer_token_after = Account::unpack_from_slice(
        context
            .banks_client
            .get_account(buyer_token_account)
            .await
            .unwrap()
            .unwrap()
            .data
            .as_slice(),
    )
    .unwrap();
    let fee_minus: u64 = 100_000_000 - ((ah.seller_fee_basis_points as u64 * 100_000_000) / 10000);
    assert!(seller_before.lamports < seller_after.lamports);
    assert_eq!(buyer_token_after.amount, 1);

    let rent = context.banks_client.get_rent().await.unwrap();
    let rent_exempt_min: u64 = rent.minimum_balance(listing_config_account.data.len());

    assert_eq!(
        seller_before.lamports + fee_minus + rent_exempt_min,
        seller_after.lamports
    );

    let listing_config_closed = context
        .banks_client
        .get_account(listing_config_address)
        .await
        .unwrap();

    assert!(listing_config_closed.is_none());
}

#[tokio::test]
async fn execute_sale_two_bids_success() {
    let mut context = auctioneer_program_test().start_with_context().await;
    // Payer Wallet
    let (ah, ahkey, authority) = existing_auction_house_test_context(&mut context)
        .await
        .unwrap();
    let test_metadata = Metadata::new();
    airdrop(&mut context, &test_metadata.token.pubkey(), 10_000_000_000)
        .await
        .unwrap();
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
    let ((sell_acc, listing_config_address), sell_tx) = sell(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        (SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs()
            - 60) as i64,
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

    let buyer0 = Keypair::new();
    airdrop(&mut context, &buyer0.pubkey(), 10_000_000_000)
        .await
        .unwrap();
    let (_bid0_acc, buy0_tx) = buy(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        &test_metadata.token.pubkey(),
        &buyer0,
        &sell_acc.wallet,
        &listing_config_address,
        100_000_000,
    );
    context
        .banks_client
        .process_transaction(buy0_tx)
        .await
        .unwrap();
    let _buyer0_token_account =
        get_associated_token_address(&buyer0.pubkey(), &test_metadata.mint.pubkey());

    let buyer1 = Keypair::new();
    airdrop(&mut context, &buyer1.pubkey(), 10_000_000_000)
        .await
        .unwrap();
    let (bid1_acc, buy1_tx) = buy(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        &test_metadata.token.pubkey(),
        &buyer1,
        &sell_acc.wallet,
        &listing_config_address,
        100_000_001,
    );
    context
        .banks_client
        .process_transaction(buy1_tx)
        .await
        .unwrap();
    let buyer1_token_account =
        get_associated_token_address(&buyer1.pubkey(), &test_metadata.mint.pubkey());

    context.warp_to_slot(120 * 400).unwrap();

    let (auctioneer_authority, aa_bump) = find_auctioneer_authority_seeds(&ahkey);
    let (auctioneer_pda, _) = find_auctioneer_pda(&ahkey, &auctioneer_authority);
    let accounts = mpl_auctioneer::accounts::AuctioneerExecuteSale {
        auction_house_program: mpl_auction_house::id(),
        listing_config: listing_config_address,
        buyer: buyer1.pubkey(),
        seller: test_metadata.token.pubkey(),
        authority: ah.authority,
        auction_house: ahkey,
        metadata: test_metadata.pubkey,
        token_account: sell_acc.token_account,
        seller_trade_state: sell_acc.seller_trade_state,
        buyer_trade_state: bid1_acc.buyer_trade_state,
        token_program: spl_token::id(),
        free_trade_state: sell_acc.free_seller_trade_state,
        seller_payment_receipt_account: test_metadata.token.pubkey(),
        buyer_receipt_token_account: buyer1_token_account,
        escrow_payment_account: bid1_acc.escrow_payment_account,
        token_mint: test_metadata.mint.pubkey(),
        auction_house_fee_account: ah.auction_house_fee_account,
        auction_house_treasury: ah.auction_house_treasury,
        treasury_mint: ah.treasury_mint,
        program_as_signer: sell_acc.program_as_signer,
        system_program: system_program::id(),
        ata_program: spl_associated_token_account::id(),
        rent: sysvar::rent::id(),
        auctioneer_authority,
        ah_auctioneer_pda: auctioneer_pda,
    }
    .to_account_metas(None);
    let (_, free_sts_bump) = find_trade_state_address(
        &test_metadata.token.pubkey(),
        &ahkey,
        &sell_acc.token_account,
        &ah.treasury_mint,
        &test_metadata.mint.pubkey(),
        0,
        1,
    );
    let (_, escrow_bump) = find_escrow_payment_address(&ahkey, &buyer1.pubkey());
    let (_, pas_bump) = find_program_as_signer_address();

    let instruction = Instruction {
        program_id: mpl_auctioneer::id(),
        data: mpl_auctioneer::instruction::ExecuteSale {
            escrow_payment_bump: escrow_bump,
            free_trade_state_bump: free_sts_bump,
            program_as_signer_bump: pas_bump,
            auctioneer_authority_bump: aa_bump,
            token_size: 1,
            buyer_price: 100_000_001,
        }
        .data(),
        accounts,
    };
    airdrop(&mut context, &ah.auction_house_fee_account, 10_000_000_000)
        .await
        .unwrap();

    let compute_ix = ComputeBudgetInstruction::set_compute_unit_limit(350_000);

    let tx = Transaction::new_signed_with_payer(
        &[compute_ix, instruction],
        Some(&authority.pubkey()),
        &[&authority],
        context.last_blockhash,
    );
    let seller_before = context
        .banks_client
        .get_account(test_metadata.token.pubkey())
        .await
        .unwrap()
        .unwrap();
    let buyer1_token_before = &context
        .banks_client
        .get_account(buyer1_token_account)
        .await
        .unwrap();
    assert!(buyer1_token_before.is_none());

    let listing_config_account = context
        .banks_client
        .get_account(listing_config_address)
        .await
        .unwrap()
        .unwrap();

    context.banks_client.process_transaction(tx).await.unwrap();

    let seller_after = context
        .banks_client
        .get_account(test_metadata.token.pubkey())
        .await
        .unwrap()
        .unwrap();
    let buyer1_token_after = Account::unpack_from_slice(
        context
            .banks_client
            .get_account(buyer1_token_account)
            .await
            .unwrap()
            .unwrap()
            .data
            .as_slice(),
    )
    .unwrap();
    let fee_minus: u64 = 100_000_001 - ((ah.seller_fee_basis_points as u64 * 100_000_000) / 10000);
    assert!(seller_before.lamports < seller_after.lamports);
    assert_eq!(buyer1_token_after.amount, 1);

    let rent = context.banks_client.get_rent().await.unwrap();
    let rent_exempt_min: u64 = rent.minimum_balance(listing_config_account.data.len());

    assert_eq!(
        seller_before.lamports + fee_minus + rent_exempt_min,
        seller_after.lamports
    );

    let listing_config_closed = context
        .banks_client
        .get_account(listing_config_address)
        .await
        .unwrap();

    assert!(listing_config_closed.is_none());
}

#[tokio::test]
async fn execute_sale_two_bids_failure() {
    let mut context = auctioneer_program_test().start_with_context().await;
    // Payer Wallet
    let (ah, ahkey, authority) = existing_auction_house_test_context(&mut context)
        .await
        .unwrap();
    let test_metadata = Metadata::new();
    airdrop(&mut context, &test_metadata.token.pubkey(), 10_000_000_000)
        .await
        .unwrap();
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
    let ((sell_acc, listing_config_address), sell_tx) = sell(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        (SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs()
            - 60) as i64,
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

    let buyer0 = Keypair::new();
    airdrop(&mut context, &buyer0.pubkey(), 10_000_000_000)
        .await
        .unwrap();
    let (bid0_acc, buy0_tx) = buy(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        &test_metadata.token.pubkey(),
        &buyer0,
        &sell_acc.wallet,
        &listing_config_address,
        100_000_000,
    );
    context
        .banks_client
        .process_transaction(buy0_tx)
        .await
        .unwrap();
    let buyer0_token_account =
        get_associated_token_address(&buyer0.pubkey(), &test_metadata.mint.pubkey());

    let buyer1 = Keypair::new();
    airdrop(&mut context, &buyer1.pubkey(), 10_000_000_000)
        .await
        .unwrap();
    let (_bid1_acc, buy1_tx) = buy(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        &test_metadata.token.pubkey(),
        &buyer1,
        &sell_acc.wallet,
        &listing_config_address,
        100_000_001,
    );
    context
        .banks_client
        .process_transaction(buy1_tx)
        .await
        .unwrap();
    let _buyer1_token_account =
        get_associated_token_address(&buyer1.pubkey(), &test_metadata.mint.pubkey());

    context.warp_to_slot(120 * 400).unwrap();

    let (auctioneer_authority, aa_bump) = find_auctioneer_authority_seeds(&ahkey);
    let (auctioneer_pda, _) = find_auctioneer_pda(&ahkey, &auctioneer_authority);
    let accounts = mpl_auctioneer::accounts::AuctioneerExecuteSale {
        auction_house_program: mpl_auction_house::id(),
        listing_config: listing_config_address,
        buyer: buyer0.pubkey(),
        seller: test_metadata.token.pubkey(),
        authority: ah.authority,
        auction_house: ahkey,
        metadata: test_metadata.pubkey,
        token_account: sell_acc.token_account,
        seller_trade_state: sell_acc.seller_trade_state,
        buyer_trade_state: bid0_acc.buyer_trade_state,
        token_program: spl_token::id(),
        free_trade_state: sell_acc.free_seller_trade_state,
        seller_payment_receipt_account: test_metadata.token.pubkey(),
        buyer_receipt_token_account: buyer0_token_account,
        escrow_payment_account: bid0_acc.escrow_payment_account,
        token_mint: test_metadata.mint.pubkey(),
        auction_house_fee_account: ah.auction_house_fee_account,
        auction_house_treasury: ah.auction_house_treasury,
        treasury_mint: ah.treasury_mint,
        program_as_signer: sell_acc.program_as_signer,
        system_program: system_program::id(),
        ata_program: spl_associated_token_account::id(),
        rent: sysvar::rent::id(),
        auctioneer_authority,
        ah_auctioneer_pda: auctioneer_pda,
    }
    .to_account_metas(None);
    let (_, free_sts_bump) = find_trade_state_address(
        &test_metadata.token.pubkey(),
        &ahkey,
        &sell_acc.token_account,
        &ah.treasury_mint,
        &test_metadata.mint.pubkey(),
        0,
        1,
    );
    let (_, escrow_bump) = find_escrow_payment_address(&ahkey, &buyer0.pubkey());
    let (_, pas_bump) = find_program_as_signer_address();

    let instruction = Instruction {
        program_id: mpl_auctioneer::id(),
        data: mpl_auctioneer::instruction::ExecuteSale {
            escrow_payment_bump: escrow_bump,
            free_trade_state_bump: free_sts_bump,
            program_as_signer_bump: pas_bump,
            auctioneer_authority_bump: aa_bump,
            token_size: 1,
            buyer_price: 100_000_000,
        }
        .data(),
        accounts,
    };
    airdrop(&mut context, &ah.auction_house_fee_account, 10_000_000_000)
        .await
        .unwrap();

    let tx = Transaction::new_signed_with_payer(
        &[instruction],
        Some(&authority.pubkey()),
        &[&authority],
        context.last_blockhash,
    );

    let result = context
        .banks_client
        .process_transaction(tx)
        .await
        .unwrap_err();

    assert_error!(result, NOT_HIGH_BIDDER)
}

#[tokio::test]
async fn execute_sale_one_creator() {
    execute_sale_with_creators(vec![(Pubkey::new_unique(), 100)]).await;
}

#[tokio::test]
async fn execute_sale_two_creator() {
    execute_sale_with_creators(vec![(Pubkey::new_unique(), 25), (Pubkey::new_unique(), 75)]).await;
}

#[tokio::test]
async fn execute_pnft_sale_success() {
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

    let mut sell_acc = mpl_auctioneer::accounts::AuctioneerSell {
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

    sell_acc.push(AccountMeta {
        pubkey: mpl_token_metadata::id(),
        is_signer: false,
        is_writable: true,
    });
    sell_acc.push(AccountMeta {
        pubkey: delegate_record,
        is_signer: false,
        is_writable: true,
    });
    sell_acc.push(AccountMeta {
        pubkey: test_metadata.token_record,
        is_signer: false,
        is_writable: true,
    });
    sell_acc.push(AccountMeta {
        pubkey: test_metadata.mint.pubkey(),
        is_signer: false,
        is_writable: false,
    });
    sell_acc.push(AccountMeta {
        pubkey: test_metadata.master_edition,
        is_signer: false,
        is_writable: false,
    });
    sell_acc.push(AccountMeta {
        pubkey: mpl_token_auth_rules::id(),
        is_signer: false,
        is_writable: false,
    });
    sell_acc.push(AccountMeta {
        pubkey: rule_set,
        is_signer: false,
        is_writable: false,
    });
    sell_acc.push(AccountMeta {
        pubkey: sysvar::instructions::id(),
        is_signer: false,
        is_writable: false,
    });

    let sell_data = mpl_auctioneer::instruction::Sell {
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

    let sell_ix = Instruction {
        program_id: mpl_auctioneer::id(),
        data: sell_data,
        accounts: sell_acc,
    };

    let sell_compute_ix = ComputeBudgetInstruction::set_compute_unit_limit(350_000);

    let sell_tx = Transaction::new_signed_with_payer(
        &[sell_compute_ix, sell_ix],
        Some(&authority.pubkey()),
        &[&authority],
        context.last_blockhash,
    );

    context
        .banks_client
        .process_transaction(sell_tx)
        .await
        .unwrap();

    let buyer = Keypair::new();
    airdrop(&mut context, &buyer.pubkey(), 10_000_000_000)
        .await
        .unwrap();
    let (bid_acc, buy_tx) = buy(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        &test_metadata.token.pubkey(),
        &buyer,
        &test_metadata.token.pubkey(),
        &listing_config_address,
        100_000_000,
    );
    context
        .banks_client
        .process_transaction(buy_tx)
        .await
        .unwrap();
    let buyer_token_account =
        get_associated_token_address(&buyer.pubkey(), &test_metadata.mint.pubkey());

    context.warp_to_slot(120 * 400).unwrap();

    let (auctioneer_authority, _aa_bump) = find_auctioneer_authority_seeds(&ahkey);
    let (auctioneer_pda, _) = find_auctioneer_pda(&ahkey, &auctioneer_authority);
    let mut accounts = mpl_auctioneer::accounts::AuctioneerExecuteSale {
        auction_house_program: mpl_auction_house::id(),
        listing_config: listing_config_address,
        buyer: buyer.pubkey(),
        seller: test_metadata.token.pubkey(),
        authority: ah.authority,
        auction_house: ahkey,
        metadata: test_metadata.pubkey,
        token_account: token,
        seller_trade_state: seller_trade_state,
        buyer_trade_state: bid_acc.buyer_trade_state,
        token_program: spl_token::id(),
        free_trade_state: free_seller_trade_state,
        seller_payment_receipt_account: test_metadata.token.pubkey(),
        buyer_receipt_token_account: buyer_token_account,
        escrow_payment_account: bid_acc.escrow_payment_account,
        token_mint: test_metadata.mint.pubkey(),
        auction_house_fee_account: ah.auction_house_fee_account,
        auction_house_treasury: ah.auction_house_treasury,
        treasury_mint: ah.treasury_mint,
        program_as_signer: pas,
        system_program: system_program::id(),
        ata_program: spl_associated_token_account::id(),
        rent: sysvar::rent::id(),
        auctioneer_authority,
        ah_auctioneer_pda: auctioneer_pda,
    }
    .to_account_metas(None);
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
    let (_, free_sts_bump) = find_trade_state_address(
        &test_metadata.token.pubkey(),
        &ahkey,
        &token,
        &ah.treasury_mint,
        &test_metadata.mint.pubkey(),
        0,
        1,
    );
    let (_, escrow_bump) = find_escrow_payment_address(&ahkey, &buyer.pubkey());
    let (_, pas_bump) = find_program_as_signer_address();
    let (_, aa_bump) = find_auctioneer_authority_seeds(&ahkey);

    let instruction = Instruction {
        program_id: mpl_auctioneer::id(),
        data: mpl_auctioneer::instruction::ExecuteSale {
            escrow_payment_bump: escrow_bump,
            free_trade_state_bump: free_sts_bump,
            program_as_signer_bump: pas_bump,
            auctioneer_authority_bump: aa_bump,
            token_size: 1,
            buyer_price: 100_000_000,
        }
        .data(),
        accounts,
    };
    airdrop(&mut context, &ah.auction_house_fee_account, 10_000_000_000)
        .await
        .unwrap();

    let compute_ix = ComputeBudgetInstruction::set_compute_unit_limit(350_000);

    let tx = Transaction::new_signed_with_payer(
        &[compute_ix, instruction],
        Some(&authority.pubkey()),
        &[&authority],
        context.last_blockhash,
    );
    let seller_before = context
        .banks_client
        .get_account(test_metadata.token.pubkey())
        .await
        .unwrap()
        .unwrap();
    let buyer_token_before = &context
        .banks_client
        .get_account(buyer_token_account)
        .await
        .unwrap();
    assert!(buyer_token_before.is_none());

    let listing_config_account = context
        .banks_client
        .get_account(listing_config_address)
        .await
        .unwrap()
        .unwrap();

    context.banks_client.process_transaction(tx).await.unwrap();

    let seller_after = context
        .banks_client
        .get_account(test_metadata.token.pubkey())
        .await
        .unwrap()
        .unwrap();
    let buyer_token_after = Account::unpack_from_slice(
        context
            .banks_client
            .get_account(buyer_token_account)
            .await
            .unwrap()
            .unwrap()
            .data
            .as_slice(),
    )
    .unwrap();
    let fee_minus: u64 = 100_000_000 - ((ah.seller_fee_basis_points as u64 * 100_000_000) / 10000);
    assert!(seller_before.lamports < seller_after.lamports);
    assert_eq!(buyer_token_after.amount, 1);

    let rent = context.banks_client.get_rent().await.unwrap();
    let rent_exempt_min: u64 = rent.minimum_balance(listing_config_account.data.len());

    assert_eq!(
        seller_before.lamports + fee_minus + rent_exempt_min,
        seller_after.lamports
    );

    let listing_config_closed = context
        .banks_client
        .get_account(listing_config_address)
        .await
        .unwrap();

    assert!(listing_config_closed.is_none());
}

#[tokio::test]
async fn execute_pnft_sale_one_creator() {
    execute_pnft_sale_with_creators(vec![(Pubkey::new_unique(), 100)]).await;
}

#[tokio::test]
async fn execute_pnft_sale_two_creator() {
    execute_pnft_sale_with_creators(vec![(Pubkey::new_unique(), 25), (Pubkey::new_unique(), 75)]).await;
}

async fn execute_sale_with_creators(metadata_creators: Vec<(Pubkey, u8)>) {
    let mut context = auctioneer_program_test().start_with_context().await;
    // Payer Wallet
    let (ah, ahkey, authority) = existing_auction_house_test_context(&mut context)
        .await
        .unwrap();
    let test_metadata = Metadata::new();
    airdrop(&mut context, &test_metadata.token.pubkey(), 10_000_000_000)
        .await
        .unwrap();

    for (creator, _) in &metadata_creators {
        // airdrop 0.1 sol to ensure rent-exempt minimum
        airdrop(&mut context, creator, 100_000_000).await.unwrap();
    }
    test_metadata
        .create(
            &mut context,
            "Test".to_string(),
            "TST".to_string(),
            "uri".to_string(),
            Some(
                metadata_creators
                    .clone()
                    .iter()
                    .map(|(address, share)| Creator {
                        address: *address,
                        verified: false,
                        share: *share,
                    })
                    .collect(),
            ),
            1000,
            false,
            1,
        )
        .await
        .unwrap();
    let ((sell_acc, listing_config_address), sell_tx) = sell(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        (SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs()
            - 60) as i64,
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

    let buyer = Keypair::new();
    airdrop(&mut context, &buyer.pubkey(), 10_000_000_000)
        .await
        .unwrap();
    let (bid_acc, buy_tx) = buy(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        &test_metadata.token.pubkey(),
        &buyer,
        &sell_acc.wallet,
        &listing_config_address,
        100_000_000,
    );
    context
        .banks_client
        .process_transaction(buy_tx)
        .await
        .unwrap();
    let buyer_token_account =
        get_associated_token_address(&buyer.pubkey(), &test_metadata.mint.pubkey());

    context.warp_to_slot(120 * 400).unwrap();

    let (auctioneer_authority, _aa_bump) = find_auctioneer_authority_seeds(&ahkey);
    let (auctioneer_pda, _) = find_auctioneer_pda(&ahkey, &auctioneer_authority);
    let mut accounts = mpl_auctioneer::accounts::AuctioneerExecuteSale {
        auction_house_program: mpl_auction_house::id(),
        listing_config: listing_config_address,
        buyer: buyer.pubkey(),
        seller: test_metadata.token.pubkey(),
        authority: ah.authority,
        auction_house: ahkey,
        metadata: test_metadata.pubkey,
        token_account: sell_acc.token_account,
        seller_trade_state: sell_acc.seller_trade_state,
        buyer_trade_state: bid_acc.buyer_trade_state,
        token_program: spl_token::id(),
        free_trade_state: sell_acc.free_seller_trade_state,
        seller_payment_receipt_account: test_metadata.token.pubkey(),
        buyer_receipt_token_account: buyer_token_account,
        escrow_payment_account: bid_acc.escrow_payment_account,
        token_mint: test_metadata.mint.pubkey(),
        auction_house_fee_account: ah.auction_house_fee_account,
        auction_house_treasury: ah.auction_house_treasury,
        treasury_mint: ah.treasury_mint,
        program_as_signer: sell_acc.program_as_signer,
        system_program: system_program::id(),
        ata_program: spl_associated_token_account::id(),
        rent: sysvar::rent::id(),
        auctioneer_authority,
        ah_auctioneer_pda: auctioneer_pda,
    }
    .to_account_metas(None);
    for (pubkey, _) in &metadata_creators {
        accounts.push(AccountMeta {
            pubkey: *pubkey,
            is_signer: false,
            is_writable: true,
        });
    }

    let (_, free_sts_bump) = find_trade_state_address(
        &test_metadata.token.pubkey(),
        &ahkey,
        &sell_acc.token_account,
        &ah.treasury_mint,
        &test_metadata.mint.pubkey(),
        0,
        1,
    );
    let (_, escrow_bump) = find_escrow_payment_address(&ahkey, &buyer.pubkey());
    let (_, pas_bump) = find_program_as_signer_address();
    let (_, aa_bump) = find_auctioneer_authority_seeds(&ahkey);

    let instruction = Instruction {
        program_id: mpl_auctioneer::id(),
        data: mpl_auctioneer::instruction::ExecuteSale {
            escrow_payment_bump: escrow_bump,
            free_trade_state_bump: free_sts_bump,
            program_as_signer_bump: pas_bump,
            auctioneer_authority_bump: aa_bump,
            token_size: 1,
            buyer_price: 100_000_000,
        }
        .data(),
        accounts,
    };
    airdrop(&mut context, &ah.auction_house_fee_account, 10_000_000_000)
        .await
        .unwrap();

    let compute_ix = ComputeBudgetInstruction::set_compute_unit_limit(350_000);

    let tx = Transaction::new_signed_with_payer(
        &[compute_ix, instruction],
        Some(&authority.pubkey()),
        &[&authority],
        context.last_blockhash,
    );
    let seller_before = context
        .banks_client
        .get_account(test_metadata.token.pubkey())
        .await
        .unwrap()
        .unwrap();
    let mut metadata_creators_before: Vec<SolanaAccount> = Vec::new();
    for (creator, _) in &metadata_creators {
        metadata_creators_before.push(
            context
                .banks_client
                .get_account(*creator)
                .await
                .unwrap()
                .unwrap(),
        );
    }
    let buyer_token_before = &context
        .banks_client
        .get_account(buyer_token_account)
        .await
        .unwrap();
    assert!(buyer_token_before.is_none());

    let listing_config_account = context
        .banks_client
        .get_account(listing_config_address)
        .await
        .unwrap()
        .unwrap();

    context.banks_client.process_transaction(tx).await.unwrap();

    let seller_after = context
        .banks_client
        .get_account(test_metadata.token.pubkey())
        .await
        .unwrap()
        .unwrap();
    let mut metadata_creators_after: Vec<SolanaAccount> = Vec::new();
    for (creator, _) in &metadata_creators {
        metadata_creators_after.push(
            context
                .banks_client
                .get_account(*creator)
                .await
                .unwrap()
                .unwrap(),
        );
    }
    let buyer_token_after = Account::unpack_from_slice(
        context
            .banks_client
            .get_account(buyer_token_account)
            .await
            .unwrap()
            .unwrap()
            .data
            .as_slice(),
    )
    .unwrap();

    let royalty = (test_metadata
        .get_data(&mut context)
        .await
        .data
        .seller_fee_basis_points as u64
        * 100_000_000)
        / 10000;
    let fee_minus: u64 =
        100_000_000 - royalty - ((ah.seller_fee_basis_points as u64 * (100_000_000)) / 10000);
    assert!(seller_before.lamports < seller_after.lamports);
    assert_eq!(buyer_token_after.amount, 1);

    let rent = context.banks_client.get_rent().await.unwrap();
    let rent_exempt_min: u64 = rent.minimum_balance(listing_config_account.data.len());

    for (((_, share), creator_before), creator_after) in metadata_creators
        .iter()
        .zip(metadata_creators_before.iter())
        .zip(metadata_creators_after.iter())
    {
        assert_eq!(
            creator_before.lamports + (royalty * (*share as u64)) / 100,
            creator_after.lamports
        );
    }

    assert_eq!(
        seller_before.lamports + fee_minus + rent_exempt_min,
        seller_after.lamports
    );

    let listing_config_closed = context
        .banks_client
        .get_account(listing_config_address)
        .await
        .unwrap();

    assert!(listing_config_closed.is_none());
}

async fn execute_pnft_sale_with_creators(metadata_creators: Vec<(Pubkey, u8)>) {
    let mut context = auctioneer_program_test().start_with_context().await;
    // Payer Wallet
    let (ah, ahkey, authority) = existing_auction_house_test_context(&mut context)
        .await
        .unwrap();
    let test_metadata = Metadata::new();
    airdrop(&mut context, &test_metadata.token.pubkey(), 10_000_000_000)
        .await
        .unwrap();

    for (creator, _) in &metadata_creators {
        // airdrop 0.1 sol to ensure rent-exempt minimum
        airdrop(&mut context, creator, 100_000_000).await.unwrap();
    }

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
            Some(
                metadata_creators
                    .clone()
                    .iter()
                    .map(|(address, share)| Creator {
                        address: *address,
                        verified: false,
                        share: *share,
                    })
                    .collect(),
            ),
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

    let mut sell_acc = mpl_auctioneer::accounts::AuctioneerSell {
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

    sell_acc.push(AccountMeta {
        pubkey: mpl_token_metadata::id(),
        is_signer: false,
        is_writable: true,
    });
    sell_acc.push(AccountMeta {
        pubkey: delegate_record,
        is_signer: false,
        is_writable: true,
    });
    sell_acc.push(AccountMeta {
        pubkey: test_metadata.token_record,
        is_signer: false,
        is_writable: true,
    });
    sell_acc.push(AccountMeta {
        pubkey: test_metadata.mint.pubkey(),
        is_signer: false,
        is_writable: false,
    });
    sell_acc.push(AccountMeta {
        pubkey: test_metadata.master_edition,
        is_signer: false,
        is_writable: false,
    });
    sell_acc.push(AccountMeta {
        pubkey: mpl_token_auth_rules::id(),
        is_signer: false,
        is_writable: false,
    });
    sell_acc.push(AccountMeta {
        pubkey: rule_set,
        is_signer: false,
        is_writable: false,
    });
    sell_acc.push(AccountMeta {
        pubkey: sysvar::instructions::id(),
        is_signer: false,
        is_writable: false,
    });

    let sell_data = mpl_auctioneer::instruction::Sell {
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

    let sell_ix = Instruction {
        program_id: mpl_auctioneer::id(),
        data: sell_data,
        accounts: sell_acc,
    };

    let sell_compute_ix = ComputeBudgetInstruction::set_compute_unit_limit(350_000);

    let sell_tx = Transaction::new_signed_with_payer(
        &[sell_compute_ix, sell_ix],
        Some(&authority.pubkey()),
        &[&authority],
        context.last_blockhash,
    );

    context
        .banks_client
        .process_transaction(sell_tx)
        .await
        .unwrap();

    let buyer = Keypair::new();
    airdrop(&mut context, &buyer.pubkey(), 10_000_000_000)
        .await
        .unwrap();
    let (bid_acc, buy_tx) = buy(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        &test_metadata.token.pubkey(),
        &buyer,
        &test_metadata.token.pubkey(),
        &listing_config_address,
        100_000_000,
    );
    context
        .banks_client
        .process_transaction(buy_tx)
        .await
        .unwrap();
    let buyer_token_account =
        get_associated_token_address(&buyer.pubkey(), &test_metadata.mint.pubkey());

    context.warp_to_slot(120 * 400).unwrap();

    let (auctioneer_authority, _aa_bump) = find_auctioneer_authority_seeds(&ahkey);
    let (auctioneer_pda, _) = find_auctioneer_pda(&ahkey, &auctioneer_authority);
    let mut accounts = mpl_auctioneer::accounts::AuctioneerExecuteSale {
        auction_house_program: mpl_auction_house::id(),
        listing_config: listing_config_address,
        buyer: buyer.pubkey(),
        seller: test_metadata.token.pubkey(),
        authority: ah.authority,
        auction_house: ahkey,
        metadata: test_metadata.pubkey,
        token_account: token,
        seller_trade_state: seller_trade_state,
        buyer_trade_state: bid_acc.buyer_trade_state,
        token_program: spl_token::id(),
        free_trade_state: free_seller_trade_state,
        seller_payment_receipt_account: test_metadata.token.pubkey(),
        buyer_receipt_token_account: buyer_token_account,
        escrow_payment_account: bid_acc.escrow_payment_account,
        token_mint: test_metadata.mint.pubkey(),
        auction_house_fee_account: ah.auction_house_fee_account,
        auction_house_treasury: ah.auction_house_treasury,
        treasury_mint: ah.treasury_mint,
        program_as_signer: pas,
        system_program: system_program::id(),
        ata_program: spl_associated_token_account::id(),
        rent: sysvar::rent::id(),
        auctioneer_authority,
        ah_auctioneer_pda: auctioneer_pda,
    }
    .to_account_metas(None);
    for (pubkey, _) in &metadata_creators {
        accounts.push(AccountMeta {
            pubkey: *pubkey,
            is_signer: false,
            is_writable: true,
        });
    }
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

    let (_, free_sts_bump) = find_trade_state_address(
        &test_metadata.token.pubkey(),
        &ahkey,
        &token,
        &ah.treasury_mint,
        &test_metadata.mint.pubkey(),
        0,
        1,
    );
    let (_, escrow_bump) = find_escrow_payment_address(&ahkey, &buyer.pubkey());
    let (_, pas_bump) = find_program_as_signer_address();
    let (_, aa_bump) = find_auctioneer_authority_seeds(&ahkey);

    let instruction = Instruction {
        program_id: mpl_auctioneer::id(),
        data: mpl_auctioneer::instruction::ExecuteSale {
            escrow_payment_bump: escrow_bump,
            free_trade_state_bump: free_sts_bump,
            program_as_signer_bump: pas_bump,
            auctioneer_authority_bump: aa_bump,
            token_size: 1,
            buyer_price: 100_000_000,
        }
        .data(),
        accounts,
    };
    airdrop(&mut context, &ah.auction_house_fee_account, 10_000_000_000)
        .await
        .unwrap();

    let compute_ix = ComputeBudgetInstruction::set_compute_unit_limit(350_000);

    let tx = Transaction::new_signed_with_payer(
        &[compute_ix, instruction],
        Some(&authority.pubkey()),
        &[&authority],
        context.last_blockhash,
    );
    let seller_before = context
        .banks_client
        .get_account(test_metadata.token.pubkey())
        .await
        .unwrap()
        .unwrap();
    let mut metadata_creators_before: Vec<SolanaAccount> = Vec::new();
    for (creator, _) in &metadata_creators {
        metadata_creators_before.push(
            context
                .banks_client
                .get_account(*creator)
                .await
                .unwrap()
                .unwrap(),
        );
    }
    let buyer_token_before = &context
        .banks_client
        .get_account(buyer_token_account)
        .await
        .unwrap();
    assert!(buyer_token_before.is_none());

    let listing_config_account = context
        .banks_client
        .get_account(listing_config_address)
        .await
        .unwrap()
        .unwrap();

    context.banks_client.process_transaction(tx).await.unwrap();

    let seller_after = context
        .banks_client
        .get_account(test_metadata.token.pubkey())
        .await
        .unwrap()
        .unwrap();
    let mut metadata_creators_after: Vec<SolanaAccount> = Vec::new();
    for (creator, _) in &metadata_creators {
        metadata_creators_after.push(
            context
                .banks_client
                .get_account(*creator)
                .await
                .unwrap()
                .unwrap(),
        );
    }
    let buyer_token_after = Account::unpack_from_slice(
        context
            .banks_client
            .get_account(buyer_token_account)
            .await
            .unwrap()
            .unwrap()
            .data
            .as_slice(),
    )
    .unwrap();

    let royalty = (test_metadata
        .get_data(&mut context)
        .await
        .data
        .seller_fee_basis_points as u64
        * 100_000_000)
        / 10000;
    let fee_minus: u64 =
        100_000_000 - royalty - ((ah.seller_fee_basis_points as u64 * (100_000_000)) / 10000);
    assert!(seller_before.lamports < seller_after.lamports);
    assert_eq!(buyer_token_after.amount, 1);

    let rent = context.banks_client.get_rent().await.unwrap();
    let rent_exempt_min: u64 = rent.minimum_balance(listing_config_account.data.len());

    for (((_, share), creator_before), creator_after) in metadata_creators
        .iter()
        .zip(metadata_creators_before.iter())
        .zip(metadata_creators_after.iter())
    {
        assert_eq!(
            creator_before.lamports + (royalty * (*share as u64)) / 100,
            creator_after.lamports
        );
    }

    assert_eq!(
        seller_before.lamports + fee_minus + rent_exempt_min,
        seller_after.lamports
    );

    let listing_config_closed = context
        .banks_client
        .get_account(listing_config_address)
        .await
        .unwrap();

    assert!(listing_config_closed.is_none());
}
