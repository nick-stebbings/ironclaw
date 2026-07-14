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

1. **OTel OTLP tracing for `reborn serve`** — active when `OTEL_EXPORTER_OTLP_ENDPOINT` set.
2. **`build-all.sh` feature flags** — builds `ironclaw_reborn_cli` with `webui-v2-beta,postgres` (else `serve` is compiled out).
3. **Env opt-out for private-IP egress denial** — required for tailnet MCP shims (`IRONCLAW_REBORN_EXTENSION_ALLOW_PRIVATE_EGRESS=1`).
4. **Env allowlist for non-gsuite google-account requesters** — broker REST shim (`google-rest`).
5. **Log skill-activation outcome** — activated vs passthrough, per turn.
6. **Distinct `budget_approval_required` failure category**.
7. **Prompt caching over OpenAI-compatible gateways** — enables Anthropic `cache_control` via OpenRouter; downgrades to None where unsupported. (cost saver)
8. **Per-call LLM cost telemetry span** (`target: ironclaw::cost`) — feeds the `:9559` exporter.
9. **Dep secfix** — `quinn-proto` 0.11.16 + `crossbeam-epoch` 0.9.20 (RUSTSEC-2026-0185 / -0204). *Candidate to drop once upstream bumps these.*
10. **`CLAUDE.md`** — agentiff-specific agent rules.
11. **`.gitignore *.bak*`** — stop manual-backup cruft.

## Rules
- One patch = one `local:` commit. No uncommitted edits, no `*.bak` files.
- Never `git reset --hard` without checking `git stash list` / a backup branch.
- After any change, `git status` must be clean before deploying.
