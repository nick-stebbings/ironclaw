# Loom Pipeline — Remaining Work Spec

**Purpose:** the authoritative "what's left to get Loom working end-to-end," grounded in
what was *verified live on `/opt/ironclaw`* (not assumed). Supersedes the optimistic
"what's left" in `tools-src/loom-render/SPEC.md` §9 where they disagree.

Pipeline: **Stage 1** (`personalized-loom-outreach-poc` bundle → ElevenLabs MP3 to Drive)
→ **Stage 2** (`loom-render` wasm tool → website capture + audio → video to Drive).

---

## 0. Verified state (this session)

| Area | State | Evidence |
|---|---|---|
| Table-element cap | Patched 10k→16k (`ff3ac837f`), **built Jul 27 00:03, LIVE on `pilot`** (restarted 00:07). `drew-claw` NOT rebuilt. | binary mtime + `ExecMainStartTimestamp`; boot healthy |
| loom-render tool | Committed WIP (`5fed1ca9`); builds to a valid wasip2 component (17.3 MB); **instantiates under the 16k cap** | in-sandbox `WitToolRuntime` prepare/execute |
| google-drive v0.3.0 | Committed (`40fa49f9`), binary base64 download/upload | git log |
| Encoders | VP9, AV1, Opus, AAC all present → gate picks **WebM/VP9+Opus** | in-sandbox `probe_encoders` |
| **Decoders** | **JPEG + MP3 present; NO PNG decoder** (`ff_png_decoder: 0`) | `nm libavcodec.a` |
| **`tool-invoke`** | 🔴 **DENY-ALL in the live runtime** — only `DenyWasmHostTools` exists; nothing wires `.with_tools(...)` | `host.rs:604`, `runtime_adapters.rs:751-767` |
| `http-request` | ✅ Real (`RuntimeHttpEgress` wired via `.with_http`) | `runtime_adapters.rs:767` |
| Capture service | ✅ Real (not a stub): `capture-shim.service` live (Playwright + headless Chromium), tailnet-only, unauthenticated `/screenshot`. Extension installed on `pilot` **and** `drew-claw`. | `systemctl status capture-shim` |
| Registration | ✅ `loom-render` installed on `pilot` **and** `drew-claw` (extension-catalog `search`/`install`, not `registry/tools` — see §2.5) | `extension search loom` on both instances |

---

## 1. THE critical decision — Drive I/O transport (blocks everything in Stage 2)

`loom-render` fetches audio and uploads video via `tool-invoke("google-drive", …)`. **That
transport is deny-all in the live runtime.** So the shipped tool cannot do Drive I/O in
production. Pick one before any other Stage-2 work matters:

