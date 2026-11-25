# OotleGig — escrowed micro-jobs with verifiable delivery receipts

**A Tari November Build Contest 2026 (MVP) entry, by Indra Dwi Junianto. MIT licensed.**

OotleGig is a two-party work contract for Tari Ootle. A client posts a job by
escrowing TARI against a hash of the task; a worker accepts, delivers, and records
the delivery hash on-chain; the client approves and the worker is paid — and if the
client goes silent, the worker claims the same payout after a review window. Either
way the job mints a **delivery receipt NFT** that only that job's component can ever
mint, which the worker collects as portable proof of delivered, paid work.

No platform account, no custody beyond the template, no fee. One job = one component
with no owner and no upgrade path: nobody can rewrite a job's rules after it is posted.

## Try it

**Browser simulation (no wallet, no funds):** open `docs/index.html` — or serve the
folder with any static file server. It runs the template's exact rules in your
browser: same state machine, same signer checks, same rejection messages, with a
manual epoch clock so you can watch the review window expire. It is labelled a
simulation on the page, because it is one.

## What's in the box

| Part | Where | What it proves |
| --- | --- | --- |
| Rust Ootle template | `template/src/lib.rs` | The on-chain rules: escrow, signer-checked accept/deliver/approve/claim/cancel, epoch review window, receipt NFT minting locked to the component |
| Engine tests | `template/tests/job_escrow.rs` | The template's behavior against the real Ootle engine, in-process |
| Browser simulation | `docs/` (`sim.js`, `index.html`), served by GitHub Pages | The judge-clickable demo; `sim.js` mirrors the template rule-for-rule |
| Sim tests | `docs/sim.test.mjs` | The mirror cannot drift from the template's rules unnoticed |

## Verify it yourself

```sh
# Engine tests (real Ootle engine, in-process; no network or funds needed after the first dependency download)
cd template && cargo test

# Simulation rule-parity tests
node --test docs/sim.test.mjs

# Build the template WASM
cd template && cargo build --target wasm32-unknown-unknown --release
```

## How the template works

State machine: `Open → Accepted → Delivered → Released`, or `Open → Refunded`.

- `new(payment, task_hash, review_epochs)` — the signer becomes the **client**; the
  payment (TARI only, non-zero) is escrowed in the component's vault. The constructor
  also creates the job's receipt NFT resource, with its mint rule locked to this
  component's address (allocated up front) — only this job can ever mint its receipt.
- `accept_job()` — the signer becomes the **worker**. The client cannot accept their
  own job.
- `submit_delivery(delivery_hash)` — worker only. Records the hash and the current
  epoch; the review window starts now.
- `approve()` — client only. Pays the full escrow out (the transaction routes the
  bucket to the worker) and mints the receipt NFT into the component's receipt vault.
- `claim_timeout()` — worker only, once `current_epoch ≥ delivered_epoch + review_epochs`.
  The silent-client path: same payout, same receipt.
- `cancel()` — client only, only while the job is still open. Full refund.
- `collect_receipt()` — worker only, after release, exactly once.
- `info()` — read-only snapshot: parties, hashes, epochs, escrowed amount, state.

Authorization is enforced in code against the transaction signer
(`CallerContext::transaction_signer_public_key`), and the component is created with
`OwnerRule::None`, so its access rules can never be changed afterwards.

## Honest limits

- The template has **not** been published to Esmeralda or mainnet yet, and is not
  audited. The browser demo is a simulation and says so on the page.
- **Disputes are out of scope for v1.** A client who dislikes a delivery simply does
  not approve it; the worker's protections are the on-chain delivery record and the
  timeout claim when the client goes silent. Arbitration is a v2 design question.
- Only the hashes of the task and the delivery go on-chain. The texts themselves,
  and the work itself, move off-chain between client and worker.
- Amounts and parties are public, as with any vault-based template.

## Roadmap

Wallet-connected app flow (Sapient) on top of this template, an Esmeralda publish,
then milestone jobs (multiple deliveries per job) and an optional dispute window
with a named arbiter.
