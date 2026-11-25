//! Engine tests for the OotleGig `JobEscrow` template.
//!
//! These run against the real Ootle engine in-process via `tari_template_test_tooling`
//! (no network, no funds). Every rule in the template has at least one test that would
//! fail if the rule were removed: signer checks, state machine order, the review-window
//! epoch math, exact payout amounts, and the one-time receipt.

use ootle_byte_type::ToByteType;
use ootle_gig::{JobInfo, JobState};
use tari_crypto::ristretto::{RistrettoPublicKey, RistrettoSecretKey};
use tari_ootle_common_types::substate_type::SubstateType;
use tari_ootle_transaction::{args, builder::named_args::NamedArg};
use tari_template_lib_types::{
    Amount, ComponentAddress, NonFungibleAddress, ResourceAddress, constants::TARI_TOKEN,
};
use tari_template_test_tooling::{
    TemplateTest,
    engine_types::virtual_substate::{VirtualSubstate, VirtualSubstateId},
};

/// sha256("test")
const TASK_HASH: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
/// sha256("hello")
const DELIVERY_HASH: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
/// Funded accounts start with 1_000_000_000 (TemplateTest::FUNDED_ACCOUNT_INITIAL_BALANCE).
const START_BALANCE: u64 = 1_000_000_000;
const JOB_AMOUNT: u64 = 400_000_000;

struct Party {
    account: ComponentAddress,
    proof: NonFungibleAddress,
    secret: RistrettoSecretKey,
    public_key: RistrettoPublicKey,
}

fn make_party(test: &mut TemplateTest) -> Party {
    let (account, proof, secret, public_key) = test.create_funded_account_with_keypair();
    Party {
        account,
        proof,
        secret,
        public_key,
    }
}

fn tari_balance(test: &TemplateTest, account: ComponentAddress) -> Amount {
    let store = test.read_only_state_store();
    let vaults = store
        .get_vaults_for_account(account)
        .expect("account vaults readable");
    vaults
        .get(&TARI_TOKEN)
        .map(|vault| vault.balance())
        .unwrap_or_default()
}

fn nft_count(test: &TemplateTest, account: ComponentAddress, resource: ResourceAddress) -> usize {
    let store = test.read_only_state_store();
    let vaults = store
        .get_vaults_for_account(account)
        .expect("account vaults readable");
    vaults
        .get(&resource)
        .map(|vault| vault.get_non_fungible_ids().len())
        .unwrap_or(0)
}

fn set_epoch(test: &mut TemplateTest, epoch: u64) {
    test.set_virtual_substate(
        VirtualSubstateId::CurrentEpoch,
        VirtualSubstate::CurrentEpoch(epoch),
    );
}

/// Post a job as `client`, escrowing JOB_AMOUNT against TASK_HASH.
fn create_job(test: &mut TemplateTest, client: &Party, review_epochs: u64) -> ComponentAddress {
    let template = test.get_template_address("JobEscrow");
    test.execute_expect_success(
        test.transaction()
            .call_method(client.account, "withdraw", args![TARI_TOKEN, JOB_AMOUNT])
            .put_last_instruction_output_on_workspace("payment")
            .call_function(
                template,
                "new",
                args![Workspace("payment"), TASK_HASH, review_epochs],
            )
            .build_and_seal(&client.secret),
        vec![client.proof.clone()],
    );
    test.get_previous_output_address(SubstateType::Component)
        .as_component_address()
        .expect("job component was created")
}

/// Call a no-bucket method on the job, signed by `party`, expecting success.
fn call_ok(test: &mut TemplateTest, party: &Party, job: ComponentAddress, method: &str, args: Vec<NamedArg>) {
    test.execute_expect_success(
        test.transaction()
            .call_method(job, method, args)
            .build_and_seal(&party.secret),
        vec![party.proof.clone()],
    );
}

/// Call a method on the job, signed by `party`, expecting the transaction to be rejected
/// with a reason containing `expected` (the template's own assertion message).
fn call_rejected(
    test: &mut TemplateTest,
    party: &Party,
    job: ComponentAddress,
    method: &str,
    args: Vec<NamedArg>,
    expected: &str,
) {
    let reason = test.execute_expect_failure(
        test.transaction()
            .call_method(job, method, args)
            .build_and_seal(&party.secret),
        vec![party.proof.clone()],
    );
    let reason = reason.to_string();
    assert!(
        reason.contains(expected),
        "expected rejection containing {expected:?}, got {reason:?}"
    );
}

/// Call a bucket-returning method on the job, signed by `signer`, and deposit the
/// returned bucket into `dest`'s account in the same transaction.
fn call_and_deposit(
    test: &mut TemplateTest,
    signer: &Party,
    job: ComponentAddress,
    method: &str,
    dest: &Party,
) {
    test.execute_expect_success(
        test.transaction()
            .call_method(job, method, args![])
            .put_last_instruction_output_on_workspace("out")
            .call_method(dest.account, "deposit", args![Workspace("out")])
            .build_and_seal(&signer.secret),
        vec![signer.proof.clone(), dest.proof.clone()],
    );
}

