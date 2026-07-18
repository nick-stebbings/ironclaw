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
# ALWAYS after a rebase: run the range-diff + env-gated-patch verification below.
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

## Env-gated patches — verify ACTIVE after every rebase / rebuild / instance-cleanup

**A rebase preserves patch _code_; it does not preserve the per-instance _config_
that makes several patches actually do anything.** Half of this series is inert
unless an env var is (a) set in `/etc/ironclaw/instances/<id>.env` **and** (b)
passed through the launcher's `env -i` allowlist
(`/usr/local/bin/ironclaw-reborn-instance-launcher`) **and** (c) the binary is
rebuilt from the post-rebase source. An instance-env wipe / regeneration can
silently disable a patch whose source is perfectly intact — the symptom then
looks exactly like "the rebase clobbered the source" when it did not.

Run `git range-diff <old-base>..<pre-rebase-backup> origin/main..local-patches`
after every rebase: `=` means a patch survived byte-identical, `!` means its
content drifted and needs a read. Then verify each env-gated patch is *active*:

| Patch | Env var (instance `.env` + launcher allowlist) | Smoke check it's ACTIVE |
|-------|------------------------------------------------|--------------------------|
| #1 feature flags | _(build-time)_ `--features webui-v2-beta,postgres` | `ironclaw-reborn serve --help` exists; instance boots WebChat |
| #2 private-IP egress | `IRONCLAW_REBORN_EXTENSION_ALLOW_PRIVATE_EGRESS=1` | any tailnet shim (notion/google/vane on 100.x) is reachable, not `error_kind="network"` |
| #3 non-gsuite google requesters | `IRONCLAW_REBORN_GOOGLE_ACCOUNT_EXTRA_REQUESTERS=google-rest` | `google-rest` Sheets/Drive call resolves a token — NOT `x-google-token header missing` |
| #6 cost telemetry / #11 OTel | `OTEL_EXPORTER_OTLP_ENDPOINT`, `:9559` exporter | spans arrive at the collector |

Repo source of truth for the instance env is `ops-library
scripts/ironclaw/create-instance.sh` — a fresh instance gets all of these; a
**pre-existing** instance whose `.env` predates a patch (or was wiped) will be
missing it and must be reconciled by hand.

> **2026-07-16 incident.** After this rebase + a pilot account-wipe, `google-rest`
> Sheets/Drive failed with `x-google-token header missing`. Root cause was **not**
> the rebase: patch #3 survived byte-identical (`range-diff` `=`), the binary was
> rebuilt (04:20), and the launcher passed the var. The pilot's `.env` had simply
> **lost `IRONCLAW_REBORN_GOOGLE_ACCOUNT_EXTRA_REQUESTERS`**, so the gsuite
> account-visibility policy hid the broker-pushed `provider=google` account from
> `google-rest` → no token staged. The Agentiff broker was refreshing + pushing
> correctly the whole time (Loki: `Successfully refreshed OAuth token` +
> `Pushed credential` every 30 min). A stale persistent grant had been masking the
> missing env until the restart cleared it. Fix: add the var back to the instance
> `.env`, restart.

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
12. **Propagate W3C `traceparent` to MCP shim calls** (Tier 2) — `ironclaw_mcp` injects the current span trace-context into outbound MCP headers (post-plan, like the session header); `reborn_cli` init_tracing registers the `TraceContextPropagator`. Shim spans join the IronClaw trace in Tempo end-to-end (turn -> tool -> shim). No-op when OTLP is off.
13. **Structured JSON logs for `reborn serve`** — the stderr `fmt` layer now emits newline-delimited JSON (`.json().flatten_event(true).with_current_span(true)`) instead of pretty text, so Loki/Promtail ingest fields (target, span, level, msg, trace_id) as parsed labels rather than one opaque string. `tracing-subscriber` gains the `json` feature. Pretty console output is unchanged for interactive TTY use elsewhere.
14. **Manual-token pushes land on the `Api` auth surface** — the broker manual-token setup/submit/secret-submit handlers (`product_auth/serve/manual_token.rs`) re-stamp `scope.surface = AuthSurface::Api` after building the caller scope. The shared `scope_from_authenticated_caller_parts` hardcodes `Callback` (correct for the OAuth redirect flow), but the product-auth store is surface-partitioned and runtime credential resolution (`credentials/runtime_credentials.rs`) reads `AuthSurface::Api`. Before this, pushed tokens landed under `callback/` and were never injected into tool calls. OAuth start/callback paths are unchanged.

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
