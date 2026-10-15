# WillDeep Agent RSI P4a P4b shared contract v1

Frozen working contract, 2026-10-15. The JSON in golden.json is authoritative for exact keys/types. Changes are coordinated by root; implementations must not silently accept unknown fields. Existing agent-definition.v1 stays unchanged.

## Canonical and validation

Canonical encoding uses the existing agent-definition v1 string rules: sort object keys lexically, no whitespace, UTF-16 non-ASCII/control code units as lowercase \\uXXXX (including newline), escape quote/backslash only otherwise, preserve ASCII slash and HTML characters. Extend values with nonnegative integers in 0..9007199254740991 rendered in decimal; reject negative numbers, fractions, exponent notation, duplicate JSON keys, trailing input and unknown schema keys. Hash is lowercase SHA256 of canonical UTF-8 bytes. Arrays preserve order. All fields shown in each golden object are required, including explicit null. Runtime display text can be Unicode; identifier fields match [a-zA-Z0-9._:/-]{1,128}. Hash fields are lowercase 64 hex; nullable hash explicitly permits null only where golden has null.

Golden wrapper contains samples, canonical strings and SHA256 values; wrapper itself is not a wire entity. Source package baseline/candidate are agent-definition.v1 packages with real canonical hashes; no artificial hash placeholders for their revision.

## ExperimentSpec

schema=willdeep.agent-rsi.experiment.v1. Candidate is one instructions-only change, same ID/base_role/tools/skills/modalities and unchanged host permissions. Candidate must have distinct immutable revision/version and forked_from=baseline_revision. Register using candidate_id separately; experiment references those package hashes. No registration calls install/select/agentregistry PUT/official POST.

Spec keys: schema, experiment_id, grant_id(nullable for local only), candidate_id, definition_id, baseline_revision, candidate_revision, hypothesis, execution_mode(direct|team), runtime, model, conditions, dataset, budget, gate, repetitions(1..10), execution_policy(offline_only).

runtime: adapter, product_version, commit, binary_sha256, platform, architecture.
model: provider, model, model_revision(nullable), parameters_sha256, routing_policy_sha256. Offline spec requires provider=mock; real names yield paid_not_approved before any provider call.
conditions: runtime_parameters_sha256, tools_sha256, skills_sha256, workspace_sha256, rules_sha256, memory_sha256.
skills_sha256 is SHA256(canonical(definition.skills)), including the full ordered array of id/content/sha256/required objects. Do not hash only a list of content hashes; the order, IDs and required flags are part of the dependency contract. The runtime must confirm actual locked skills match that manifest. skills-golden.json supplies a nonempty Unicode dependency fixture in addition to golden.json's empty skills.
dataset: suite_id, suite_sha256, fixture_sha256, verifier_sha256, split_sha256, tasks. Each task: task_id, split(validation|regression|holdout), task_sha256. Unique task IDs, nonempty list <=128, all three splits required for a complete experiment. No question/answer text in the service Spec.
budget: max_turns(1..1000), max_tokens(1..10000000), timeout_seconds(1..86400), max_attempts(1..10), max_parallel_trials(1..4), max_candidates(1 for this single-candidate v1), max_holdout_accesses(1..128), max_cost_microusd(0 for offline). Unknown measured cost is null, never zero by implication. A different candidate requires a separate experiment; this phase cannot claim a global paid budget guarantee across experiments.
gate: gate_id, gate_sha256. Gate settings themselves are frozen in local fixture/config; the hash binds them. Manifest changes after registration create another experiment.

## ExecutionEvidence

schema=willdeep.agent-rsi.evidence.v1. Exact fields in golden. runtime/model/conditions/budget schemas reused from Spec; model_route is ordered actual [{provider,model,model_revision(nullable)}]. If target_executed=false and status is not passed, an empty route is permitted to record an interrupted admission with no model execution; never fabricate a route. target_executed=true or passed requires a nonempty actual route matching the frozen model policy. task_sha256/split checked against manifest; dataset_sha256 equals suite_sha256; fixture_sha256 and verifier_sha256 match declared dataset. skills_sha256 matches conditions.skills_sha256 and actual loaded skills, not merely request.

trial_id, experiment_id, task_id, arm(baseline|candidate), repetition(1-based), attempt(1-based), resume_of(nullable previous trial segment ID), root_run_id, worker_run_id(nullable), definition_id, definition_revision, effective_prompt_sha256, skills_sha256, runtime, model, model_route, conditions, budget, dataset_sha256, fixture_sha256, verifier, target_executed, started_at(RFC3339 UTC), finished_at(nullable RFC3339 UTC), status, usage, events_sha256(nullable), trust(local_mock|client_reported).

