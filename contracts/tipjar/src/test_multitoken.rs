//! Multi-token isolation and cross-token accounting tests for the TipJar contract.
//!
//! Addresses issue #408: verifies that accounting across multiple tokens is
//! strictly isolated — tipping heavily in one token does not affect another token's
//! recorded balances/totals, removing one token leaves other tokens' allowlist
//! membership and balances untouched, and fee deductions operate independently.

#![cfg(test)]

use crate::{DataKey, Error, TipJar, TipJarClient, DATA_VERSION};
use soroban_sdk::{
    testutils::Address as _,
    token, Address, Env, Vec,
};

const TEST_TIMELOCK: u32 = 1_000;

struct MultiTokenCtx {
    env: Env,
    contract_id: Address,
    token_a: Address,
    token_b: Address,
    admin: Address,
}

impl MultiTokenCtx {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();

        let token_admin = Address::generate(&env);
        let token_a = env
            .register_stellar_asset_contract_v2(token_admin.clone())
            .address();
        let token_b = env
            .register_stellar_asset_contract_v2(token_admin)
            .address();

        let admin = Address::generate(&env);
        let contract_id = env.register(TipJar, ());
        let client = TipJarClient::new(&env, &contract_id);

        // Initialize with token_a
        client.init(&token_a, &admin, &TEST_TIMELOCK);

        MultiTokenCtx {
            env,
            contract_id,
            token_a,
            token_b,
            admin,
        }
    }

    fn client(&self) -> TipJarClient<'_> {
        TipJarClient::new(&self.env, &self.contract_id)
    }

    fn token_client_a(&self) -> token::Client<'_> {
        token::Client::new(&self.env, &self.token_a)
    }

    fn token_client_b(&self) -> token::Client<'_> {
        token::Client::new(&self.env, &self.token_b)
    }

    fn fund_a(&self, user: &Address, amount: i128) {
        token::StellarAssetClient::new(&self.env, &self.token_a).mint(user, &amount);
    }

    fn fund_b(&self, user: &Address, amount: i128) {
        token::StellarAssetClient::new(&self.env, &self.token_b).mint(user, &amount);
    }

    fn new_token(&self) -> Address {
        let token_admin = Address::generate(&self.env);
        self.env
            .register_stellar_asset_contract_v2(token_admin)
            .address()
    }
}

#[test]
fn test_multi_token_initialization() {
    let ctx = MultiTokenCtx::new();
    let client = ctx.client();

    // Should have token_a in allowlist initially
    let tokens = client.get_tokens();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens.first().unwrap(), ctx.token_a);
}

#[test]
fn test_add_and_remove_tokens() {
    let ctx = MultiTokenCtx::new();
    let client = ctx.client();

    // Add token_b
    client.add_token(&ctx.admin, &ctx.token_b);

    let tokens = client.get_tokens();
    assert_eq!(tokens.len(), 2);
    assert!(tokens.iter().any(|t| t == ctx.token_a));
    assert!(tokens.iter().any(|t| t == ctx.token_b));

    // Remove token_b
    client.remove_token(&ctx.admin, &ctx.token_b);

    let tokens = client.get_tokens();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens.first().unwrap(), ctx.token_a);
}

#[test]
fn test_multi_token_tip_and_balance_isolation() {
    let ctx = MultiTokenCtx::new();
    let client = ctx.client();
    let creator = Address::generate(&ctx.env);
    let tipper = Address::generate(&ctx.env);

    // Add token_b to allowlist
    client.add_token(&ctx.admin, &ctx.token_b);

    // Fund tipper with both tokens
    ctx.fund_a(&tipper, 1000);
    ctx.fund_b(&tipper, 2000);

    // Tip different amounts in different tokens
    client.tip(&tipper, &creator, &ctx.token_a, &100);
    client.tip(&tipper, &creator, &ctx.token_b, &300);

    // Check balances are isolated per token
    assert_eq!(client.get_balance(&creator, &ctx.token_a), 100);
    assert_eq!(client.get_balance(&creator, &ctx.token_b), 300);
    assert_eq!(client.get_total_tips(&creator, &ctx.token_a), 100);
    assert_eq!(client.get_total_tips(&creator, &ctx.token_b), 300);

    // Tip more to same creator in same tokens
    client.tip(&tipper, &creator, &ctx.token_a, &50);
    client.tip(&tipper, &creator, &ctx.token_b, &200);

    // Check accumulation
    assert_eq!(client.get_balance(&creator, &ctx.token_a), 150);
    assert_eq!(client.get_balance(&creator, &ctx.token_b), 500);
    assert_eq!(client.get_total_tips(&creator, &ctx.token_a), 150);
    assert_eq!(client.get_total_tips(&creator, &ctx.token_b), 500);
}

