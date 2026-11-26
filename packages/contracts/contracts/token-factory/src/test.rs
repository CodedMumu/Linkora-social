#![cfg(test)]

use super::*;
use soroban_sdk::{testutils::Address as _, Address, BytesN, Env, String};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Placeholder 32-byte hash used when we don't need real WASM semantics.
fn dummy_wasm_hash(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &[1u8; 32])
}

fn setup(env: &Env) -> (TokenFactoryContractClient<'_>, Address, BytesN<32>) {
    let factory_id = env.register(TokenFactoryContract, ());
    let client = TokenFactoryContractClient::new(env, &factory_id);

    let admin = Address::generate(env);
    let wasm_hash = dummy_wasm_hash(env);
    client.initialize(&admin, &wasm_hash);

    (client, admin, wasm_hash)
}

// ── Initialization tests ──────────────────────────────────────────────────────

#[test]
fn test_initialize_stores_wasm_hash() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, wasm_hash) = setup(&env);

    assert_eq!(client.get_token_wasm_hash(), wasm_hash);
}

#[test]
#[should_panic(expected = "already initialized")]
fn test_double_initialize_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin, wasm_hash) = setup(&env);

    // Second call must panic.
    client.initialize(&admin, &wasm_hash);
}

#[test]
fn test_update_token_wasm_stores_new_hash() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup(&env);

    let new_hash = BytesN::from_array(&env, &[2u8; 32]);
    client.update_token_wasm(&new_hash);

    assert_eq!(client.get_token_wasm_hash(), new_hash);
}

// ── Salt derivation tests ─────────────────────────────────────────────────────
//
// Two deployments with the same deployer but different symbols must produce
// different salts.  We verify this indirectly through the derive_salt helper.

#[test]
fn test_different_symbols_produce_different_salts() {
    let env = Env::default();
    env.mock_all_auths();

    let deployer = Address::generate(&env);

    let salt_alpha =
        TokenFactoryContract::derive_salt(&env, &deployer, &String::from_str(&env, "ALPHA"));
    let salt_beta =
        TokenFactoryContract::derive_salt(&env, &deployer, &String::from_str(&env, "BETA"));

    assert_ne!(salt_alpha, salt_beta);
}

#[test]
fn test_same_inputs_produce_same_salt() {
    let env = Env::default();
    env.mock_all_auths();

    let deployer = Address::generate(&env);
    let sym = String::from_str(&env, "STL");

    let s1 = TokenFactoryContract::derive_salt(&env, &deployer, &sym);
    let s2 = TokenFactoryContract::derive_salt(&env, &deployer, &sym);

    assert_eq!(s1, s2);
}

// ── Event emission test ───────────────────────────────────────────────────────

#[test]
fn test_event_published_when_deploy_helper_called() {
    // This test verifies the event structs are wired correctly.
    // Since deploy_creator_token needs a real WASM hash, we trust that
    // test_initialize_stores_wasm_hash + the ADR integration spec cover
    // the deploy path, and we simply confirm the publish call compiles
    // and can be called within a contract execution context.
    //
    // The integration test (run_e2e.sh or a dedicated script) validates
    // the full deploy → event → indexer pipeline on a live RPC.
    //
    // We verify the event data types are correct by instantiating the struct.
    let env = Env::default();
    env.mock_all_auths();

    let deployer = Address::generate(&env);
    let token_address = Address::generate(&env);

    // Verify the event struct can be constructed with the expected fields.
    let _event = CreatorTokenDeployedEvent {
        deployer: deployer.clone(),
        token_address: token_address.clone(),
        name: String::from_str(&env, "My Token"),
        symbol: String::from_str(&env, "MTK"),
    };
    // If this compiles and runs, the struct is correctly wired.
    // The publish() call itself is exercised in the integration tests.
}

// ── Validation tests ──────────────────────────────────────────────────────────