verifier: verifier_id, verifier_sha256, input_sha256, result(passed|failed|unverified), exit_code(nullable nonnegative integer). input_sha256 hashes the actual verifier inputs under the named adapter's documented encoding; it is independent of task_sha256 and must not be replaced with a task-manifest hash. Hosts retain the underlying inputs or immutable local references for revalidation. Unexecuted orphan records use an explicit unexecuted-input marker and remain unverified. The service validates digest syntax and Spec bindings, while client-reported input hashes do not establish trusted execution. usage: input_tokens, output_tokens, auxiliary_tokens, elapsed_ms, cost_microusd; all nullable nonnegative safe integers. status: passed|failed|unverified|infra_error|interrupted|budget_exhausted|invalid. passed requires target_executed, nonnull worker_run_id/finished_at/events_sha256, verifier passed and exit_code=0. Missing unfinished start cannot be completed by inference. Nonnull resume_of stays same arm/task/repetition lineage, increasing attempt, never expands independent sample count. A retry must reference the immediately preceding terminal interrupted or infra_error segment; failed, passed, budget_exhausted, unverified or invalid do not authorize a new attempt. Retry records retained; don't choose best attempt.

Evidence persisted locally may omit experiment binding only through a separate runtime-event schema; do not fabricate ExperimentSpec/task hashes for normal user runs. Link those actual events into complete evidence only inside an explicitly registered trial. Normal root feedback remains unverified by default. Server always stores incoming evidence at client_reported trust; never accept trusted=true/runner claims from client.

## Grant Candidate and admission

Grant create body: {schema:willdeep.agent-rsi.grant.v1,grant_id,purpose:private_evaluation|official_contribution,material_scope:[execution_summary],expires_at:RFC3339 UTC}. Service derives owner user/tenant from authenticated customer session; no client owner fields. Expired/revoked grant blocks reads/writes as appropriate and all new admissions; immutable audit metadata may remain. No raw prompt/source/path/command output upload in v1.

Candidate create body: {schema:willdeep.agent-rsi.candidate.v1,candidate_id,grant_id,baseline_package,candidate_package}. Both packages existing agent-definition.v1 validated, canonical hash verified, candidate only instructions/version/forked_from differ (display purpose must remain unchanged for initial controlled experiment). Same candidate ID + canonical body is idempotent; different body conflicts. Owner-scoped independent storage.

Admission body: {schema:willdeep.agent-rsi.admission.v1,trial_id,task_id,arm,repetition,attempt,resume_of,estimated_cost_microusd}. Offline estimate=0, server hard refuses any non-mock/paid request. Lock grant and experiment and reserve attempt/concurrency/holdout quota atomically. Repeated same body must not charge twice. Holdout is one access per distinct admitted trial (including retries), monotonic, no refund that would allow answer reuse. The server also fixes the initial holdout ceiling and sums accesses across experiments sharing owner+grant+suite_sha256; a new experiment ID or larger requested ceiling cannot reset that authorization's counter. A new separately created grant remains a separate user authorization, not a trusted official campaign. Finished evidence releases active concurrency once. Interrupted/abandoned trial is finalized explicitly as interrupted, never guessed successful; no silent lease refund/reuse. max_attempts applies per task/arm/repetition. No automatic provider retries outside recorded budget.

## HTTP API

All customer-authenticated at /api/v1/customer/agent-rsi. Preserve existing API response envelope and X-App-Version. Root clients unwrap the normal data envelope.

- POST /grants ; GET /grants ; DELETE /grants/{id} revokes (no token required in JSON).
- POST /candidates ; GET /candidates ; GET /candidates/{id}.
- POST /experiments ; GET /experiments ; GET /experiments/{id}.
- POST /experiments/{id}/admissions using Admission body.
- POST /experiments/{id}/evidence using exact ExecutionEvidence body; trial must already be admitted. GET /experiments/{id}/evidence returns immutable items.

Mutating requests accept Idempotency-Key, scope it to owner+route. Same key/body returns original result, different body conflicts. IDs are validated before SQL/path use. Missing or inaccessible object returns consistent 404; membership rechecked per request. Revoke/admit race serialized; invalid metadata cannot affect current registry heads/installations. Paid execution/official publication APIs deliberately absent in this phase.

## Offline runner and tests

Consumers copy golden.json byte-for-byte into their repo. Verify shared canonical hashes, duplicate/unknown/integer validation, direct effective revision, no activation changes, missing target/verifier, orphan starts, resume accounting, budget/concurrency/holdout, owner isolation and revoke races. Trial order supplied by a suite cannot bypass stages: every planned validation and regression task/arm/repetition must be terminal and passed before any holdout access. Reordering tasks must not open holdout early. Three runtime fingerprints differ by platform; never label same scores cross-platform. An integration test can replace the sample runtime fingerprints with actual native adapter fingerprints and produce new full evidence hashes.

Root acceptance uses deterministic mock Provider with normal tool/verifier execution, then localhost service APIs on an isolated database and home. No production, existing user installs, provider endpoint or model spend. Reports JSON + Markdown, separate evidence from quality improvement. Real-model preflight proposal contains exact models, task splits/counts, repeats, arms, maximum calls/tokens, price source and total cap; approval of that proposal is a separate step.