fn info(test: &mut TemplateTest, job: ComponentAddress) -> JobInfo {
    test.build_and_execute(
        test.transaction().call_method(job, "info", args![]),
        vec![],
    );
    test.expect_return(0)
}

/// Drive a job to the Delivered state: client posts, worker accepts and delivers.
fn delivered_job(test: &mut TemplateTest, client: &Party, worker: &Party, review_epochs: u64) -> ComponentAddress {
    let job = create_job(test, client, review_epochs);
    call_ok(test, worker, job, "accept_job", args![]);
    call_ok(test, worker, job, "submit_delivery", args![DELIVERY_HASH]);
    job
}

#[test]
fn happy_path_approve_pays_worker_exact_amount_and_receipt_is_issued() {
    let mut test = TemplateTest::my_crate();
    let client = make_party(&mut test);
    let worker = make_party(&mut test);

    let job = create_job(&mut test, &client, 10);
    assert_eq!(tari_balance(&test, client.account), Amount::from(START_BALANCE - JOB_AMOUNT));
    assert_eq!(tari_balance(&test, worker.account), Amount::from(START_BALANCE));

    let snapshot = info(&mut test, job);
    assert_eq!(snapshot.state, JobState::Open);
    assert_eq!(snapshot.client, client.public_key.to_byte_type());
    assert_eq!(snapshot.worker, None);
    assert_eq!(snapshot.task_hash, TASK_HASH);
    assert_eq!(snapshot.escrowed_amount, Amount::from(JOB_AMOUNT));
    assert_eq!(snapshot.review_epochs, 10);

    call_ok(&mut test, &worker, job, "accept_job", args![]);
    let snapshot = info(&mut test, job);
    assert_eq!(snapshot.state, JobState::Accepted);
    assert_eq!(snapshot.worker, Some(worker.public_key.to_byte_type()));

    call_ok(&mut test, &worker, job, "submit_delivery", args![DELIVERY_HASH]);
    let snapshot = info(&mut test, job);
    assert_eq!(snapshot.state, JobState::Delivered);
    assert_eq!(snapshot.delivery_hash.as_deref(), Some(DELIVERY_HASH));
    assert!(snapshot.delivered_epoch.is_some());

    // Client approves; the payout bucket is deposited to the worker in the same transaction.
    call_and_deposit(&mut test, &client, job, "approve", &worker);
    assert_eq!(tari_balance(&test, worker.account), Amount::from(START_BALANCE + JOB_AMOUNT));
    // The client does not get the escrow back.
    assert_eq!(tari_balance(&test, client.account), Amount::from(START_BALANCE - JOB_AMOUNT));

    let snapshot = info(&mut test, job);
    assert_eq!(snapshot.state, JobState::Released);
    assert_eq!(snapshot.escrowed_amount, Amount::zero());
    assert!(!snapshot.receipt_collected);

    // Worker collects the delivery receipt NFT.
    let receipt_resource = snapshot.receipt_resource;
    call_and_deposit(&mut test, &worker, job, "collect_receipt", &worker);
    assert_eq!(nft_count(&test, worker.account, receipt_resource), 1);
    assert!(info(&mut test, job).receipt_collected);
}

#[test]
fn silent_client_worker_claims_after_review_window() {
    let mut test = TemplateTest::my_crate();
    let client = make_party(&mut test);
    let worker = make_party(&mut test);

    set_epoch(&mut test, 100);
    let job = delivered_job(&mut test, &client, &worker, 10);
    let delivered_epoch = info(&mut test, job).delivered_epoch.expect("delivery recorded");
    assert_eq!(delivered_epoch, 100);

    // Move past delivered_epoch + review_epochs; the worker can now claim unilaterally.
    set_epoch(&mut test, 100 + 10);
    call_and_deposit(&mut test, &worker, job, "claim_timeout", &worker);
    assert_eq!(tari_balance(&test, worker.account), Amount::from(START_BALANCE + JOB_AMOUNT));
    assert_eq!(info(&mut test, job).state, JobState::Released);
}

#[test]
fn claim_before_review_window_is_rejected() {
    let mut test = TemplateTest::my_crate();
    let client = make_party(&mut test);
    let worker = make_party(&mut test);

    set_epoch(&mut test, 100);
    let job = delivered_job(&mut test, &client, &worker, 10);

    // One epoch too early.
    set_epoch(&mut test, 100 + 9);
    let reason = test.execute_expect_failure(
        test.transaction()
            .call_method(job, "claim_timeout", args![])
            .build_and_seal(&worker.secret),
        vec![worker.proof.clone()],
    );
    assert!(
        reason.to_string().contains("review window has not passed yet"),
        "unexpected rejection: {reason}"
    );
    // Funds are still escrowed.
    assert_eq!(info(&mut test, job).escrowed_amount, Amount::from(JOB_AMOUNT));
    assert_eq!(tari_balance(&test, worker.account), Amount::from(START_BALANCE));
}

