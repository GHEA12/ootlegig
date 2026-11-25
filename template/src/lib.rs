//! OotleGig — escrowed micro-jobs with verifiable delivery receipts on Tari Ootle.
//!
//! A client escrows TARI into a `JobEscrow` component against a hash of the task.
//! A worker accepts the job, delivers off-chain, and records the delivery hash on-chain.
//! The client approves and the worker is paid; if the client goes silent, the worker can
//! claim the escrow after a review window measured in epochs. Either way, the component
//! mints a delivery receipt NFT that only it can mint, which the worker collects as a
//! portable proof of delivered, paid work.
//!
//! Authorization is enforced in code against the transaction signer
//! (`CallerContext::transaction_signer_public_key`). The component has no owner
//! (`OwnerRule::None`), so nobody — not even the client who created it — can change
//! its rules after the job is posted.

use tari_template_lib::prelude::*;

/// Lifecycle of a job. `Open -> Accepted -> Delivered -> Released`, or `Open -> Refunded`.
///
/// Defined outside the template module (with explicit CBOR indices) so integration
/// tests can decode it with exactly the wire format the engine uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, minicbor::Encode, minicbor::Decode, minicbor::CborLen)]
pub enum JobState {
    #[n(0)]
    Open,
    #[n(1)]
    Accepted,
    #[n(2)]
    Delivered,
    #[n(3)]
    Released,
    #[n(4)]
    Refunded,
}

/// Read-only snapshot of a job, returned by the template's `info` method.
#[derive(Debug, Clone, PartialEq, Eq, minicbor::Encode, minicbor::Decode, minicbor::CborLen)]
pub struct JobInfo {
    #[n(0)]
    pub client: RistrettoPublicKeyBytes,
    #[n(1)]
    pub worker: Option<RistrettoPublicKeyBytes>,
    #[n(2)]
    pub task_hash: String,
    #[n(3)]
    pub delivery_hash: Option<String>,
    #[n(4)]
    pub delivered_epoch: Option<u64>,
    #[n(5)]
    pub review_epochs: u64,
    #[n(6)]
    pub escrowed_amount: Amount,
    #[n(7)]
    pub state: JobState,
    #[n(8)]
    pub receipt_resource: ResourceAddress,
    #[n(9)]
    pub receipt_collected: bool,
}

#[template]
mod ootle_gig {
    use super::*;

    pub struct JobEscrow {
        /// Public key of the client who funded the job. Only this signer can approve or cancel.
        client: RistrettoPublicKeyBytes,
        /// Public key of the worker who accepted the job. Only this signer can deliver or claim.
        worker: Option<RistrettoPublicKeyBytes>,
        /// Hex SHA-256 of the task description, committed by the client at posting time.
        /// The task text itself never goes on-chain.
        task_hash: String,
        /// The escrowed payment.
        payment: Vault,
        state: JobState,
        /// Hex SHA-256 of the delivered work, recorded by the worker at delivery time.
        delivery_hash: Option<String>,
        /// Epoch at which the delivery was recorded; the review window counts from here.
        delivered_epoch: Option<u64>,
        /// How many epochs the client has after delivery to approve before the worker
        /// may claim the escrow unilaterally.
        review_epochs: u64,
        /// The receipt NFT resource. Created in the constructor with its mint rule locked
        /// to this component, so only this job can ever mint its own receipt.
        receipt_resource: ResourceAddress,
        /// Vault holding the minted receipt NFT between release and collection.
        receipt_vault: Vault,
        receipt_collected: bool,
    }

    impl JobEscrow {
        /// Post a new job, escrowing `payment` (must be TARI) against `task_hash`.
        ///
        /// The transaction signer becomes the client. Returns the created component.
        pub fn new(payment: Bucket, task_hash: String, review_epochs: u64) -> Component<Self> {
            assert!(
                payment.resource_address() == TARI_TOKEN,
                "OotleGig: jobs must be funded in TARI"
            );
            assert!(!payment.is_empty(), "OotleGig: job payment must be non-zero");
            assert!(!task_hash.is_empty(), "OotleGig: task_hash must not be empty");

            let client = CallerContext::transaction_signer_public_key();

            // Allocate the component address first so the receipt resource's mint rule can
            // name this component before the component exists. The rule is then LOCKED:
            // no one can ever change who may mint this job's receipt.
            let address_allocation = CallerContext::allocate_component_address(None);
            let component_address = address_allocation.get_address();

            let receipt_resource = ResourceBuilder::non_fungible()
                .with_token_symbol("OGIG")
                .add_metadata("name", "OotleGig Delivery Receipt")
                .mintable(rule!(component(component_address)), LOCKED)
                .build();

            let payment_vault = Vault::new_empty(TARI_TOKEN);
            let amount = payment.amount();
            payment_vault.deposit(payment);

            let receipt_vault = Vault::new_empty(receipt_resource);

            let state = Self {
                client,
                worker: None,
                task_hash,
                payment: payment_vault,
                state: JobState::Open,
                delivery_hash: None,
                delivered_epoch: None,
                review_epochs,
                receipt_resource,
                receipt_vault,
                receipt_collected: false,
            };

            emit_event("ootle_gig.posted", metadata_event(&[("amount", amount.to_string())]));

            Component::new(state)
                .with_address_allocation(address_allocation)
                .with_access_rules(ComponentAccessRules::allow_all())
                .with_owner_rule(OwnerRule::None)
                .create()
        }

