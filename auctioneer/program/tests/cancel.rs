#![cfg(feature = "test-bpf")]
pub mod common;
pub mod utils;

use common::*;
use mpl_auctioneer::pda::*;
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction,
    signature::Keypair
};
use std::time::SystemTime;
use utils::setup_functions::*;

use solana_program::{
    instruction::{AccountMeta, Instruction},
    system_program, sysvar,
};

use mpl_token_metadata::{
    pda::find_token_record_account,
    state::{TokenStandard, PrintSupply}
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

use crate::utils::helpers::DirtyClone;
use rmp_serde::Serializer;
use serde::Serialize;

#[tokio::test]
async fn cancel_listing() {
    let mut context = auctioneer_program_test().start_with_context().await;
    // Payer Wallet
    let (ah, ahkey, _) = existing_auction_house_test_context(&mut context)
        .await
        .unwrap();
    let test_metadata = Metadata::new();
    airdrop(
        &mut context,
        &test_metadata.token.pubkey(),
        100_000_000_000_000,
    )
    .await
    .unwrap();
    test_metadata
        .create(
            &mut context,
            "Tests".to_string(),
            "TST".to_string(),
            "uri".to_string(),
            None,
            10,
            false,
            1,
        )
        .await
        .unwrap();
    context.warp_to_slot(100).unwrap();
    // Derive Auction House Key
    let ((acc, listing_config_address), sell_tx) = sell(
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
    let token =
        get_associated_token_address(&test_metadata.token.pubkey(), &test_metadata.mint.pubkey());
    let (auctioneer_authority, aa_bump) = find_auctioneer_authority_seeds(&ahkey);
    let (auctioneer_pda, _) = find_auctioneer_pda(&ahkey, &auctioneer_authority);
    let accounts = mpl_auctioneer::accounts::AuctioneerCancel {
        auction_house_program: mpl_auction_house::id(),
        listing_config: listing_config_address,
        seller: acc.wallet,
        auction_house: ahkey,
        wallet: test_metadata.token.pubkey(),
        token_account: token,
        authority: ah.authority,
        trade_state: acc.seller_trade_state,
        token_program: spl_token::id(),
        token_mint: test_metadata.mint.pubkey(),
        auction_house_fee_account: ah.auction_house_fee_account,
        auctioneer_authority,
        ah_auctioneer_pda: auctioneer_pda,
    }
    .to_account_metas(None);
    let instruction = Instruction {
        program_id: mpl_auctioneer::id(),
        data: mpl_auctioneer::instruction::Cancel {
            auctioneer_authority_bump: aa_bump,
            buyer_price: u64::MAX,
            token_size: 1,
        }
        .data(),
        accounts,
    };

    let tx = Transaction::new_signed_with_payer(
        &[instruction],
        Some(&test_metadata.token.pubkey()),
        &[&test_metadata.token],
        context.last_blockhash,
    );

    context.banks_client.process_transaction(tx).await.unwrap();

    let listing_config_closed = context
        .banks_client
        .get_account(listing_config_address)
        .await
        .unwrap();

    assert!(listing_config_closed.is_none());
}

#[tokio::test]
async fn cancel_bid() {
    let mut context = auctioneer_program_test().start_with_context().await;
    // Payer Wallet
    let (ah, ahkey, _) = existing_auction_house_test_context(&mut context)
        .await
        .unwrap();
    let test_metadata = Metadata::new();
    airdrop(&mut context, &test_metadata.token.pubkey(), 1000000000)
        .await
        .unwrap();
    test_metadata
        .create(
            &mut context,
            "Tests".to_string(),
            "TST".to_string(),
            "uri".to_string(),
            None,
            10,
            false,
            1,
        )
        .await
        .unwrap();

    let price = 1000000000;

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
        Some(true),
    );
    context
        .banks_client
        .process_transaction(sell_tx)
        .await
        .unwrap();

    context.warp_to_slot(100).unwrap();
    let buyer = Keypair::new();
    // Derive Auction House Key
    airdrop(&mut context, &buyer.pubkey(), 2000000000)
        .await
        .unwrap();
    let (acc, buy_tx) = buy(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        &test_metadata.token.pubkey(),
        &buyer,
        &test_metadata.token.pubkey(),
        &listing_config_address,
        price,
    );

    context
        .banks_client
        .process_transaction(buy_tx)
        .await
        .unwrap();
    let (auctioneer_authority, aa_bump) = find_auctioneer_authority_seeds(&ahkey);
    let (auctioneer_pda, _) = find_auctioneer_pda(&ahkey, &auctioneer_authority);
    let accounts = mpl_auctioneer::accounts::AuctioneerCancel {
        auction_house_program: mpl_auction_house::id(),
        listing_config: listing_config_address,
        seller: test_metadata.token.pubkey(),
        auction_house: ahkey,
        wallet: buyer.pubkey(),
        token_account: acc.token_account,
        authority: ah.authority,
        trade_state: acc.buyer_trade_state,
        token_program: spl_token::id(),
        token_mint: test_metadata.mint.pubkey(),
        auction_house_fee_account: ah.auction_house_fee_account,
        auctioneer_authority,
        ah_auctioneer_pda: auctioneer_pda,
    }
    .to_account_metas(None);
    let instruction = Instruction {
        program_id: mpl_auctioneer::id(),
        data: mpl_auctioneer::instruction::Cancel {
            auctioneer_authority_bump: aa_bump,
            buyer_price: price,
            token_size: 1,
        }
        .data(),
        accounts,
    };

    let tx = Transaction::new_signed_with_payer(
        &[instruction],
        Some(&buyer.pubkey()),
        &[&buyer],
        context.last_blockhash,
    );
    context.banks_client.process_transaction(tx).await.unwrap();

    // Make sure the trade state wasn't erroneously closed.
    let listing_config_closed = context
        .banks_client
        .get_account(listing_config_address)
        .await
        .unwrap();

    assert!(listing_config_closed.is_some());
}

#[tokio::test]
async fn cancel_highest_bid() {
    let mut context = auctioneer_program_test().start_with_context().await;
    // Payer Wallet
    let (ah, ahkey, _) = existing_auction_house_test_context(&mut context)
        .await
        .unwrap();
    let test_metadata = Metadata::new();
    airdrop(&mut context, &test_metadata.token.pubkey(), 1000000000)
        .await
        .unwrap();
    test_metadata
        .create(
            &mut context,
            "Tests".to_string(),
            "TST".to_string(),
            "uri".to_string(),
            None,
            10,
            false,
            1,
        )
        .await
        .unwrap();

    let price = 1000000000;

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
        Some(false),
    );
    context
        .banks_client
        .process_transaction(sell_tx)
        .await
        .unwrap();

    context.warp_to_slot(100).unwrap();
    let buyer0 = Keypair::new();
    // Derive Auction House Key
    airdrop(&mut context, &buyer0.pubkey(), 2000000000)
        .await
        .unwrap();
    let (acc0, buy_tx0) = buy(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        &test_metadata.token.pubkey(),
        &buyer0,
        &test_metadata.token.pubkey(),
        &listing_config_address,
        price,
    );

    context
        .banks_client
        .process_transaction(buy_tx0)
        .await
        .unwrap();

    context.warp_to_slot(200).unwrap();

    let (auctioneer_authority, aa_bump) = find_auctioneer_authority_seeds(&ahkey);
    let (auctioneer_pda, _) = find_auctioneer_pda(&ahkey, &auctioneer_authority);
    let accounts0 = mpl_auctioneer::accounts::AuctioneerCancel {
        auction_house_program: mpl_auction_house::id(),
        listing_config: listing_config_address,
        seller: test_metadata.token.pubkey(),
        auction_house: ahkey,
        wallet: buyer0.pubkey(),
        token_account: acc0.token_account,
        authority: ah.authority,
        trade_state: acc0.buyer_trade_state,
        token_program: spl_token::id(),
        token_mint: test_metadata.mint.pubkey(),
        auction_house_fee_account: ah.auction_house_fee_account,
        auctioneer_authority,
        ah_auctioneer_pda: auctioneer_pda,
    }
    .to_account_metas(None);
    let instruction0 = Instruction {
        program_id: mpl_auctioneer::id(),
        data: mpl_auctioneer::instruction::Cancel {
            auctioneer_authority_bump: aa_bump,
            buyer_price: price,
            token_size: 1,
        }
        .data(),
        accounts: accounts0,
    };

    let tx0 = Transaction::new_signed_with_payer(
        &[instruction0],
        Some(&buyer0.pubkey()),
        &[&buyer0],
        context.last_blockhash,
    );
    let result0 = context
        .banks_client
        .process_transaction(tx0)
        .await
        .unwrap_err();
    assert_error!(result0, CANNOT_CANCEL_HIGHEST_BID);

    context.warp_to_slot(300).unwrap();

    // Buyer 1 bids higher and should now be the highest bidder.
    let buyer1 = Keypair::new();
    // Derive Auction House Key
    airdrop(&mut context, &buyer1.pubkey(), 2000000000)
        .await
        .unwrap();
    let (acc1, buy_tx1) = buy(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        &test_metadata.token.pubkey(),
        &buyer1,
        &test_metadata.token.pubkey(),
        &listing_config_address,
        price + 1,
    );

    context
        .banks_client
        .process_transaction(buy_tx1)
        .await
        .unwrap();
    context.warp_to_slot(400).unwrap();

    let (auctioneer_authority, aa_bump) = find_auctioneer_authority_seeds(&ahkey);
    let (auctioneer_pda, _) = find_auctioneer_pda(&ahkey, &auctioneer_authority);
    let accounts1 = mpl_auctioneer::accounts::AuctioneerCancel {
        auction_house_program: mpl_auction_house::id(),
        listing_config: listing_config_address,
        seller: test_metadata.token.pubkey(),
        auction_house: ahkey,
        wallet: buyer1.pubkey(),
        token_account: acc1.token_account,
        authority: ah.authority,
        trade_state: acc1.buyer_trade_state,
        token_program: spl_token::id(),
        token_mint: test_metadata.mint.pubkey(),
        auction_house_fee_account: ah.auction_house_fee_account,
        auctioneer_authority,
        ah_auctioneer_pda: auctioneer_pda,
    }
    .to_account_metas(None);
    let instruction1 = Instruction {
        program_id: mpl_auctioneer::id(),
        data: mpl_auctioneer::instruction::Cancel {
            auctioneer_authority_bump: aa_bump,
            buyer_price: price + 1,
            token_size: 1,
        }
        .data(),
        accounts: accounts1,
    };

    let tx1 = Transaction::new_signed_with_payer(
        &[instruction1],
        Some(&buyer1.pubkey()),
        &[&buyer1],
        context.last_blockhash,
    );

    let result1 = context
        .banks_client
        .process_transaction(tx1)
        .await
        .unwrap_err();
    assert_error!(result1, CANNOT_CANCEL_HIGHEST_BID);
    context.warp_to_slot(500).unwrap();

    // Rerun the cancel on the lower bid to verify it now succeeds.
    let (auctioneer_authority, aa_bump) = find_auctioneer_authority_seeds(&ahkey);
    let (auctioneer_pda, _) = find_auctioneer_pda(&ahkey, &auctioneer_authority);
    let accounts2 = mpl_auctioneer::accounts::AuctioneerCancel {
        auction_house_program: mpl_auction_house::id(),
        listing_config: listing_config_address,
        seller: test_metadata.token.pubkey(),
        auction_house: ahkey,
        wallet: buyer0.pubkey(),
        token_account: acc0.token_account,
        authority: ah.authority,
        trade_state: acc0.buyer_trade_state,
        token_program: spl_token::id(),
        token_mint: test_metadata.mint.pubkey(),
        auction_house_fee_account: ah.auction_house_fee_account,
        auctioneer_authority,
        ah_auctioneer_pda: auctioneer_pda,
    }
    .to_account_metas(None);
    let instruction2 = Instruction {
        program_id: mpl_auctioneer::id(),
        data: mpl_auctioneer::instruction::Cancel {
            auctioneer_authority_bump: aa_bump,
            buyer_price: price,
            token_size: 1,
        }
        .data(),
        accounts: accounts2,
    };

    let tx2 = Transaction::new_signed_with_payer(
        &[instruction2],
        Some(&buyer0.pubkey()),
        &[&buyer0],
        context.last_blockhash,
    );
    context.banks_client.process_transaction(tx2).await.unwrap();
}

