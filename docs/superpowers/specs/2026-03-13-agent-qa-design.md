# Agent QA — Weekly Skill & Workflow Audit

**Date**: 2026-03-13
**Status**: Approved (v3)

## Purpose

Weekly audit of all IronClaw skills, workflows, MCP servers, and WASM tools. Reports what's active, what's dead, what's failing, and what can be improved. Delivered as a Slack DM every Sunday evening.

## Scope

- Read-only audit — no automated fixes, no Notion writes
- Single Slack DM report with structured sections
- Runs as a scheduled routine on the default model (Kimi 2.5)
- Two-phase execution: worker gathers data, main agent compiles report

## Data Sources

### 1. Skill Inventory

- Use `skill_list` built-in tool (returns all installed skills with metadata)
- Cross-reference against job history to classify:
  - **Active**: appeared in a job description in the last 14 days
  - **Dormant**: 14-30 days since last appearance
  - **Defunct**: 30+ days or never appeared
- "Triggered" heuristic: a skill is considered triggered when a job's `description` field contains the skill name (case-insensitive substring match). This is imperfect (manual jobs without skill names won't match) but avoids parsing conversation logs.

### 2. Job Health

- Worker fetches job data from Archon API (`GET /api/ironclaw/jobs?limit=200`)
- Main agent filters to last 14 days by `created_at`
- Group by description pattern (first 50 chars, normalized)
- Calculate per-group: success rate, avg `actual_cost`, avg `actual_time_secs`
- Extract error patterns from failed jobs (group similar `failure_reason` messages)
- Flag groups with >30% failure rate

### 3. Routine Health

- Worker fetches routines (`GET /api/ironclaw/routines`) and runs (`GET /api/ironclaw/routine-runs?limit=200`)
- Main agent filters runs to last 14 days by `started_at`, maps `routine_id` to routine `name`
- Check: which routines fired on schedule, which missed, which errored
- Flag any routine with 0 successful runs in the period or `consecutive_failures > 0`

### 4. MCP Server Health

- Worker runs health checks via shell (`curl --connect-timeout 3`):
  - Archon (8051), SeqThink (8061), Serpstat (8062), Notion (8063),
    Smartlead (8064), Perplexity (8065), HubSpot (8066), Apollo (8067),
    Kit (8068), Kiro (8069), Figma (8070)
- Report up/down status for each
- Note: main agent cannot do this directly — `http_request` tool blocks localhost (SSRF protection). Worker has shell access and `network_mode: host`, so `curl` to localhost works.

### 5. WASM Tool & Channel Status

- Worker lists `~/.ironclaw/tools/*.wasm` and `~/.ironclaw/channels/*.wasm` via shell (`ls -la`)
- Verify each file exists and size > 0
- Expected WASM files (update this list as tools are added/removed):
  - **Channels**: `slack.wasm`
  - **Tools**: (currently none — update when WASM tools are installed)
- Cross-reference with channel routing config for completeness

## Output Format

Slack DM with these sections:

```
📊 Weekly Agent QA — week of {date}

🟢 Active Skills ({count})
  {skill_name} ({job_count} jobs, {success_rate}% success, ${avg_cost} avg)
  ...

🟡 Dormant ({count}) — no triggers in 14 days
  {skill_name}, ...

🔴 Defunct ({count}) — never triggered or 30+ days
  {skill_name}, ...

📋 Routines
  ✅ {routine_name}: {success}/{total} runs OK
  ❌ {routine_name}: {description of issue}

🔌 MCP Servers ({total})
  ✅ {healthy}/{total} healthy
  ❌ {server_name} ({port}): {error}

🧩 WASM Tools: {count}/{expected} present | Channels: {count}/{expected}

💡 Recommendations
  • {specific actionable recommendation based on data}
  • {e.g. "cold-email-sequences: 3 failures — all Smartlead API 429. Add rate limiting."}
  • {e.g. "16 skills never triggered — consider archiving or merging."}
```

## Execution

| Property | Value |
|----------|-------|
| Skill name | `agent-qa` |
| Model | default (Kimi 2.5) |
| Schedule | Sunday 8pm NZDT (`0 7 * * 0` UTC) |
| Delivery | Slack DM (same as heartbeat) |
| Activation keywords | "agent qa", "skill audit", "workflow audit", "weekly qa" |
| Channel routing | minimal group (DM) |
| max_context_tokens | 3000 |

## Implementation Approach

Two-phase execution using the **worker delegation pattern**:

### Phase 1: Data Gathering (Worker Job)

