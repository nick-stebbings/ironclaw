# IronClaw Capture Skill — web crawl + screenshot (spec)

**Status:** spec, not implemented. Two consumers need this:
1. **meta-ads-creative-optimizer** (agentiffai-workflows) — already has a *text-extraction-only*
   contract with IronClaw (`temporal/src/integrations/ironclaw.ts`), currently a fixture stub.
2. **loom-render** (`tools-src/loom-render/`, this repo) — needs a *screenshot* of the lead's
   website to compose into the video. Per `tools-src/loom-render/SPEC.md` §4, this cannot run
   inside the WASI sandbox (no browser in wasm) and was previously scoped as an external capture
   service. This spec replaces that plan: route through IronClaw instead, for one reason —

**Why one shared capability, not two:** `ironclaw.ts`'s existing docstring states the architecture
decision already made for meta-ads: *"Ironclaw handles ALL external HTTP fetches. The Temporal
worker never fetches customer websites; production Temporal is firewalled."* A screenshot fetch is
the same category of operation (reach an arbitrary customer URL) as a text-extraction fetch. Two
separate capture paths (one for Temporal-driven bundles, one for loom-render) would duplicate the
egress-policy boundary and the eventual real implementation (headless browser + fetch). One
capability, two transport bindings — see §3.

---

## 1. What's confirmed to NOT exist yet (checked, not assumed)

- No browser/screenshot/crawl capability anywhere in this repo. Searched for
  playwright/puppeteer/chromium/headless-chrome and any first-party `tools-src/*` crawl/screenshot
  tool — none. The only "screenshot" hits are generic chat-attachment handling (Slack/WeCom),
  unrelated. `tools-src/web-search` exists but is search-results, not page fetch/render.
- `ironclaw.ts`'s `fetchWebsiteContent` is a **fixture stub** today — hardcoded placeholder text,
  no real HTTP call. Its own comment: *"Current implementation is a fixture stub... When Ironclaw's
  MCP wiring lands, only the internals change."* That wiring hasn't landed.
- **RESOLVED (was a blocking unknown): `tool-invoke` is deny-all in the live runtime.** Confirmed
  on the deployed fork (`/opt/ironclaw`), not just this checkout: the only `WasmHostTools` impl is
  `DenyWasmHostTools`; `store.rs`'s `tool_invoke` binding routes to it and returns
  `Err("WASM tool invocation is not configured")`; and **nothing** wires a real provider via
  `WitToolHost::with_tools(...)` anywhere (the `.with_tools` hits in `ironclaw_llm/reasoning.rs`
  are the unrelated LLM tool-definitions builder). `http-request`, by contrast, **is** wired
  (`RuntimeHttpEgress` via `.with_http` in `runtime_adapters.rs`) with declarative per-host bearer
  injection (`http.credentials` in `capabilities.json`). **loom-render was the only tool in the
  tree that called `tool-invoke`**, and its Drive I/O has since been **re-pointed to `http-request`**
  against the Drive REST API (verified in-sandbox). ⇒ Do **not** build capture on `tool-invoke`
  unless someone first implements the dispatcher; use `http-request` (see §3b, now the recommended
  binding).

---

## 2. Proposed contract

Extend the existing `IronclawFetchRequest`/`IronclawFetchResponse` shape
(`temporal/src/integrations/ironclaw.ts`) with a second action, rather than inventing a parallel
interface:

```ts
export interface IronclawFetchRequest {
  action: "extract_text" | "screenshot";
  url: string;
  // screenshot-only:
  viewport?: { width: number; height: number };   // default e.g. 1920x1080
  fullPage?: boolean;                              // default false (viewport only)
  format?: "png" | "jpeg";                         // default png
}

export interface IronclawFetchResponse {
  url: string;
  // extract_text:
  text?: string;
  links?: string[];
  // screenshot:
  imageBase64?: string;
  mimeType?: string;       // "image/png" | "image/jpeg"
  width?: number;
  height?: number;
}
```

Caps, mirroring the existing `MAX_PAGE_TEXT_CHARS`/`MAX_PAGE_LINKS` pattern:
`MAX_SCREENSHOT_BYTES` (suggest 5 MiB pre-base64) and a fixed viewport ceiling (e.g. 1920×1920) —
reject oversized/absurd requests before capture, not after. `validateIronclawResponse` gets a
matching branch for the screenshot shape, same tainted-data discipline as today (strings only where
strings are expected, http/https URL validation, no eval'd content).

## 3. Two transport bindings, one capability

The capability is the same; how each consumer reaches it differs because they run in different
places.

### 3a. Temporal (meta-ads, and any future Temporal-side bundle)
Unchanged shape from today, once the real (non-fixture) implementation lands: `fetchWebsiteContent`
in `agentiffai-workflows/temporal/src/integrations/ironclaw.ts` makes the real call (HTTP/MCP,
whatever IronClaw exposes for external callers) instead of returning `fixtureResponse`. Add
`action: "screenshot"` support there when needed — meta-ads doesn't need it today (text-only), but
keeping one contract means it's a non-event if a future ad-generation stage wants a visual
reference of the site.

### 3b. loom-render (WASM tool running *inside* an IronClaw instance) — **use `http-request`**
It's sandboxed (`wit/tool.wit`'s `host` interface only exposes `log`, `now-millis`,
`workspace-read`, `http-request`, `tool-invoke`, `secret-exists`). With §1 resolved (`tool-invoke`
deny-all, `http-request` wired), the binding is decided:

