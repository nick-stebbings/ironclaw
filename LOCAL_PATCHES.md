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

### Build toolchain (bumped by the 2026-07-16 catch-up)

`build-all.sh` now needs, on top of a plain cargo:
- **rustc >= 1.96** (built on **1.97**) — upstream raised the MSRV. `rustup update stable`.
- **`wasm32-wasip2`** target — `rustup target add wasm32-wasip2` (wasm channels/tools).
- **`wasm-tools`** CLI — `cargo install wasm-tools` (telegram channel `component new`/`strip`).
- **Node >= 22** for the `ironclaw_webui_v2` frontend build (`corepack pnpm@11.7` needs
  `node:sqlite`). A deploy-local Node 24 at `/home/deploy/opt/node-v24.18.0-linux-x64`
  is used just for the build — prepend its `bin/` to `PATH`; system Node stays untouched.
  Node is **build-time only** — the runtime serves the compiled-in assets.

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
11. **OTel OTLP tracing for `reborn serve`** — active when `OTEL_EXPORTER_OTLP_ENDPOINT` is set. *(2026-07-16: re-homed after upstream removed `crates/ironclaw_engine` (#5545). OTLP init/shutdown now in `reborn_cli/src/runtime/mod.rs` + `commands/serve.rs`; `opentelemetry 0.27` / `tracing-opentelemetry 0.28`; verified compiled into the release binary. The old per-action `tracing::info!` lines were intentionally not re-homed — the OTLP layer already captures the new `ironclaw_runner` instrumentation.)*

## Retired / notes

- **Distinct `budget_approval_required` failure category** — RETIRED at the
  2026-07-16 rebase, do **not** re-apply: upstream absorbed it.
  `ironclaw_turns/src/run_profile/host.rs` maps `BudgetApprovalRequired =>
  "budget_approval_required"`, with `budget_approval_blocked_exit` in
  `ironclaw_agent_loop/src/executor/model.rs` and dedicated tests + a budget e2e
  suite. (Confirm the failure-summary surface matches our intent, then delete this note.)

## Rules
- One patch = one `local:` commit. No uncommitted edits, no `*.bak` files.
- Never `git reset --hard` without checking `git stash list` / a backup branch.
- After any change, `git status` must be clean before deploying.
