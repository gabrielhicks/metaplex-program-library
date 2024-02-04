use mpl_auction_house::{constants::MAX_NUM_SCOPES, AuthorityScope};

use solana_sdk::signer::keypair::Keypair;

pub fn default_scopes() -> Vec<AuthorityScope> {
    vec![
        AuthorityScope::Deposit,
        AuthorityScope::Buy,
        AuthorityScope::PublicBuy,
        AuthorityScope::ExecuteSale,
        AuthorityScope::Sell,
        AuthorityScope::Cancel,
        AuthorityScope::Withdraw,
    ]
}

pub fn assert_scopes_eq(scopes: Vec<AuthorityScope>, scopes_array: [bool; MAX_NUM_SCOPES]) {
    for scope in scopes {
        if !scopes_array[scope as usize] {
            panic!();
        }
    }
}

pub trait DirtyClone {
    fn dirty_clone(&self) -> Self;
}

impl DirtyClone for Keypair {
    fn dirty_clone(&self) -> Self {
        Keypair::from_bytes(&self.to_bytes()).unwrap()
    }
}