        /// Accept an open job. The transaction signer becomes the worker.
        pub fn accept_job(&mut self) {
            assert!(self.state == JobState::Open, "OotleGig: job is not open");
            let worker = CallerContext::transaction_signer_public_key();
            assert!(worker != self.client, "OotleGig: client cannot accept their own job");
            self.worker = Some(worker);
            self.state = JobState::Accepted;
            emit_event("ootle_gig.accepted", metadata_event(&[]));
        }

        /// Record delivery of the work. Only the worker, only after acceptance.
        /// Stores the delivery hash and the current epoch; the review window starts now.
        pub fn submit_delivery(&mut self, delivery_hash: String) {
            assert!(self.state == JobState::Accepted, "OotleGig: job is not in progress");
            assert!(!delivery_hash.is_empty(), "OotleGig: delivery_hash must not be empty");
            let signer = CallerContext::transaction_signer_public_key();
            assert!(
                Some(signer) == self.worker,
                "OotleGig: only the worker can submit a delivery"
            );
            self.delivery_hash = Some(delivery_hash);
            self.delivered_epoch = Some(Consensus::current_epoch());
            self.state = JobState::Delivered;
            emit_event("ootle_gig.delivered", metadata_event(&[]));
        }

        /// Approve the delivery and release the escrow. Only the client, only after delivery.
        /// Returns the payment bucket; the transaction routes it to the worker's account.
        /// The delivery receipt NFT is minted into the component's receipt vault for the
        /// worker to collect with `collect_receipt`.
        pub fn approve(&mut self) -> Bucket {
            assert!(self.state == JobState::Delivered, "OotleGig: no delivery to approve");
            let signer = CallerContext::transaction_signer_public_key();
            assert!(signer == self.client, "OotleGig: only the client can approve");
            self.release()
        }

        /// Claim the escrow after the review window has passed without client approval.
        /// Only the worker, only after delivery, only once
        /// `delivered_epoch + review_epochs` has been reached.
        pub fn claim_timeout(&mut self) -> Bucket {
            assert!(self.state == JobState::Delivered, "OotleGig: no delivery to claim on");
            let signer = CallerContext::transaction_signer_public_key();
            assert!(
                Some(signer) == self.worker,
                "OotleGig: only the worker can claim after the review window"
            );
            let delivered = self.delivered_epoch.expect("delivered_epoch is set when Delivered");
            let now = Consensus::current_epoch();
            assert!(
                now >= delivered + self.review_epochs,
                "OotleGig: review window has not passed yet"
            );
            self.release()
        }

        /// Cancel an open job before any worker accepts it and refund the escrow in full.
        /// Only the client. Returns the payment bucket.
        pub fn cancel(&mut self) -> Bucket {
            assert!(
                self.state == JobState::Open,
                "OotleGig: only an open job can be cancelled"
            );
            let signer = CallerContext::transaction_signer_public_key();
            assert!(signer == self.client, "OotleGig: only the client can cancel");
            self.state = JobState::Refunded;
            let refund = self.payment.withdraw_all();
            emit_event("ootle_gig.refunded", metadata_event(&[]));
            refund
        }

        /// Collect the delivery receipt NFT minted at release. Only the worker,
        /// only after release, only once.
        pub fn collect_receipt(&mut self) -> Bucket {
            assert!(self.state == JobState::Released, "OotleGig: job is not released yet");
            assert!(!self.receipt_collected, "OotleGig: receipt already collected");
            let signer = CallerContext::transaction_signer_public_key();
            assert!(
                Some(signer) == self.worker,
                "OotleGig: only the worker can collect the receipt"
            );
            self.receipt_collected = true;
            self.receipt_vault.withdraw_all()
        }

        /// Read-only snapshot of the job.
        pub fn info(&self) -> JobInfo {
            JobInfo {
                client: self.client,
                worker: self.worker,
                task_hash: self.task_hash.clone(),
                delivery_hash: self.delivery_hash.clone(),
                delivered_epoch: self.delivered_epoch,
                review_epochs: self.review_epochs,
                escrowed_amount: self.payment.balance(),
                state: self.state,
                receipt_resource: self.receipt_resource,
                receipt_collected: self.receipt_collected,
            }
        }

        /// Shared release path for `approve` and `claim_timeout`: pay out the full escrow
        /// and mint the delivery receipt NFT into the receipt vault.
        fn release(&mut self) -> Bucket {
            self.state = JobState::Released;
            let payout = self.payment.withdraw_all();
            let amount = payout.amount();

            let mut receipt_metadata = Metadata::new();
            receipt_metadata.insert("task_hash", &self.task_hash);
            receipt_metadata.insert(
                "delivery_hash",
                &self.delivery_hash.clone().unwrap_or_default(),
            );
            receipt_metadata.insert("amount", &amount.to_string());
            let receipt = ResourceManager::get(self.receipt_resource).mint_non_fungible(
                NonFungibleId::from_u64(1),
                &receipt_metadata,
                &(),
            );
            self.receipt_vault.deposit(receipt);

            emit_event(
                "ootle_gig.released",
                metadata_event(&[("amount", amount.to_string())]),
            );
            payout
        }
    }

    /// Build event payload metadata from string pairs.
    fn metadata_event(pairs: &[(&str, String)]) -> Metadata {
        let mut metadata = Metadata::new();
        for (key, value) in pairs {
            metadata.insert(*key, value);
        }
        metadata
    }
}
