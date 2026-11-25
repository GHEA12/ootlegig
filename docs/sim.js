/**
 * OotleGig browser simulation engine.
 *
 * A faithful, dependency-free mirror of the Rust `JobEscrow` template in
 * `../template/src/lib.rs`: same state machine, same checks, in the same
 * order, with the same rejection messages. If a rule changes in the template,
 * it must change here too (and vice versa) — `sim.test.mjs` pins the rules.
 *
 * This is a SIMULATION. No wallet, no network, no real funds are involved.
 * The template itself only ever sees hashes, so the simulation does too:
 * task and delivery plaintext never enter the state machine.
 */

export const JobState = Object.freeze({
  Open: "Open",
  Accepted: "Accepted",
  Delivered: "Delivered",
  Released: "Released",
  Refunded: "Refunded",
});

export const START_BALANCE = 1_000_000_000; // µT, mirrors TemplateTest's funded accounts

export class GigSim {
  constructor() {
    this.epoch = 0;
    this.balances = {
      client: START_BALANCE,
      worker: START_BALANCE,
      stranger: START_BALANCE,
    };
    this.receipts = { client: 0, worker: 0, stranger: 0 };
    this.job = null;
    this.events = [];
  }

  #emit(name, detail) {
    this.events.push({ epoch: this.epoch, name, detail });
  }

  #release() {
    const job = this.job;
    job.state = JobState.Released;
    const amount = job.escrowed;
    job.escrowed = 0;
    this.balances[job.worker] += amount;
    // The component mints the receipt NFT into its own receipt vault;
    // the worker collects it separately, exactly like the template.
    job.receiptMinted = true;
    this.#emit("ootle_gig.released", { amount });
    return amount;
  }

  /**
   * Post a job. The poster becomes the client (in the template, the
   * transaction signer). Mirrors `JobEscrow::new`.
   */
  postJob(actor, amount, taskHash, reviewEpochs) {
    if (this.job && this.job.state !== JobState.Released && this.job.state !== JobState.Refunded) {
      throw new Error("simulation: finish or reset the current job first");
    }
    if (!(amount > 0)) throw new Error("OotleGig: job payment must be non-zero");
    if (!taskHash) throw new Error("OotleGig: task_hash must not be empty");
    if (this.balances[actor] < amount) throw new Error("insufficient funds in account");

    this.balances[actor] -= amount;
    this.job = {
      client: actor,
      worker: null,
      taskHash,
      escrowed: amount,
      state: JobState.Open,
      deliveryHash: null,
      deliveredEpoch: null,
      reviewEpochs,
      receiptMinted: false,
      receiptCollected: false,
    };
    this.#emit("ootle_gig.posted", { amount });
    return this.job;
  }

  /** Mirrors `accept_job`. The signer becomes the worker. */
  acceptJob(actor) {
    const job = this.job;
    if (!job || job.state !== JobState.Open) throw new Error("OotleGig: job is not open");
    if (actor === job.client) throw new Error("OotleGig: client cannot accept their own job");
    job.worker = actor;
    job.state = JobState.Accepted;
    this.#emit("ootle_gig.accepted", {});
  }

  /** Mirrors `submit_delivery`. */
  submitDelivery(actor, deliveryHash) {
    const job = this.job;
    if (!job || job.state !== JobState.Accepted) throw new Error("OotleGig: job is not in progress");
    if (!deliveryHash) throw new Error("OotleGig: delivery_hash must not be empty");
    if (actor !== job.worker) throw new Error("OotleGig: only the worker can submit a delivery");
    job.deliveryHash = deliveryHash;
    job.deliveredEpoch = this.epoch;
    job.state = JobState.Delivered;
    this.#emit("ootle_gig.delivered", {});
  }

  /** Mirrors `approve`. Pays the worker and mints the receipt. */
  approve(actor) {
    const job = this.job;
    if (!job || job.state !== JobState.Delivered) throw new Error("OotleGig: no delivery to approve");
    if (actor !== job.client) throw new Error("OotleGig: only the client can approve");
    return this.#release();
  }

  /** Mirrors `claim_timeout`. */
  claimTimeout(actor) {
    const job = this.job;
    if (!job || job.state !== JobState.Delivered) throw new Error("OotleGig: no delivery to claim on");
    if (actor !== job.worker) {
      throw new Error("OotleGig: only the worker can claim after the review window");
    }
    if (this.epoch < job.deliveredEpoch + job.reviewEpochs) {
      throw new Error("OotleGig: review window has not passed yet");
    }
    return this.#release();
  }

  /** Mirrors `cancel`. Refunds the client in full. */
  cancel(actor) {
    const job = this.job;
    if (!job || job.state !== JobState.Open) {
      throw new Error("OotleGig: only an open job can be cancelled");
    }
    if (actor !== job.client) throw new Error("OotleGig: only the client can cancel");
    job.state = JobState.Refunded;
    const refund = job.escrowed;
    job.escrowed = 0;
    this.balances[job.client] += refund;
    this.#emit("ootle_gig.refunded", {});
    return refund;
  }

  /** Mirrors `collect_receipt`. */
  collectReceipt(actor) {
    const job = this.job;
    if (!job || job.state !== JobState.Released) throw new Error("OotleGig: job is not released yet");
    if (job.receiptCollected) throw new Error("OotleGig: receipt already collected");
    if (actor !== job.worker) throw new Error("OotleGig: only the worker can collect the receipt");
    job.receiptCollected = true;
    this.receipts[actor] += 1;
    return true;
  }

  advanceEpoch(n = 1) {
    this.epoch += n;
    return this.epoch;
  }

  reset() {
    const fresh = new GigSim();
    this.epoch = fresh.epoch;
    this.balances = fresh.balances;
    this.receipts = fresh.receipts;
    this.job = null;
    this.events = [];
  }
}
