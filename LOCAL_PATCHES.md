# IronClaw fork — local patch series

This fork carries a **small, rebasable series** on top of upstream
`nearai/ironclaw`. Keep it that way: every change is one focused `local:`
commit on `local-patches`, never an uncommitted working-tree edit.

- **Upstream:**    `origin`  = https://github.com/nearai/ironclaw.git
- **Personal fork:** `fork`  = https://github.com/nick-stebbings/ironclaw.git
- **Working branch:** `local-patches`  (built + deployed by `scripts/build-all.sh`)
- **`main`** tracks upstream only — do not commit local work there.

## Staying up to date

```bash
cd /opt/ironclaw/src
git checkout local-patches
bash scripts/sync-upstream.sh     # fetch + backup + rebase onto origin/main
# then: build-all.sh -> canary drew-claw -> restart pilot
```

Rebase on a regular cadence (monthly, or before adopting an upstream feature)
so the diff never grows unmanageable. On each rebase, drop any local patch that
upstream has since implemented (`git rebase -i origin/main`).

## The series (top of `local-patches`, oldest first)

_Last rebased onto `origin/main` on **2026-07-16** (upstream tip `7ae6c411b`)._

1. **`build-all.sh` feature flags** — builds `ironclaw_reborn_cli` with `webui-v2-beta,postgres` (else `serve` is compiled out).
2. **Env opt-out for private-IP egress denial** — tailnet MCP shims (`IRONCLAW_REBORN_EXTENSION_ALLOW_PRIVATE_EGRESS=1`). *(2026-07-16: merged with upstream's new `has_egress_targets` gate + `is_web_access_exa_mcp` rename.)*
3. **Env allowlist for non-gsuite google-account requesters** — broker REST shim (`google-rest`).
4. **Log skill-activation outcome** — activated vs passthrough, per turn.
5. **Prompt caching over OpenAI-compatible gateways** — Anthropic `cache_control` via OpenRouter; downgrades to None where unsupported. (cost saver)
6. **Per-call LLM cost telemetry span** (`target: ironclaw::cost`) — feeds the `:9559` exporter.
7. **`CLAUDE.md`** — agentiff agent rules + the `serve` feature-flag deploy note. *(2026-07-16: kept alongside upstream's new "Testing Discipline"; dropped the stale `ironclaw_engine` from the cargo-clean hint — that crate was removed upstream.)*
8. **`.gitignore *.bak*`** — stop manual-backup cruft.
9. **Fork maintenance tooling** — `sync-upstream.sh` + this file.
10. **Dep secfix** — `quinn-proto` 0.11.16 (RUSTSEC-2026-0185). *Re-applied after the rebase; the `crossbeam-epoch` half was dropped — upstream now ships 0.9.20 (RUSTSEC-2026-0204).*

## Deferred patches (re-home tracker)

Two patches were dropped at the 2026-07-16 rebase because upstream restructured
under them. Reasons + what to do:

- **OTel OTLP tracing for `reborn serve`** — DEFERRED, still wanted (upstream ships
  **no** OTLP). Dropped because the old patch instrumented the now-deleted
  `crates/ironclaw_engine` (`refactor: remove engine v2`, #5545). Re-home spec:
  - (a) **survives — re-apply to same files:** the tracer-provider `OnceLock` +
    `shutdown_otel()` + OTLP layer into `crates/ironclaw_reborn_cli/src/runtime/mod.rs`
    (`init_tracing()` still at line ~50), and the shutdown call in `commands/serve.rs`.
  - (b) **dep reconciliation (the real work):** re-add to `ironclaw_reborn_cli/Cargo.toml`
    — the old patch pinned `opentelemetry 0.27` / `opentelemetry_sdk 0.27` /
    `opentelemetry-otlp 0.27` / `tracing-opentelemetry 0.28`, but the workspace now
    carries **none** of these, so re-pick versions compatible with the current
    `tracing-subscriber`.
  - (c) **re-home the action instrumentation:** the `tracing::info!(action, "action
    dispatch/completed/failed")` lines lived in the removed engine's
    `handle_execute_action`; move them to where `EventKind::ActionExecuted /
    ActionFailed` are emitted now (`crates/ironclaw_runner/src/turn_run_executor.rs`).

- **Distinct `budget_approval_required` failure category** — RETIRED, do **not**
  re-apply: upstream absorbed it. `ironclaw_turns/src/run_profile/host.rs` maps
  `BudgetApprovalRequired => "budget_approval_required"`, with
  `budget_approval_blocked_exit` in `ironclaw_agent_loop/src/executor/model.rs`
  and dedicated tests + a budget e2e suite. (Confirm the failure-summary surface
  matches our intent, then delete this note.)


## Rules
- One patch = one `local:` commit. No uncommitted edits, no `*.bak` files.
- Never `git reset --hard` without checking `git stash list` / a backup branch.
- After any change, `git status` must be clean before deploying.