#[test]
fn test_multi_token_withdrawal() {
    let ctx = MultiTokenCtx::new();
    let client = ctx.client();
    let creator = Address::generate(&ctx.env);
    let tipper = Address::generate(&ctx.env);

    // Add token_b to allowlist
    client.add_token(&ctx.admin, &ctx.token_b);

    // Fund and tip
    ctx.fund_a(&tipper, 1000);
    ctx.fund_b(&tipper, 1000);
    client.tip(&tipper, &creator, &ctx.token_a, &100);
    client.tip(&tipper, &creator, &ctx.token_b, &200);

    // Withdraw token_a
    let creator_balance_a_before = ctx.token_client_a().balance(&creator);
    client.withdraw(&creator, &creator, &ctx.token_a, &creator, &Some(50));
    let creator_balance_a_after = ctx.token_client_a().balance(&creator);

    assert_eq!(creator_balance_a_after - creator_balance_a_before, 50);
    assert_eq!(client.get_balance(&creator, &ctx.token_a), 50);
    assert_eq!(client.get_balance(&creator, &ctx.token_b), 200); // Unchanged

    // Withdraw token_b (full amount)
    let creator_balance_b_before = ctx.token_client_b().balance(&creator);
    client.withdraw(&creator, &creator, &ctx.token_b, &creator, &None);
    let creator_balance_b_after = ctx.token_client_b().balance(&creator);

    assert_eq!(creator_balance_b_after - creator_balance_b_before, 200);
    assert_eq!(client.get_balance(&creator, &ctx.token_a), 50); // Unchanged
    assert_eq!(client.get_balance(&creator, &ctx.token_b), 0);
}

#[test]
fn test_token_not_allowed_error() {
    let ctx = MultiTokenCtx::new();
    let client = ctx.client();
    let creator = Address::generate(&ctx.env);
    let tipper = Address::generate(&ctx.env);

    // Try to tip with token_b which is not in allowlist
    ctx.fund_b(&tipper, 1000);

    let err = client.try_tip(&tipper, &creator, &ctx.token_b, &100).unwrap_err().unwrap();
    assert_eq!(err, Error::TokenNotAllowed.into());
}

#[test]
fn test_token_already_exists_error() {
    let ctx = MultiTokenCtx::new();
    let client = ctx.client();

    // Try to add token_a again (already in allowlist)
    let err = client.try_add_token(&ctx.admin, &ctx.token_a).unwrap_err().unwrap();
    assert_eq!(err, Error::TokenAlreadyExists.into());
}

#[test]
fn test_remove_nonexistent_token_error() {
    let ctx = MultiTokenCtx::new();
    let client = ctx.client();

    // Try to remove token_b which is not in allowlist
    let err = client.try_remove_token(&ctx.admin, &ctx.token_b).unwrap_err().unwrap();
    assert_eq!(err, Error::TokenNotAllowed.into());
}

// ─── ISSUE #408 EXPLICIT CROSS-TOKEN ISOLATION TESTS ─────────────────────────

/// Issue #408: Tipping heavily in token A must never perturb token B's recorded
/// balances or lifetime totals for the same creator, and withdrawing from token A
/// must leave token B's balances completely untouched.
#[test]
fn test_cross_token_isolation_heavy_tipping_and_withdrawal() {
    let ctx = MultiTokenCtx::new();
    let client = ctx.client();
    let creator = Address::generate(&ctx.env);
    let tipper = Address::generate(&ctx.env);

    // Add token_b alongside token_a
    client.add_token(&ctx.admin, &ctx.token_b);

    // Seed baseline balances in both tokens
    ctx.fund_a(&tipper, 200_000);
    ctx.fund_b(&tipper, 5_000);

    client.tip(&tipper, &creator, &ctx.token_a, &500);
    client.tip(&tipper, &creator, &ctx.token_b, &1_000);

    assert_eq!(client.get_balance(&creator, &ctx.token_a), 500);
    assert_eq!(client.get_total_tips(&creator, &ctx.token_a), 500);
    assert_eq!(client.get_balance(&creator, &ctx.token_b), 1_000);
    assert_eq!(client.get_total_tips(&creator, &ctx.token_b), 1_000);

    // Execute heavy tipping in token A (+100_000)
    client.tip(&tipper, &creator, &ctx.token_a, &100_000);

    // Token A must update correctly
    assert_eq!(client.get_balance(&creator, &ctx.token_a), 100_500);
    assert_eq!(client.get_total_tips(&creator, &ctx.token_a), 100_500);

    // Token B MUST remain strictly isolated and unaffected
    assert_eq!(client.get_balance(&creator, &ctx.token_b), 1_000);
    assert_eq!(client.get_total_tips(&creator, &ctx.token_b), 1_000);

    // Withdraw partially from Token A
    client.withdraw(&creator, &creator, &ctx.token_a, &creator, &Some(50_000));
    assert_eq!(client.get_balance(&creator, &ctx.token_a), 50_500);
    assert_eq!(client.get_total_tips(&creator, &ctx.token_a), 100_500);

    // Token B balances and totals remain intact
    assert_eq!(client.get_balance(&creator, &ctx.token_b), 1_000);
    assert_eq!(client.get_total_tips(&creator, &ctx.token_b), 1_000);
}