### Option A — implement a real `WasmHostTools` dispatcher (host-side)
Wire a production `WasmHostTools` impl that routes a wasm tool's `tool-invoke(alias, params)`
to another registered tool (google-drive), threading the tenant's OAuth + egress policy, and
inject it via `.with_tools(...)` in `runtime_adapters.rs`.
- **Pro:** unlocks tool→tool composition generally (also the capture skill's tool-invoke path).
- **Con:** net-new runtime engineering + security surface (a tool can now invoke tools). Slower.
- **DONE-when:** an in-sandbox test where a component calls `tool-invoke("google-drive", …)`
  against the *real* dispatcher (not a mock) returns real Drive data.

### Option B — re-point loom-render's Drive I/O at `http-request` (RECOMMENDED)
Rewrite `fetch_audio`/`upload_video` to call the Google Drive REST API over `http-request`
(which *is* wired live), with the Google token injected as a header (host credential
injection / `secret-exists`) and `www.googleapis.com` in the `capabilities.json` egress
allowlist.
- **Pro:** no runtime changes; unblocks e2e now; uses the one transport proven to work live.
- **Con:** loom-render now holds a Google-scoped egress + token (the reason it originally used
  google-drive tool-invoke was to *avoid* that). Acceptable for a POC.
- **DONE-when:** in-sandbox test downloads + uploads against Drive via `http-request` with a
  mocked egress returning canned bodies; capabilities.json egress = Drive + capture host only.

> Recommendation: **B** to reach e2e; revisit **A** when the capture skill needs tool-invoke
> anyway (see `docs/specs/ironclaw-capture-skill.md`, which flags the same blocker).
>
> **✅ DONE — Option B is implemented and verified.** loom-render's `fetch_audio`/`upload_video`
> now use `http-request` against the Drive REST API (`GET /drive/v3/files/{id}?alt=media`,
> `POST /upload/drive/v3/files?uploadType=multipart`); `capabilities.json` declares the
> `www.googleapis.com` egress + `google_oauth_token` bearer injection (copied from the google-drive
> tool); the `tool-invoke` path and the `invoke_tool` helper are deleted. Verified in-sandbox
> (`crates/ironclaw_wasm/tests/loom_render_encode.rs`, no `with_tools`): Drive GET download + Drive
> multipart upload + screenshot fetch all exercised via a URL-routing http mock, component
> instantiates under the 16k table cap. The capture skill (§3) should use the same transport.

---

## 2. Stage 2 — loom-render remaining work (after §1)

1. **Drive transport** — ✅ DONE (Option B, see §1).
2. **Capture format = JPEG, not PNG** — ✅ recorded in `capabilities.json`; `decode_image` already
   tries MJPEG. `ffmpeg-wasi` has no PNG decoder, so the capture endpoint MUST return JPEG.
3. **Audio slice + full encode — ✅ GREEN.** The in-sandbox harness
   (`crates/ironclaw_wasm/tests/loom_render_encode.rs`) drove out four real issues, all fixed and
   **committed to the server** (`local:` patch `e4f0f615`); the *server-built* component was pulled
   back and verified to produce a valid **`matroska,webm · vp9 · opus`** file:
   - ✅ **ID3v2 not stripped** → MP3 `send_packet` failed. `decode_mp3` now skips a leading ID3v2 tag.
   - ✅ **wasi EAGAIN mismatch** → every encode drain died with `-6`. `EAGAIN` is `6` on
     wasm32-wasi (not Linux's `11`); `AVERROR_EAGAIN` was `-11` in `encode.rs` + `audio.rs`. Fixed.
   - ✅ **vorbis experimental-compliance flag** (was opus-only). Fixed.
   - ✅ **The "VP9 flush trap" was FUEL EXHAUSTION, not a codec bug.** With the fuel budget raised
     (~1e14 for a 1 s 720p clip), VP9 encodes + flushes + muxes cleanly. See §6 — this is the real
     deployment gate.
   - **Remaining audio nuance (not a blocker for the core path):** the green path is
     **48 kHz → opus**. Real ElevenLabs audio is likely **44.1 kHz mono**, which routes to the
     native vorbis fallback (needs stereo; no resampler in the build). Fix: have stage-1/ElevenLabs
     emit **48 kHz** (recommended), or add a 44.1→48k resampler, or mono→stereo upmix for vorbis.
4. **Measure the resource envelope.** The encode test above also yields peak memory / fuel /
   wall-time for a real (e.g. 5 s @ 720p) encode. Set `loom-render`'s `WitToolLimits`
   accordingly and **clamp inputs** (duration ≤ N, resolution ≤ 720p if needed). Default
   sandbox memory is 10 MiB — nowhere near enough; the tool must request more.
   - **DONE-when:** a worst-case-input encode completes under committed limits; inputs clamped
     so no request can exceed them.
5. **Register + install.** ✅ DONE on both `pilot` and `drew-claw` — turned out to need
   two manifest-level fixes, not a `registry/tools` entry (that registry is unrelated to the
   `extension search`/`install` discovery path, which scans `$IRONCLAW_REBORN_HOME/local-dev/system/extensions/*/manifest.toml`
   directly):
   - **`trust` must be `third_party`, not `first_party_requested`.** Manifests discovered via
     the filesystem scan are always `ManifestSource::InstalledLocal`, and
     `ManifestSource::allows_first_party()` is `true` only for `HostBundled`
     (`ironclaw_extensions/src/v2.rs:132-135, 765-776`). Requesting `first_party_requested`
     from an `InstalledLocal` source is `ManifestV2Error::TrustForbiddenForSource`, which
     `load_filesystem_packages()` **silently fail-open-swallows** (only a `tracing::warn!`,
     no CLI-visible error — see `available_extensions.rs:1655-1668`, citing #5966) — this is
     why `extension search` returned `count: 0` with zero explanation.
   - **Legacy top-level `[[capabilities]]` is rejected for `InstalledLocal`.** Must declare
     `[[host_api]] id = "ironclaw.capability_provider/v1"` / `section = "capability_provider.tools"`
     and nest capabilities under `[[capability_provider.tools.capabilities]]`, matching how
     `github`'s manifest (a working first-party wasm tool) already does it. Same fail-open
     swallow applies, surfaced only via `tracing::warn!{message="skipping invalid available
     extension manifest", reason="...legacy top-level capabilities..."}`.
   - **Tooling footgun:** the standalone CLI's storage root is
     `$IRONCLAW_REBORN_HOME/<profile-subdir>/...`, not `$IRONCLAW_REBORN_HOME/...` directly —
     for the `LocalDev` profile the subdir is literally `local-dev`
     (`ironclaw_reborn_config/src/profile.rs:84-86`,
     `ironclaw_reborn_cli/src/runtime/mod.rs:978-986`). To point the CLI at a real running
     instance's on-disk state, set `IRONCLAW_REBORN_HOME=/var/lib/ironclaw/instances/<id>/home`
     (one level *above* the `local-dev` dir you can see on disk), e.g.:
     `IRONCLAW_REBORN_HOME=/var/lib/ironclaw/instances/pilot/home ironclaw-reborn extension search loom`.
     Also run from outside the repo tree (e.g. `/tmp`) — `dotenvy::dotenv()` in `main()` loads
     `/opt/ironclaw/src/.env` if invoked from inside it, and `OTEL_EXPORTER_OTLP_ENDPOINT` being
     set there panics with "no reactor running" (OTel init happens outside a Tokio context in
     the CLI's sync `main()`).
   - **DONE-when:** ✅ the agent on `pilot`/`drew-claw` can see `loom-render` via
     `extension search loom`. `extension activate` still returns `blockers: [{credential: google}]`
     when run standalone (a tenant-identity mismatch — the CLI probe's default owner isn't the
     real chat agent's tenant, which already has Google connected via `google-drive`/`gsuite`);
     expected to activate cleanly when the real agent calls `builtin.extension_activate` under
     its own tenant. Not yet confirmed live in a real chat turn.

---

## 3. Capture — ✅ DONE (real engine shipped, not a stub)

Superseded: this was scoped as a throwaway JPEG stub because the browser engine was assumed
to be a large net-new build blocked on the `tool-invoke` gap. It shipped as a real capture
engine instead — `capture-shim.service` (Playwright + headless `chrome-headless-shell`),
reachable tailnet-only at `http://loom-capture.internal:8939` (plain HTTP; the `tailscale
serve` HTTPS front uses a `*.ts.net` cert that egress cert verification rejects, so
`loom-render` talks to it over the internal alias instead — see `/etc/hosts` + git history
`4c2192f1a`). `/screenshot` is intentionally unauthenticated (tailnet-only is the boundary;
`loom_screenshot_api_key` credential was dropped, `958210045`). Extension installed on
`pilot` and `drew-claw` (copied from `pilot`'s working manifest — identical across instances,
no per-instance config).

- **DONE-when:** ✅ loom-render, given a real Drive audio id + any URL, can reach
  `capture.screenshot` and get back a real JPEG. Full lead→video Drive flow not yet run
  end-to-end on a live tenant.

---

## 4. Stage 1 — audio path (independent; near-term win)

1. **Merge `agentiffai-workflows!9`** — ✅ merged to `main`.
2. **Provision the tenant:** Google + Apollo connected; the bundle's 4 n8n helper webhooks
   deployed/synced. Not yet confirmed.
3. **Deploy + register the `loom-pipeline` shim.** 🔴 **Still the real blocker, precisely
   identified this session:** the Python shim source is done and refactored
   (`ops-library/shims/loom-pipeline-shim/`, shares `agentiff_shim_base.py` with
   `social-media-v2-shim`), but `loom-pipeline-shim.service` **does not exist on the box** —
   confirmed via `systemctl list-units --all --type=service | grep shim` (every other shim —
   `notion-rest-shim`, `social-media-v2-shim`, `capture-shim`, `meta-ads-shim`,
   `creative-studio-shim`, `content-review-mcp`, `vane-shim`, `agentiff-warehouse-mcp` — is
   `loaded active running`; `loom-pipeline-shim` is absent entirely, not even
   inactive/failed). `drew-claw` has a `system/extensions/loom-pipeline/manifest.toml`
   file already, but it points at a backend that was never deployed, so it is **not actually
   functional there either** despite `extension search` listing it. Needed:
   `make deploy-shim SHIM=loom-pipeline-shim PORT=<unused-89xx>` (ports 8930-8943 already
   taken — check with `ss -tlnp | grep :89`), then `make install-extension EXT=loom-pipeline
   INSTANCE=pilot` (and `drew-claw`, replacing its stale manifest), then per-tenant secrets.
   - **DONE-when:** "generate loom intro audio" produces an MP3 in Drive end-to-end.

*(Stage 1 has no dependency on the §1 blocker — it can ship while Stage 2 is unblocked.)*

---

## 5. Wiring + finish

1. **`drew-claw`** still runs the pre-patch (Jul 23) binary — restart it deliberately when
   convenient to pick up the 16k table cap **and** the `mcp.rs` `InstalledLocal`
   credential-injection fix (`f01cfc77d`, this session — widens product-auth credential
   injection from `HostBundled`-only to also cover `InstalledLocal` hosted-MCP shims;
   load-bearing for `notion-rest`/`google-rest`/`social-media-v2`/`loom-pipeline`'s tokens,
   all of which are `InstalledLocal`). `loom-render` itself installs/discovers fine on the
   old binary but **will not instantiate** (16k table cap) until restarted.
2. **Flip the skill** `loom-pipeline-render-video` `disabled → active` (ops-library
   `agentiff-tool-manifest.yml`) once loom-render is installed; add its MODULES.md module.
3. **Stage-1 → Stage-2 handoff:** the render skill passes stage-1's `audio_drive_file_id`.
4. **MR !10** — 22 CodeRabbit findings triaged; needs `git push` of `abd6a425` + `0ace8df5`.

---

## 6. Acceptance (end-to-end)

- [ ] Drive I/O works against the live runtime (§1 decision implemented + tested).
- [ ] loom-render encodes a playable WebM (vp9+opus) from JPEG capture + MP3 audio, in-sandbox,
      under committed limits.
- [ ] loom-render registered + installed on `pilot`, visible in `tools/list`.
- [ ] Capture stub (or real engine) returns JPEG at the allowlisted endpoint.
- [ ] Stage 1 shipped: lead → ElevenLabs MP3 in Drive.
- [ ] Full flow on a test tenant: lead → audio → capture → video in Drive.
- [ ] Skill flipped active; stage-1→2 handoff wired.

## 7. Open decisions
1. **Drive I/O: Option A (dispatcher) vs B (http-request).** — recommend B. *(§1)*
2. **Capture engine:** native host-side build; transport (tool-invoke vs internal http). *(§3,
   `ironclaw-capture-skill.md`)*
3. **Resolution/duration ceiling** from the measured envelope. *(§2.4)*
4. **When to restart `drew-claw`** onto the patched binary. *(§5.1)*

## Appendix — scratch/verification artifacts (local `ironclaw` checkout, untracked)
- `crates/ironclaw_wasm/tests/loom_render_probe.rs` — in-sandbox instantiate + `probe_encoders`.
- `crates/ironclaw_wasm/tests/loom_render_encode.rs` — mocked-host full-encode harness (switch
  fixture to JPEG).
- `crates/ironclaw_wasm/tests/loom_fixture_shot.{png,jpg}`, `loom_fixture_audio.mp3` — fixtures.
- `tools-src/loom-render-spike/` — original link+run feasibility spike.
- Local table-cap branch `fix/wasm-tool-table-cap-for-large-tools` (named-const, upstream-style;
  the *live* fix is the one-line `local:` patch `ff3ac837f` on the server).
