use super::super::test::*;
use crate::{GovParameter, GovStatus};
use soroban_sdk::{testutils::Address as _, Address, BytesN, Env, String};

#[test]
#[should_panic(expected = "authority_pubkey must not be an all-zero or malformed public key")]
fn test_set_credential_authority_zero_pubkey_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin, _) = setup_contract(&env);
    let zero_pubkey = BytesN::from_array(&env, &[0u8; 32]);

    client.set_credential_authority(&admin, &zero_pubkey);
}

#[test]
#[should_panic(expected = "new_root must not be an all-zero or malformed public key")]
fn test_update_credential_root_zero_root_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin, _) = setup_contract(&env);

    let signing_key = credential_authority_signing_key(1);
    let pubkey = credential_authority_pubkey(&env, &signing_key);
    client.set_credential_authority(&admin, &pubkey);

    let user = Address::generate(&env);
    let zero_root = BytesN::from_array(&env, &[0u8; 32]);
    let dummy_sig = BytesN::from_array(&env, &[1u8; 64]);

    client.update_credential_root(&user, &zero_root, &dummy_sig);
}

#[test]
#[should_panic(expected = "signature must not be an all-zero or malformed signature")]
fn test_update_credential_root_zero_signature_rejected_governance() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, admin, _) = setup_contract(&env);

    let signing_key = credential_authority_signing_key(1);
    let pubkey = credential_authority_pubkey(&env, &signing_key);
    client.set_credential_authority(&admin, &pubkey);

    let user = Address::generate(&env);
    let new_root = BytesN::from_array(&env, &[9u8; 32]);
    let zero_signature = BytesN::from_array(&env, &[0u8; 64]);

    client.update_credential_root(&user, &new_root, &zero_signature);
}

// ── Issue #115: Governance vote replay prevention and overwrite behavior ──────────

/// Test Issue #115: Governance vote replay prevention and overwrite behavior.
///
/// Specification & Behavior Definition:
/// - Replay / Overwrite Policy: Rejection by Panic ("already voted").
///   When a voter submits a second vote on the same proposal from the same address,
///   the contract strictly REJECTS the vote (panics with "already voted") rather than
///   replacing or updating the previous vote.
/// - Vote Count Consistency:
///   Because the second vote is rejected, the proposal's vote tally (`votes_for`, `votes_against`)
///   remains strictly consistent with the initial valid vote. The second attempt does not alter,
///   increment, or decrement existing counts.
#[test]
fn test_vote_overwrite_behavior() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _) = setup_governance(&env);

    let proposer = Address::generate(&env);
    let token = Address::generate(&env);
    client.set_profile(&proposer, &String::from_str(&env, "proposer_gov_t"), &token);
    let proposal_id = client.gov_propose(&proposer, &GovParameter::FeeBps, &500, &None);

    let voter1 = Address::generate(&env);

    // Initial state: proposal starts with 0 votes for and 0 votes against
    let initial_proposal = client.gov_get_proposal(&proposal_id);
    assert_eq!(initial_proposal.votes_for, 0);
    assert_eq!(initial_proposal.votes_against, 0);

    // 1. Cast first vote: voter1 votes FOR the proposal
    client.gov_vote(&voter1, &proposal_id, &true);

    let proposal_after_first = client.gov_get_proposal(&proposal_id);
    assert_eq!(proposal_after_first.votes_for, 1);
    assert_eq!(proposal_after_first.votes_against, 0);

    // 2. Second vote attempt: voter1 attempts to vote again with support = false.
    // The contract must reject the second vote with a panic ("already voted").
    // With `try_gov_vote`, the error is safely caught and asserted.
    let second_vote_res = client.try_gov_vote(&voter1, &proposal_id, &false);
    assert!(
        second_vote_res.is_err(),
        "Second vote from same address must be rejected with error"
    );

    // 3. Assert vote counts remain strictly consistent and unchanged
    let proposal_after_second = client.gov_get_proposal(&proposal_id);
    assert_eq!(
        proposal_after_second.votes_for, 1,
        "votes_for count must remain 1 after rejected vote replay"
    );
    assert_eq!(
        proposal_after_second.votes_against, 0,
        "votes_against count must remain 0 after rejected vote replay"
    );

    // 4. Verify that another voter can legitimately vote AGAINST
    let voter2 = Address::generate(&env);
    client.gov_vote(&voter2, &proposal_id, &false);

    let proposal_after_voter2 = client.gov_get_proposal(&proposal_id);
    assert_eq!(proposal_after_voter2.votes_for, 1);
    assert_eq!(proposal_after_voter2.votes_against, 1);

    // Voter 2 also cannot replay or overwrite their vote to true
    let voter2_second_vote_res = client.try_gov_vote(&voter2, &proposal_id, &true);
    assert!(
        voter2_second_vote_res.is_err(),
        "Second vote from voter 2 must be rejected"
    );

    let final_proposal = client.gov_get_proposal(&proposal_id);
    assert_eq!(final_proposal.votes_for, 1);
    assert_eq!(final_proposal.votes_against, 1);
}

