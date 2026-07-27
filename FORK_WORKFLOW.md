# Fork workflow & conventions

How we work on this fork without disturbing the live binary. Companion to
`LOCAL_PATCHES.md` (what we carry and why) and `scripts/sync-upstream.sh`.

## 1. Isolation: risky work happens in a worktree, never the primary checkout

`/opt/ironclaw/src` is the **primary checkout** — it stays on `local-patches`,
clean, and is the source the *deployed* binary
(`target/release/ironclaw-reborn`) was built from. Treat it as reference, not
scratch.

**All rebase / PR / upstream-triage / experimental-build work happens in a
dedicated git worktree**, so the primary tree and the running binary are never
disturbed:

```bash
git worktree add /tmp/ironclaw-<purpose> -b <branch> <base>
# ...work, rebase, cargo build, cargo test in there...
git worktree remove /tmp/ironclaw-<purpose>
```

**Why this is safe — build isolation.** There is no `CARGO_TARGET_DIR` /
`target-dir` override (verified 2026-07-27), so each worktree gets its **own**
`target/`. A `cargo build` in a worktree produces *that worktree's*
`ironclaw-reborn`; the live binary at `/opt/ironclaw/src/target/release/` is
untouched until you deliberately promote it. This means even the *actual upstream
rebase and rebuild* can be done and canaried in a worktree, then swapped in — the
running service keeps serving from the old binary the whole time.

This supersedes `sync-upstream.sh`'s in-place rebase for anything on a live host:
rebase in a worktree, not the primary tree.

## 2. Building the live binary — only on explicit say-so, with an ordered backup

The deployed binary serves pilot / drew-claw / the singleton. Swapping it under a
running service is the one genuinely dangerous act.

- **Never run the runtime build** (`build-all.sh`, `cargo build --release ...
  --bin ironclaw-reborn`) against the primary tree, or promote a worktree build,
  **without Nick's explicit instruction in the moment.**
- **Before any promote/swap, keep an ordered binary backup** — timestamped so you
  can roll back to the exact prior one. Existing examples live in
  `/opt/ironclaw/backups/`: `ironclaw-reborn.prev-20260705`,
  `ironclaw-reborn.prev-cache-20260706`.
- **Never `cp` over `target/release/ironclaw-reborn`** — it is a hardlink into
  cargo's cache; overwriting corrupts the cached artifact (see `src/CLAUDE.md`).
  Build in a worktree and move/deploy the produced file, or
  `cargo clean -p ironclaw_reborn_composition -p ironclaw_reborn_cli --release`
  first.
- The correct build features (never forget, or `serve` compiles out):
  `cargo build --release -p ironclaw_reborn_cli --features webui-v2-beta,postgres --bin ironclaw-reborn`.

## 3. Commit convention

Two audiences, two conventions:

**Fork-local patches** (on `local-patches`, never intended for upstream) —
`local:` prefix:

```
local: <imperative summary>

<why it exists on the fork; what upstream file it touches (reapply risk);
the LOCAL_PATCHES.md item number if it has one>
```

- One logical change per commit; keep it re-appliable across an upstream rebase.
- If it touches an upstream file, say so — that is the reapply-after-rebase signal.
- Record it in `LOCAL_PATCHES.md` (the series list + the item number).

**Upstream-PR topic branches** (branched from `origin/main`) — Conventional
Commits, because these become real PRs read by upstream maintainers:

```
fix(scope): …      feat(scope): …      chore(scope): …      perf(scope): …
```

- Branch from `origin/main`, **not** from `local-patches` — keeps the PR diff
  minimal (see §4).
- Strip anything deployment-specific (Agentiff config, env opt-outs, branding).
  A PR is the *general* fix, not our local shape of it.
- Do **not** push. Branches are prepared locally for review; Nick pushes.

Every commit ends with the co-author trailer:
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`

## 4. Peeling PRs to narrow the fork

Goal: move upstream-worthy patches *out* of the fork and into upstream, so
`local-patches` shrinks to only the things that are genuinely fork-specific.

Per `superpowers:upstream-rebase-pr`, triage every `local:` patch:

| class | examples in this fork | action |
|---|---|---|
| **PR-worthy** | security bumps (RUSTSEC), general tool improvements, observability that isn't Agentiff-specific | topic branch from `origin/main`, Conventional Commits, prepare PR, **don't push** |
| **local-only** | `CLAUDE.md` agent rules, budget scripts, env opt-outs (private-IP egress, google-account allowlist), build-flag tweaks | stays on `local-patches` forever; document in `LOCAL_PATCHES.md` |
| **superseded** | upstream already did the equivalent | verify, then drop on next rebase |

Workflow: create the topic branch in a worktree, reconstruct the change cleanly
(cherry-pick + strip local bits, or re-author minimal), `cargo check`/`test` in
the worktree, write the PR body. Nick reviews and pushes.

## 5. After any upstream rebase — verify local patches survived

Upstream can silently overwrite a patched region without a textual conflict. After
every rebase, before trusting a build, walk `LOCAL_PATCHES.md` §"Env-gated
patches — verify ACTIVE" and the series list, and confirm each upstream-touching
patch is still present. Unit tests do not catch a dropped env-gated patch — it
fails at runtime.