The main agent spawns a worker job via `create_job` with natural language instructions. The worker runs an LLM loop (Kimi 2.5) that interprets the instructions and calls `shell` to gather data. The worker uses `network_mode: host` (default), so it can reach localhost services.

The worker's instructions tell it to:

1. **Fetch job history** from Archon API (no `psql` needed — workers don't have it):
   ```bash
   curl -s "http://127.0.0.1:8051/api/ironclaw/jobs?limit=200"
   ```
   Response includes: `description`, `status`, `actual_cost`, `actual_time_secs`, `failure_reason`, `created_at` per job.

2. **Fetch routine data** from Archon API:
   ```bash
   curl -s "http://127.0.0.1:8051/api/ironclaw/routines"
   curl -s "http://127.0.0.1:8051/api/ironclaw/routine-runs?limit=200"
   ```
   Routines response includes: `name`, `enabled`, `run_count`, `consecutive_failures`, `last_run_at`.
   Runs response includes: `routine_id`, `status`, `result_summary`, `started_at`, `completed_at`.

3. **Health-check MCP servers** via curl:
   ```bash
   # Archon (FastAPI, not supergateway) — use metrics endpoint
   curl -s --connect-timeout 3 -o /dev/null -w "8051:%{http_code}\n" http://127.0.0.1:8051/api/ironclaw/metrics
   # Supergateway-proxied MCP servers — use /mcp endpoint
   for port in 8061 8062 8063 8064 8065 8066 8067 8068 8069 8070; do
     curl -s --connect-timeout 3 -o /dev/null -w "$port:%{http_code}\n" http://127.0.0.1:$port/mcp
   done
   ```
   Note: Archon (8051) is a FastAPI app — check `/api/ironclaw/metrics` not `/mcp`. Supergateway-proxied servers (8061-8070) expose `/mcp` (Streamable HTTP transport). If `/mcp` returns non-200, fall back to checking `/` for native HTTP servers (Kiro, Figma).

4. **List WASM files**:
   ```bash
   ls -la ~/.ironclaw/tools/*.wasm ~/.ironclaw/channels/*.wasm 2>&1
   ```

5. Output all results as structured text for the main agent to parse.

The worker job description should be natural language, e.g.:
> "Gather data for the weekly agent QA audit. Use curl to: (1) fetch jobs from http://127.0.0.1:8051/api/ironclaw/jobs?limit=200, (2) fetch routines from http://127.0.0.1:8051/api/ironclaw/routines, (3) fetch routine runs from http://127.0.0.1:8051/api/ironclaw/routine-runs?limit=200, (4) health-check MCP servers on ports 8051,8061-8070 via curl to /mcp endpoint, (5) list WASM files in ~/.ironclaw/tools/ and ~/.ironclaw/channels/. Output all results."

Worker cost: a few cents for Kimi 2.5 LLM reasoning + shell calls (no heavy computation).

### Phase 2: Report Compilation (Main Agent)

After the worker job completes, the main agent:

1. Reads the worker's output (job results returned via `create_job`)
2. Calls `skill_list` to get current skill inventory
3. Filters job data to last 14 days (Archon API returns all non-archived jobs; filter by `created_at`)
4. Cross-references skill names against job `description` fields to classify active/dormant/defunct
5. Maps `routine_id` in runs to routine `name` from the routines list
6. Compiles the Slack DM report using the output format above
7. Generates specific recommendations based on the data
8. Sends via Slack DM (same mechanism as heartbeat/daily report)

### Error Handling

- If the worker job fails or times out (10 min limit), send a degraded report: skill inventory (from `skill_list`) + note that data gathering failed
- If Archon API is unreachable (port 8051 down), report "Archon unavailable" in job/routine sections
- If all MCP health checks fail, flag potential network issue rather than listing each as down
- If the Archon jobs endpoint returns fewer than expected results, note the limit in the report

## Constraints

- No automated remediation — report only
- No Notion writes — Slack DM only
- No per-message skill activation analysis (too expensive to parse conversation logs)
- Historical trending is Grafana's job — this skill reports current-week snapshot only
- If Kimi 2.5 recommendations are too shallow, escalate to `model: premium` later
- Worker job cost is minimal (Kimi 2.5 reasoning loop + shell commands, typically a few cents)

## Success Criteria

- Correctly classifies all installed skills as active/dormant/defunct
- Catches failed routines and MCP outages
- Recommendations are specific and actionable (not generic)
- Report fits in a single Slack message (under 4000 chars)
- Runs reliably every Sunday without manual intervention
- Graceful degradation if worker job or Postgres is unavailable