#[tokio::test]
async fn cancel_pnft_listing() {
    let mut context = auctioneer_program_test().start_with_context().await;
    // Payer Wallet
    let (ah, ahkey, authority) = existing_auction_house_test_context(&mut context)
        .await
        .unwrap();
    let test_metadata = Metadata::new();
    airdrop(
        &mut context,
        &test_metadata.token.pubkey(),
        100_000_000_000_000,
    )
    .await
    .unwrap();
    let payer = context.payer.dirty_clone();
    let (rule_set, auth_data) = create_sale_delegate_rule_set(&mut context, payer).await;

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

    let (auctioneer_authority, aa_bump) = find_auctioneer_authority_seeds(&ahkey);
    let (auctioneer_pda, _) = find_auctioneer_pda(&ahkey, &auctioneer_authority);
    let mut accounts = mpl_auctioneer::accounts::AuctioneerCancel {
        auction_house_program: mpl_auction_house::id(),
        listing_config: listing_config_address,
        seller: test_metadata.token.pubkey(),
        auction_house: ahkey,
        wallet: test_metadata.token.pubkey(),
        token_account: token,
        authority: ah.authority,
        trade_state: seller_trade_state,
        token_program: spl_token::id(),
        token_mint: test_metadata.mint.pubkey(),
        auction_house_fee_account: ah.auction_house_fee_account,
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

    let instruction = Instruction {
        program_id: mpl_auctioneer::id(),
        data: mpl_auctioneer::instruction::Cancel {
            auctioneer_authority_bump: aa_bump,
            buyer_price: u64::MAX,
            token_size: 1,
        }
        .data(),
        accounts,
    };

    let tx = Transaction::new_signed_with_payer(
        &[instruction],
        Some(&test_metadata.token.pubkey()),
        &[&test_metadata.token],
        context.last_blockhash,
    );

    context.banks_client.process_transaction(tx).await.unwrap();

    let listing_config_closed = context
        .banks_client
        .get_account(listing_config_address)
        .await
        .unwrap();

    assert!(listing_config_closed.is_none());
}

#[tokio::test]
async fn cancel_pnft_bid() {
    let mut context = auctioneer_program_test().start_with_context().await;
    // Payer Wallet
    let (ah, ahkey, authority) = existing_auction_house_test_context(&mut context)
        .await
        .unwrap();
    let test_metadata = Metadata::new();
    airdrop(&mut context, &test_metadata.token.pubkey(), 1000000000)
        .await
        .unwrap();
    let price = 1000000000;
    let payer = context.payer.dirty_clone();
    let (rule_set, auth_data) = create_sale_delegate_rule_set(&mut context, payer).await;

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

    context.warp_to_slot(100).unwrap();
    let buyer = Keypair::new();
    // Derive Auction House Key
    airdrop(&mut context, &buyer.pubkey(), 2000000000)
        .await
        .unwrap();
    let (acc, buy_tx) = buy(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        &test_metadata.token.pubkey(),
        &buyer,
        &test_metadata.token.pubkey(),
        &listing_config_address,
        price,
    );

    context
        .banks_client
        .process_transaction(buy_tx)
        .await
        .unwrap();
    let (auctioneer_authority, aa_bump) = find_auctioneer_authority_seeds(&ahkey);
    let (auctioneer_pda, _) = find_auctioneer_pda(&ahkey, &auctioneer_authority);
    let mut accounts = mpl_auctioneer::accounts::AuctioneerCancel {
        auction_house_program: mpl_auction_house::id(),
        listing_config: listing_config_address,
        seller: test_metadata.token.pubkey(),
        auction_house: ahkey,
        wallet: buyer.pubkey(),
        token_account: acc.token_account,
        authority: ah.authority,
        trade_state: acc.buyer_trade_state,
        token_program: spl_token::id(),
        token_mint: test_metadata.mint.pubkey(),
        auction_house_fee_account: ah.auction_house_fee_account,
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

    let instruction = Instruction {
        program_id: mpl_auctioneer::id(),
        data: mpl_auctioneer::instruction::Cancel {
            auctioneer_authority_bump: aa_bump,
            buyer_price: price,
            token_size: 1,
        }
        .data(),
        accounts,
    };

    let tx = Transaction::new_signed_with_payer(
        &[instruction],
        Some(&buyer.pubkey()),
        &[&buyer],
        context.last_blockhash,
    );
    context.banks_client.process_transaction(tx).await.unwrap();

    // Make sure the trade state wasn't erroneously closed.
    let listing_config_closed = context
        .banks_client
        .get_account(listing_config_address)
        .await
        .unwrap();

    assert!(listing_config_closed.is_some());
}

#[tokio::test]
async fn cancel_pnft_highest_bid() {
    let mut context = auctioneer_program_test().start_with_context().await;
    // Payer Wallet
    let (ah, ahkey, authority) = existing_auction_house_test_context(&mut context)
        .await
        .unwrap();
    let test_metadata = Metadata::new();
    airdrop(&mut context, &test_metadata.token.pubkey(), 1000000000)
        .await
        .unwrap();
    let price = 1000000000;

    let payer = context.payer.dirty_clone();
    let (rule_set, auth_data) = create_sale_delegate_rule_set(&mut context, payer).await;

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

    context.warp_to_slot(100).unwrap();
    let buyer0 = Keypair::new();
    // Derive Auction House Key
    airdrop(&mut context, &buyer0.pubkey(), 2000000000)
        .await
        .unwrap();
    let (acc0, buy_tx0) = buy(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        &test_metadata.token.pubkey(),
        &buyer0,
        &test_metadata.token.pubkey(),
        &listing_config_address,
        price,
    );

    context
        .banks_client
        .process_transaction(buy_tx0)
        .await
        .unwrap();

    context.warp_to_slot(200).unwrap();

    let (auctioneer_authority, aa_bump) = find_auctioneer_authority_seeds(&ahkey);
    let (auctioneer_pda, _) = find_auctioneer_pda(&ahkey, &auctioneer_authority);
    let mut accounts0 = mpl_auctioneer::accounts::AuctioneerCancel {
        auction_house_program: mpl_auction_house::id(),
        listing_config: listing_config_address,
        seller: test_metadata.token.pubkey(),
        auction_house: ahkey,
        wallet: buyer0.pubkey(),
        token_account: acc0.token_account,
        authority: ah.authority,
        trade_state: acc0.buyer_trade_state,
        token_program: spl_token::id(),
        token_mint: test_metadata.mint.pubkey(),
        auction_house_fee_account: ah.auction_house_fee_account,
        auctioneer_authority,
        ah_auctioneer_pda: auctioneer_pda,
    }
    .to_account_metas(None);

    accounts0.push(AccountMeta {
        pubkey: mpl_token_metadata::id(),
        is_signer: false,
        is_writable: true,
    });
    accounts0.push(AccountMeta {
        pubkey: delegate_record,
        is_signer: false,
        is_writable: true,
    });
    accounts0.push(AccountMeta {
        pubkey: test_metadata.token_record,
        is_signer: false,
        is_writable: true,
    });
    accounts0.push(AccountMeta {
        pubkey: test_metadata.mint.pubkey(),
        is_signer: false,
        is_writable: false,
    });
    accounts0.push(AccountMeta {
        pubkey: test_metadata.master_edition,
        is_signer: false,
        is_writable: false,
    });
    accounts0.push(AccountMeta {
        pubkey: mpl_token_auth_rules::id(),
        is_signer: false,
        is_writable: false,
    });
    accounts0.push(AccountMeta {
        pubkey: rule_set,
        is_signer: false,
        is_writable: false,
    });
    accounts0.push(AccountMeta {
        pubkey: sysvar::instructions::id(),
        is_signer: false,
        is_writable: false,
    });


    let instruction0 = Instruction {
        program_id: mpl_auctioneer::id(),
        data: mpl_auctioneer::instruction::Cancel {
            auctioneer_authority_bump: aa_bump,
            buyer_price: price,
            token_size: 1,
        }
        .data(),
        accounts: accounts0,
    };

    let tx0 = Transaction::new_signed_with_payer(
        &[instruction0],
        Some(&buyer0.pubkey()),
        &[&buyer0],
        context.last_blockhash,
    );
    let result0 = context
        .banks_client
        .process_transaction(tx0)
        .await
        .unwrap_err();
    assert_error!(result0, CANNOT_CANCEL_HIGHEST_BID);

    context.warp_to_slot(300).unwrap();

    // Buyer 1 bids higher and should now be the highest bidder.
    let buyer1 = Keypair::new();
    // Derive Auction House Key
    airdrop(&mut context, &buyer1.pubkey(), 2000000000)
        .await
        .unwrap();
    let (acc1, buy_tx1) = buy(
        &mut context,
        &ahkey,
        &ah,
        &test_metadata,
        &test_metadata.token.pubkey(),
        &buyer1,
        &test_metadata.token.pubkey(),
        &listing_config_address,
        price + 1,
    );

    context
        .banks_client
        .process_transaction(buy_tx1)
        .await
        .unwrap();
    context.warp_to_slot(400).unwrap();

    let (auctioneer_authority, aa_bump) = find_auctioneer_authority_seeds(&ahkey);
    let (auctioneer_pda, _) = find_auctioneer_pda(&ahkey, &auctioneer_authority);
    let mut accounts1 = mpl_auctioneer::accounts::AuctioneerCancel {
        auction_house_program: mpl_auction_house::id(),
        listing_config: listing_config_address,
        seller: test_metadata.token.pubkey(),
        auction_house: ahkey,
        wallet: buyer1.pubkey(),
        token_account: acc1.token_account,
        authority: ah.authority,
        trade_state: acc1.buyer_trade_state,
        token_program: spl_token::id(),
        token_mint: test_metadata.mint.pubkey(),
        auction_house_fee_account: ah.auction_house_fee_account,
        auctioneer_authority,
        ah_auctioneer_pda: auctioneer_pda,
    }
    .to_account_metas(None);

    accounts1.push(AccountMeta {
        pubkey: mpl_token_metadata::id(),
        is_signer: false,
        is_writable: true,
    });
    accounts1.push(AccountMeta {
        pubkey: delegate_record,
        is_signer: false,
        is_writable: true,
    });
    accounts1.push(AccountMeta {
        pubkey: test_metadata.token_record,
        is_signer: false,
        is_writable: true,
    });
    accounts1.push(AccountMeta {
        pubkey: test_metadata.mint.pubkey(),
        is_signer: false,
        is_writable: false,
    });
    accounts1.push(AccountMeta {
        pubkey: test_metadata.master_edition,
        is_signer: false,
        is_writable: false,
    });
    accounts1.push(AccountMeta {
        pubkey: mpl_token_auth_rules::id(),
        is_signer: false,
        is_writable: false,
    });
    accounts1.push(AccountMeta {
        pubkey: rule_set,
        is_signer: false,
        is_writable: false,
    });
    accounts1.push(AccountMeta {
        pubkey: sysvar::instructions::id(),
        is_signer: false,
        is_writable: false,
    });


    let instruction1 = Instruction {
        program_id: mpl_auctioneer::id(),
        data: mpl_auctioneer::instruction::Cancel {
            auctioneer_authority_bump: aa_bump,
            buyer_price: price + 1,
            token_size: 1,
        }
        .data(),
        accounts: accounts1,
    };

    let tx1 = Transaction::new_signed_with_payer(
        &[instruction1],
        Some(&buyer1.pubkey()),
        &[&buyer1],
        context.last_blockhash,
    );

    let result1 = context
        .banks_client
        .process_transaction(tx1)
        .await
        .unwrap_err();
    assert_error!(result1, CANNOT_CANCEL_HIGHEST_BID);
    context.warp_to_slot(500).unwrap();

    // Rerun the cancel on the lower bid to verify it now succeeds.
    let (auctioneer_authority, aa_bump) = find_auctioneer_authority_seeds(&ahkey);
    let (auctioneer_pda, _) = find_auctioneer_pda(&ahkey, &auctioneer_authority);
    let mut accounts2 = mpl_auctioneer::accounts::AuctioneerCancel {
        auction_house_program: mpl_auction_house::id(),
        listing_config: listing_config_address,
        seller: test_metadata.token.pubkey(),
        auction_house: ahkey,
        wallet: buyer0.pubkey(),
        token_account: acc0.token_account,
        authority: ah.authority,
        trade_state: acc0.buyer_trade_state,
        token_program: spl_token::id(),
        token_mint: test_metadata.mint.pubkey(),
        auction_house_fee_account: ah.auction_house_fee_account,
        auctioneer_authority,
        ah_auctioneer_pda: auctioneer_pda,
    }
    .to_account_metas(None);

    accounts2.push(AccountMeta {
        pubkey: mpl_token_metadata::id(),
        is_signer: false,
        is_writable: true,
    });
    accounts2.push(AccountMeta {
        pubkey: delegate_record,
        is_signer: false,
        is_writable: true,
    });
    accounts2.push(AccountMeta {
        pubkey: test_metadata.token_record,
        is_signer: false,
        is_writable: true,
    });
    accounts2.push(AccountMeta {
        pubkey: test_metadata.mint.pubkey(),
        is_signer: false,
        is_writable: false,
    });
    accounts2.push(AccountMeta {
        pubkey: test_metadata.master_edition,
        is_signer: false,
        is_writable: false,
    });
    accounts2.push(AccountMeta {
        pubkey: mpl_token_auth_rules::id(),
        is_signer: false,
        is_writable: false,
    });
    accounts2.push(AccountMeta {
        pubkey: rule_set,
        is_signer: false,
        is_writable: false,
    });
    accounts2.push(AccountMeta {
        pubkey: sysvar::instructions::id(),
        is_signer: false,
        is_writable: false,
    });


    let instruction2 = Instruction {
        program_id: mpl_auctioneer::id(),
        data: mpl_auctioneer::instruction::Cancel {
            auctioneer_authority_bump: aa_bump,
            buyer_price: price,
            token_size: 1,
        }
        .data(),
        accounts: accounts2,
    };

    let tx2 = Transaction::new_signed_with_payer(
        &[instruction2],
        Some(&buyer0.pubkey()),
        &[&buyer0],
        context.last_blockhash,
    );
    context.banks_client.process_transaction(tx2).await.unwrap();
}