/// Test Issue #115: Voting on a non-existent proposal panics with "proposal not found".
#[test]
#[should_panic(expected = "proposal not found")]
fn test_vote_non_existent_proposal_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, _) = setup_governance(&env);

    let voter = Address::generate(&env);
    let non_existent_proposal_id: u64 = 999_999;

    client.gov_vote(&voter, &non_existent_proposal_id, &true);
}

// ── Issue #621: Proposal quorum snapshots across configuration updates ──────────

/// Proposals snapshot the quorum at creation time. A configuration change after
/// proposal creation must not alter the quorum of the existing proposal, while a
/// new proposal created after the change must reflect the updated quorum.
///
/// Setup:
///   - Config A: quorum=60, vote_window=200, time_lock=100, decay=0, floor=0
///   - Proposal P1 is created under Config A  →  P1.quorum == 60
///   - Config is updated to quorum=80 (Config B)
///   - Proposal P2 is created under Config B  →  P2.quorum == 80
///   - Both proposals are voted on with the same fixed fixtures
///   - P1 is finalized: 65 % approval ≥ 60 → Passed
///   - P2 is finalized: 65 % approval < 80 → Failed
#[test]
fn test_quorum_snapshot_isolated_across_config_change() {
    let env = Env::default();
    env.mock_all_auths();

    // Config A: quorum=60, no decay, floor=0
    let (client, admin, _) = setup_contract(&env);
    client.gov_init_config(&admin, &60, &100, &200, &0, &0);

    // Create P1 under Config A — snapshots quorum=60
    let proposer1 = Address::generate(&env);
    let token1 = Address::generate(&env);
    client.set_profile(&proposer1, &String::from_str(&env, "prop_621a"), &token1);
    let p1 = client.gov_propose(&proposer1, &GovParameter::FeeBps, &100, &None);
    let p1_snapshot = client.gov_get_proposal(&p1);
    assert_eq!(
        p1_snapshot.quorum, 60,
        "P1 must snapshot quorum=60 from Config A"
    );

    // Update config to quorum=80 (Config B) — P1 must be unaffected
    client.gov_init_config(&admin, &80, &100, &200, &0, &0);

    // Create P2 under Config B — snapshots quorum=80
    let proposer2 = Address::generate(&env);
    let token2 = Address::generate(&env);
    client.set_profile(&proposer2, &String::from_str(&env, "prop_621b"), &token2);
    let p2 = client.gov_propose(&proposer2, &GovParameter::FeeBps, &200, &None);
    let p2_snapshot = client.gov_get_proposal(&p2);
    assert_eq!(
        p2_snapshot.quorum, 80,
        "P2 must snapshot quorum=80 from Config B"
    );

    // Fixed vote fixture: 13 for, 7 against = 65 % approval
    // 65 % ≥ 60 (P1 quorum) → P1 passes
    // 65 % < 80 (P2 quorum) → P2 fails
    for _ in 0..13 {
        let v = Address::generate(&env);
        client.gov_vote(&v, &p1, &true);
    }
    for _ in 0..7 {
        let v = Address::generate(&env);
        client.gov_vote(&v, &p1, &false);
    }

    for _ in 0..13 {
        let v = Address::generate(&env);
        client.gov_vote(&v, &p2, &true);
    }
    for _ in 0..7 {
        let v = Address::generate(&env);
        client.gov_vote(&v, &p2, &false);
    }

    // Advance past vote_window (200)
    env.ledger()
        .with_mut(|l| l.sequence_number += 201);

    let s1 = client.gov_finalize(&p1);
    let s2 = client.gov_finalize(&p2);

    assert_eq!(
        s1,
        GovStatus::Passed,
        "P1 must pass: 65 % approval meets its snapshotted quorum of 60"
    );
    assert_eq!(
        s2,
        GovStatus::Failed,
        "P2 must fail: 65 % approval does not meet its snapshotted quorum of 80"
    );

    // Verify persisted statuses match what gov_finalize returned
    assert_eq!(client.gov_get_proposal(&p1).status, GovStatus::Passed);
    assert_eq!(client.gov_get_proposal(&p2).status, GovStatus::Failed);
}