**loom-render calls `http-request` (POST) to an IronClaw-internal capture endpoint** — the same
transport it already uses for the screenshot call and (now) for Drive I/O. The endpoint's host goes
in `loom-render-tool.capabilities.json`'s `http.allowlist` (replacing the placeholder
`loom-capture.internal`), and if the endpoint needs auth, a `http.credentials` bearer entry (same
mechanism as the Google token). **Zero runtime changes.**

> The rejected alternative — `tool-invoke("ironclaw-capture", …)` — would require first building a
> real `WasmHostTools` dispatcher (§1). It does **not** change the trust boundary (the host runs the
> browser + SSRF filter either way), so it buys nothing here while costing net-new runtime work and
> a broader tool→tool security surface. Only revisit if the dispatcher is wanted for other reasons.

**`format`: loom-render MUST request `jpeg`.** The bundled `ffmpeg-wasi` has **no PNG decoder**
(`ff_png_decoder: 0`; JPEG/`mjpeg` and MP3 decoders *are* present) — a PNG screenshot fails
`decode_image`. Verified.

## 4. The real implementation: host-embedded capture (net-new build, not wiring)

Something has to actually drive a browser — a large native binary that **cannot** run in the wasm
sandbox. It runs **native, host-side**, embedded in the IronClaw host process (e.g. `chromiumoxide`
/ CDP from Rust) rather than a separate microservice, to keep infra unified.

**The trust boundary that matters (why this is not "just wiring"):** the WASM sandbox protects the
*host from the tool*. It does **nothing** to protect the *host from the internet* once the host is
the one driving the browser at an attacker-controlled URL. A malicious site exploiting Chromium, or
an SSRF'd internal URL, compromises the **host** — completely bypassing the sandbox the agent sits
in. So the security work lives entirely host-side:

- **Keep Chromium's native OS sandbox on.** Never launch with `--no-sandbox`; the renderer must stay
  isolated by namespaces/seccomp. Operational caveat: the deploy host must actually *support* that
  (user namespaces enabled) or there's pressure to disable it — verify on the Hetzner host, and
  consider running the browser under its own systemd hardening / OS jail regardless.
- **Ephemeral incognito context per request.** Never share a context between requests (no cross-site
  cache/cookie/data leakage).
- **SSRF validation at the host boundary, *before* the browser is launched:**
  1. **Protocol allowlist** — `http`/`https` only; reject `file://`, `data:`, `ftp://`, etc.
  2. **Resolve-then-check** — resolve the host to IP(s) and reject private/local ranges
     (`127.0.0.0/8`, `10/8`, `172.16/12`, `192.168/16`, `169.254/16` incl. cloud metadata
     `169.254.169.254`) **and IPv6** (`::1`, `fc00::/7`, `fe80::/10`, IPv6 metadata). Guard
     **DNS-rebinding / redirects**: pin the validated IP, and re-validate on **every redirect hop**
     (a public URL can 302 → internal).
  3. **Mandatory host-enforced timeout** (e.g. 10 s) and **viewport ceiling** (e.g. ≤1920×1920).
  4. **`MAX_SCREENSHOT_BYTES`** enforced during buffering, before base64 + return across the WASM
     boundary.

This is real engineering (browser pool + CDP + the SSRF filter), not a quick follow-up. Reuse
`extract_text`'s existing host-side fetch/validation infra where possible — a screenshot is the same
category of "reach an arbitrary customer URL" operation, so the SSRF/timeout/limit machinery is
shared, only the render+encode step is new.

## 4a. Deployed security model — `/screenshot` is UNAUTHENTICATED (Option A)

The shipped `capture-shim` gates `/mcp` (the agent surface) behind `CAPTURE_API_KEY`, but leaves
`/screenshot` **unauthenticated**. This was a deliberate trade — a WASM tool (`loom-render`) has no
secure keystore and the runtime's `tool-invoke`/`secret_handle` path had no writer, so we replaced
application-layer auth (a Bearer key) with **network-layer auth (Tailscale) + input validation
(SSRF filtering) + an OS egress firewall**. It is a common, reasonable pattern for internal
microservices — but not a silver bullet. Document the trade honestly.

### Why it's acceptable
1. **Network-layer trust.** `/screenshot` is bound to the tailnet IP only; the public internet
   cannot route to it. WireGuard encrypts the wire and provides device identity. Auth shifts from a
   token to tailnet membership.