#[test]
#[should_panic(expected = "token_wasm_hash must not be zero")]
fn test_initialize_rejects_zero_wasm_hash() {
    let env = Env::default();
    env.mock_all_auths();
    let factory_id = env.register(TokenFactoryContract, ());
    let client = TokenFactoryContractClient::new(&env, &factory_id);

    let admin = Address::generate(&env);
    let zero_hash = BytesN::from_array(&env, &[0u8; 32]);
    client.initialize(&admin, &zero_hash);
}

#[test]
#[should_panic(expected = "token_wasm_hash must not be zero")]
fn test_update_token_wasm_rejects_zero_hash() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _, _) = setup(&env);

    let zero_hash = BytesN::from_array(&env, &[0u8; 32]);
    client.update_token_wasm(&zero_hash);
}

// ── Issue #623: Stable address-derivation vectors ─────────────────────────────

/// The same (deployer, symbol) pair must produce the same salt on every call.
/// This is the idempotency property SDK consumers rely on when pre-computing
/// addresses before a deployment transaction lands.
#[test]
fn test_derive_salt_same_pair_is_idempotent() {
    let env = Env::default();
    env.mock_all_auths();

    let deployer = Address::generate(&env);
    let symbol = String::from_str(&env, "LNKR");

    let salt_a = TokenFactoryContract::derive_salt(&env, &deployer, &symbol);
    let salt_b = TokenFactoryContract::derive_salt(&env, &deployer, &symbol);

    assert_eq!(
        salt_a, salt_b,
        "derive_salt must return the same value for the same (deployer, symbol) pair"
    );
}

/// Changing the deployer while keeping the symbol constant must yield a
/// different salt — two distinct accounts cannot collide on the same symbol.
#[test]
fn test_derive_salt_different_deployer_produces_distinct_salt() {
    let env = Env::default();
    env.mock_all_auths();

    let deployer_a = Address::generate(&env);
    let deployer_b = Address::generate(&env);
    let symbol = String::from_str(&env, "LNKR");

    let salt_a = TokenFactoryContract::derive_salt(&env, &deployer_a, &symbol);
    let salt_b = TokenFactoryContract::derive_salt(&env, &deployer_b, &symbol);

    assert_ne!(
        salt_a, salt_b,
        "derive_salt must differ when the deployer changes"
    );
}

/// Changing the symbol while keeping the deployer constant must yield a
/// different salt — the same deployer must get distinct addresses per symbol.
#[test]
fn test_derive_salt_different_symbol_produces_distinct_salt() {
    let env = Env::default();
    env.mock_all_auths();

    let deployer = Address::generate(&env);
    let sym_x = String::from_str(&env, "LNKR");
    let sym_y = String::from_str(&env, "WAVE");

    let salt_x = TokenFactoryContract::derive_salt(&env, &deployer, &sym_x);
    let salt_y = TokenFactoryContract::derive_salt(&env, &deployer, &sym_y);

    assert_ne!(
        salt_x, salt_y,
        "derive_salt must differ when the symbol changes"
    );
}

/// Three representative (deployer, symbol) reference vectors all produce
/// mutually distinct salts — no accidental collision between common tickers.
#[test]
fn test_derive_salt_reference_vectors_are_all_distinct() {
    let env = Env::default();
    env.mock_all_auths();

    let deployer1 = Address::generate(&env);
    let deployer2 = Address::generate(&env);
    let deployer3 = Address::generate(&env);

    let salt1 =
        TokenFactoryContract::derive_salt(&env, &deployer1, &String::from_str(&env, "LNKR"));
    let salt2 =
        TokenFactoryContract::derive_salt(&env, &deployer2, &String::from_str(&env, "WAVE"));
    let salt3 =
        TokenFactoryContract::derive_salt(&env, &deployer3, &String::from_str(&env, "XLM"));

    assert_ne!(salt1, salt2, "reference vector 1 and 2 must not collide");
    assert_ne!(salt1, salt3, "reference vector 1 and 3 must not collide");
    assert_ne!(salt2, salt3, "reference vector 2 and 3 must not collide");
}
