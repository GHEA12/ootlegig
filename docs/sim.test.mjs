/**
 * Rule-parity tests for the OotleGig browser simulation.
 *
 * Run with:  node --test app/sim.test.mjs
 *
 * These pin the same rules the Rust engine tests pin for the template
 * (same rejection messages), so the simulation cannot quietly drift
 * from the on-chain logic it claims to mirror.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { GigSim, JobState, START_BALANCE } from "./sim.js";

const TASK_HASH = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"; // sha256("test")
const DELIVERY_HASH = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"; // sha256("hello")
const AMOUNT = 400_000_000;

function delivered(reviewEpochs = 10) {
  const sim = new GigSim();
  sim.postJob("client", AMOUNT, TASK_HASH, reviewEpochs);
  sim.acceptJob("worker");
  sim.submitDelivery("worker", DELIVERY_HASH);
  return sim;
}

function rejects(fn, message) {
  assert.throws(fn, (err) => err.message === message, `expected rejection: ${message}`);
}

test("happy path: approve pays the worker the exact escrow and issues a receipt", () => {
  const sim = new GigSim();
  sim.postJob("client", AMOUNT, TASK_HASH, 10);
  assert.equal(sim.balances.client, START_BALANCE - AMOUNT);
  assert.equal(sim.job.state, JobState.Open);
  assert.equal(sim.job.escrowed, AMOUNT);

  sim.acceptJob("worker");
  assert.equal(sim.job.state, JobState.Accepted);
  assert.equal(sim.job.worker, "worker");

  sim.submitDelivery("worker", DELIVERY_HASH);
  assert.equal(sim.job.state, JobState.Delivered);
  assert.equal(sim.job.deliveryHash, DELIVERY_HASH);
  assert.equal(sim.job.deliveredEpoch, 0);

  sim.approve("client");
  assert.equal(sim.job.state, JobState.Released);
  assert.equal(sim.balances.worker, START_BALANCE + AMOUNT);
  assert.equal(sim.balances.client, START_BALANCE - AMOUNT);
  assert.equal(sim.job.receiptMinted, true);

  sim.collectReceipt("worker");
  assert.equal(sim.receipts.worker, 1);
});

test("silent client: worker claims after the review window", () => {
  const sim = delivered(10);
  sim.advanceEpoch(10);
  sim.claimTimeout("worker");
  assert.equal(sim.job.state, JobState.Released);
  assert.equal(sim.balances.worker, START_BALANCE + AMOUNT);
});

test("claim before the review window is rejected and the escrow is untouched", () => {
  const sim = delivered(10);
  sim.advanceEpoch(9);
  rejects(() => sim.claimTimeout("worker"), "OotleGig: review window has not passed yet");
  assert.equal(sim.job.escrowed, AMOUNT);
  assert.equal(sim.balances.worker, START_BALANCE);
});

test("cancel of an open job refunds the client in full", () => {
  const sim = new GigSim();
  sim.postJob("client", AMOUNT, TASK_HASH, 10);
  sim.cancel("client");
  assert.equal(sim.job.state, JobState.Refunded);
  assert.equal(sim.balances.client, START_BALANCE);
  assert.equal(sim.job.escrowed, 0);
});

test("cancel after acceptance is rejected", () => {
  const sim = new GigSim();
  sim.postJob("client", AMOUNT, TASK_HASH, 10);
  sim.acceptJob("worker");
  rejects(() => sim.cancel("client"), "OotleGig: only an open job can be cancelled");
  assert.equal(sim.job.escrowed, AMOUNT);
});

test("only the worker can deliver", () => {
  const sim = new GigSim();
  sim.postJob("client", AMOUNT, TASK_HASH, 10);
  sim.acceptJob("worker");
  rejects(() => sim.submitDelivery("stranger", DELIVERY_HASH), "OotleGig: only the worker can submit a delivery");
  rejects(() => sim.submitDelivery("client", DELIVERY_HASH), "OotleGig: only the worker can submit a delivery");
  assert.equal(sim.job.state, JobState.Accepted);
});

test("delivery before acceptance is rejected", () => {
  const sim = new GigSim();
  sim.postJob("client", AMOUNT, TASK_HASH, 10);
  rejects(() => sim.submitDelivery("worker", DELIVERY_HASH), "OotleGig: job is not in progress");
});

test("only the client can approve", () => {
  const sim = delivered();
  rejects(() => sim.approve("stranger"), "OotleGig: only the client can approve");
  rejects(() => sim.approve("worker"), "OotleGig: only the client can approve");
  assert.equal(sim.job.state, JobState.Delivered);
});

test("client cannot accept their own job", () => {
  const sim = new GigSim();
  sim.postJob("client", AMOUNT, TASK_HASH, 10);
  rejects(() => sim.acceptJob("client"), "OotleGig: client cannot accept their own job");
});

test("escrow cannot be released twice", () => {
  const sim = delivered(10);
  sim.approve("client");
  rejects(() => sim.approve("client"), "OotleGig: no delivery to approve");
  sim.advanceEpoch(10_000);
  rejects(() => sim.claimTimeout("worker"), "OotleGig: no delivery to claim on");
  assert.equal(sim.balances.worker, START_BALANCE + AMOUNT);
});

test("receipt can only be collected once, and only by the worker", () => {
  const sim = delivered();
  sim.approve("client");
  rejects(() => sim.collectReceipt("stranger"), "OotleGig: only the worker can collect the receipt");
  sim.collectReceipt("worker");
  assert.equal(sim.receipts.worker, 1);
  rejects(() => sim.collectReceipt("worker"), "OotleGig: receipt already collected");
});

test("posting validates amount and task hash like the template", () => {
  const sim = new GigSim();
  rejects(() => sim.postJob("client", 0, TASK_HASH, 10), "OotleGig: job payment must be non-zero");
  rejects(() => sim.postJob("client", AMOUNT, "", 10), "OotleGig: task_hash must not be empty");
});