2. **SSRF filter is a hard boundary.** The worst case for an unauthenticated screenshotter is SSRF
   (screenshot `169.254.169.254`, `127.0.0.1`, internal dashboards). `capture_core`'s SSRF filter
   still runs on this route, so an unauth caller can only reach **public** URLs. Verified: metadata
   IP → 400.
3. **Second, independent layer — systemd `IPAddressDeny`.** The unit denies egress to
   `127.0.0.0/8 ::1 10/8 172.16/12 192.168/16 169.254/16 fc00::/7 fe80::/10`. Even if the SSRF
   filter is *bypassed*, the OS blocks Chromium's actual TCP connect to loopback/private ranges.
   This is why the concern "security relies 100% on the SSRF filter" is overstated here — there are
   two independent layers (app filter + kernel egress deny).
4. **Solves the WASM secret problem** cleanly — no credential bootstrapping for a sandboxed caller.

### Residual risks — be aware
- **DoS / resource exhaustion.** Headless Chromium is CPU/RAM-heavy. Unauthenticated + tailnet-wide
  means *any* tailnet node can spam `/screenshot` and exhaust the host. **Mitigation (TODO):**
  per-caller rate limit + a hard concurrency cap on browser contexts. Not yet implemented.
- **Tailnet-wide, not "co-located".** The code comment says "co-located callers", but the tailnet
  bind makes it reachable by **anything on the tailnet** (user laptops, other services) unless
  restricted by **Tailscale ACLs**. Since `loom-render` runs on *this same host*, an ACL can and
  should restrict `/screenshot` (the `:443` serve path and `:8939` backend) to the host itself —
  making it effectively host-local despite the tailnet bind. **ACLs live in the tailnet admin
  console (not on the host); verify/author them there.**
- **SSRF filter brittleness.** The app-layer filter must still hold against DNS-rebinding, IPv6
  obfuscation, and 302→internal redirects (see §4's SSRF contract). The `IPAddressDeny` backstop
  covers the *connection*, but the filter is what returns clean errors and blocks by hostname.

### Operational caveats (each cost a failed deploy attempt — read before touching this)
1. **Loopback is *deliberately* denied.** `IPAddressDeny=127.0.0.0/8` means `127.0.0.1:8939` fails
   even though the shim binds there-adjacent — that deny is exactly what stops an SSRF'd Chromium tab
   from reaching `notion-rest` and the other loopback shims. **Test via the tailnet address, never
   loopback.** (It's also why the shim binds the tailnet IP, Option B, not loopback.)
2. **Scheme depends on the port.** `:8938` is **HTTPS** (via `tailscale serve`); `:8939` is **plain
   HTTP** (the backend). Get it wrong and you get `400 "Client sent an HTTP request to an HTTPS
   server"` — which *looks* like a payload error and isn't.
3. **`loom-render` reaches this as `https://loom-capture.internal/screenshot`** (hardcoded
   `SCREENSHOT_HOST`, per its capabilities allowlist). **This name must resolve to the capture
   endpoint** or stage 2 fails with a bare connection error, not anything descriptive.
   ⚠️ **Currently BROKEN: `loom-capture.internal` does not resolve** (not in `/etc/hosts`, no DNS).
   Fix before stage-2 e2e — map `loom-capture.internal` → the tailnet host, and reconcile the
   scheme/cert: loom-render uses `https://…` (:443), which is the TLS serve, but the serve cert is
   for the `*.ts.net` name, not `loom-capture.internal` → cert mismatch. Options: (a) an `/etc/hosts`
   alias + accept that the runtime egress must tolerate the cert name, (b) point `loom-capture.internal`
   at the `:443` serve with a matching cert/SAN, or (c) change loom-render's `SCREENSHOT_HOST` to the
   real tailnet hostname. **Decide + wire this before declaring stage 2 done.**

### Verdict
Reasonably secure for internal infrastructure — network-layer auth + SSRF filter + kernel egress
deny is defense-in-depth, not a single point of failure. To fully lock down, add **Tailscale ACLs**
restricting the capture port to the `loom-render` host, and **rate-limit** `/screenshot` against the
DoS vector.

## 5. Open questions (in priority order)

1. ~~Does `WasmHostTools`/`tool-invoke` have a real production implementation?~~ **RESOLVED (§1):**
   no — deny-all live. Transport is `http-request` (§3b); loom-render's Drive calls were migrated
   off `tool-invoke` accordingly.
2. Who owns/builds the native capture engine (§4) — is there prior art elsewhere in the
   organization (a service already doing this for another product) before building one from scratch?
3. Does meta-ads actually want `screenshot` now, or is `extract_text`'s current stub→real wiring
   the only near-term ask for that consumer? (Affects sequencing, not the contract shape.)
4. Rate limits / concurrency caps on the capture engine — loom-render's bulk-loop can request many
   screenshots per run; needs a sane per-tenant ceiling before it's exposed.