/// Issue #408: Removing token B from the allowlist must leave token A's
/// allowlist membership, balances, and tip flows completely intact. Furthermore,
/// existing token B balances for the creator must remain safely withdrawable.
#[test]
fn test_cross_token_isolation_allowlist_removal() {
    let ctx = MultiTokenCtx::new();
    let client = ctx.client();
    let creator = Address::generate(&ctx.env);
    let tipper = Address::generate(&ctx.env);

    client.add_token(&ctx.admin, &ctx.token_b);
    ctx.fund_a(&tipper, 10_000);
    ctx.fund_b(&tipper, 10_000);

    // Creator receives tips in both tokens
    client.tip(&tipper, &creator, &ctx.token_a, &2_000);
    client.tip(&tipper, &creator, &ctx.token_b, &3_000);

    assert_eq!(client.get_balance(&creator, &ctx.token_a), 2_000);
    assert_eq!(client.get_balance(&creator, &ctx.token_b), 3_000);

    // Remove token_b from allowlist
    client.remove_token(&ctx.admin, &ctx.token_b);

    // 1. Token A must remain in allowlist
    let tokens = client.get_tokens();
    assert_eq!(tokens.len(), 1);
    assert!(tokens.iter().any(|t| t == ctx.token_a));
    assert!(!tokens.iter().any(|t| t == ctx.token_b));

    // 2. Token A balances and totals are completely unaffected
    assert_eq!(client.get_balance(&creator, &ctx.token_a), 2_000);
    assert_eq!(client.get_total_tips(&creator, &ctx.token_a), 2_000);

    // 3. Tipping in token A continues to succeed
    client.tip(&tipper, &creator, &ctx.token_a, &500);
    assert_eq!(client.get_balance(&creator, &ctx.token_a), 2_500);

    // 4. Tipping in removed token B is blocked
    let err = client.try_tip(&tipper, &creator, &ctx.token_b, &100).unwrap_err().unwrap();
    assert_eq!(err, Error::TokenNotAllowed.into());

    // 5. Existing token B balance remains safely withdrawable
    let creator_b_before = ctx.token_client_b().balance(&creator);
    client.withdraw(&creator, &creator, &ctx.token_b, &creator, &None);
    let creator_b_after = ctx.token_client_b().balance(&creator);
    assert_eq!(creator_b_after - creator_b_before, 3_000);
    assert_eq!(client.get_balance(&creator, &ctx.token_b), 0);

    // Token A is still untouched after Token B withdrawal
    assert_eq!(client.get_balance(&creator, &ctx.token_a), 2_500);
}

/// Issue #408: Independent creator and multi-token matrix isolation.
#[test]
fn test_cross_token_multi_creator_isolation() {
    let ctx = MultiTokenCtx::new();
    let client = ctx.client();
    let creator1 = Address::generate(&ctx.env);
    let creator2 = Address::generate(&ctx.env);
    let tipper = Address::generate(&ctx.env);

    client.add_token(&ctx.admin, &ctx.token_b);
    ctx.fund_a(&tipper, 10_000);
    ctx.fund_b(&tipper, 10_000);

    // Creator 1 gets token A, Creator 2 gets token B
    client.tip(&tipper, &creator1, &ctx.token_a, &1_000);
    client.tip(&tipper, &creator2, &ctx.token_b, &2_000);

    assert_eq!(client.get_balance(&creator1, &ctx.token_a), 1_000);
    assert_eq!(client.get_balance(&creator1, &ctx.token_b), 0);
    assert_eq!(client.get_balance(&creator2, &ctx.token_a), 0);
    assert_eq!(client.get_balance(&creator2, &ctx.token_b), 2_000);

    // Cross-tip
    client.tip(&tipper, &creator1, &ctx.token_b, &500);
    client.tip(&tipper, &creator2, &ctx.token_a, &700);

    assert_eq!(client.get_balance(&creator1, &ctx.token_a), 1_000);
    assert_eq!(client.get_balance(&creator1, &ctx.token_b), 500);
    assert_eq!(client.get_balance(&creator2, &ctx.token_a), 700);
    assert_eq!(client.get_balance(&creator2, &ctx.token_b), 2_000);
}