#[test]
fn cancel_open_job_refunds_client_in_full() {
    let mut test = TemplateTest::my_crate();
    let client = make_party(&mut test);
    let _worker = make_party(&mut test);

    let job = create_job(&mut test, &client, 10);
    assert_eq!(tari_balance(&test, client.account), Amount::from(START_BALANCE - JOB_AMOUNT));

    call_and_deposit(&mut test, &client, job, "cancel", &client);
    assert_eq!(tari_balance(&test, client.account), Amount::from(START_BALANCE));
    let snapshot = info(&mut test, job);
    assert_eq!(snapshot.state, JobState::Refunded);
    assert_eq!(snapshot.escrowed_amount, Amount::zero());
}

#[test]
fn cancel_after_acceptance_is_rejected() {
    let mut test = TemplateTest::my_crate();
    let client = make_party(&mut test);
    let worker = make_party(&mut test);

    let job = create_job(&mut test, &client, 10);
    call_ok(&mut test, &worker, job, "accept_job", args![]);

    call_rejected(&mut test, &client, job, "cancel", args![], "only an open job can be cancelled");
    // Escrow untouched.
    assert_eq!(info(&mut test, job).escrowed_amount, Amount::from(JOB_AMOUNT));
}

#[test]
fn only_the_worker_can_deliver() {
    let mut test = TemplateTest::my_crate();
    let client = make_party(&mut test);
    let worker = make_party(&mut test);
    let stranger = make_party(&mut test);

    let job = create_job(&mut test, &client, 10);
    call_ok(&mut test, &worker, job, "accept_job", args![]);

    call_rejected(
        &mut test,
        &stranger,
        job,
        "submit_delivery",
        args![DELIVERY_HASH],
        "only the worker can submit a delivery",
    );
    // The client cannot deliver on the worker's behalf either.
    call_rejected(
        &mut test,
        &client,
        job,
        "submit_delivery",
        args![DELIVERY_HASH],
        "only the worker can submit a delivery",
    );
    assert_eq!(info(&mut test, job).state, JobState::Accepted);
}

#[test]
fn delivery_before_acceptance_is_rejected() {
    let mut test = TemplateTest::my_crate();
    let client = make_party(&mut test);
    let worker = make_party(&mut test);

    let job = create_job(&mut test, &client, 10);
    call_rejected(
        &mut test,
        &worker,
        job,
        "submit_delivery",
        args![DELIVERY_HASH],
        "job is not in progress",
    );
}

#[test]
fn only_the_client_can_approve() {
    let mut test = TemplateTest::my_crate();
    let client = make_party(&mut test);
    let worker = make_party(&mut test);
    let stranger = make_party(&mut test);

    let job = delivered_job(&mut test, &client, &worker, 10);

    call_rejected(&mut test, &stranger, job, "approve", args![], "only the client can approve");
    call_rejected(&mut test, &worker, job, "approve", args![], "only the client can approve");
    assert_eq!(info(&mut test, job).state, JobState::Delivered);
}

#[test]
fn client_cannot_accept_their_own_job() {
    let mut test = TemplateTest::my_crate();
    let client = make_party(&mut test);
    let _worker = make_party(&mut test);

    let job = create_job(&mut test, &client, 10);
    call_rejected(
        &mut test,
        &client,
        job,
        "accept_job",
        args![],
        "client cannot accept their own job",
    );
}

#[test]
fn escrow_cannot_be_released_twice() {
    let mut test = TemplateTest::my_crate();
    let client = make_party(&mut test);
    let worker = make_party(&mut test);

    set_epoch(&mut test, 100);
    let job = delivered_job(&mut test, &client, &worker, 10);

    call_and_deposit(&mut test, &client, job, "approve", &worker);
    assert_eq!(tari_balance(&test, worker.account), Amount::from(START_BALANCE + JOB_AMOUNT));

    // A second approval is impossible: there is no delivery left to approve.
    call_rejected(&mut test, &client, job, "approve", args![], "no delivery to approve");
    // And the timeout path is closed too, even long after the window.
    set_epoch(&mut test, 10_000);
    call_rejected(&mut test, &worker, job, "claim_timeout", args![], "no delivery to claim on");
    assert_eq!(tari_balance(&test, worker.account), Amount::from(START_BALANCE + JOB_AMOUNT));
}

#[test]
fn receipt_can_only_be_collected_once_and_only_by_the_worker() {
    let mut test = TemplateTest::my_crate();
    let client = make_party(&mut test);
    let worker = make_party(&mut test);
    let stranger = make_party(&mut test);

    let job = delivered_job(&mut test, &client, &worker, 10);
    call_and_deposit(&mut test, &client, job, "approve", &worker);
    let receipt_resource = info(&mut test, job).receipt_resource;

    call_rejected(
        &mut test,
        &stranger,
        job,
        "collect_receipt",
        args![],
        "only the worker can collect the receipt",
    );
    call_and_deposit(&mut test, &worker, job, "collect_receipt", &worker);
    assert_eq!(nft_count(&test, worker.account, receipt_resource), 1);

    call_rejected(
        &mut test,
        &worker,
        job,
        "collect_receipt",
        args![],
        "receipt already collected",
    );
    assert_eq!(nft_count(&test, worker.account, receipt_resource), 1);
}
