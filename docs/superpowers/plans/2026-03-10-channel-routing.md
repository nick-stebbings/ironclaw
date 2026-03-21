# Per-Channel MCP Tool Routing Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Filter which tools (MCP and built-in) are visible to the LLM based on which Slack channel triggered the message.

**Architecture:** A new `channel_routing` module loads `~/.ironclaw/channel-routing.json` at startup into a `ChannelRoutingConfig`. The dispatcher calls `config.filter_tool_defs()` after fetching tool definitions, before passing them to the LLM. MCP tools are matched by server-name prefix (`Notion_*` → server "Notion"). Built-in tools use an explicit allowlist per group (absent = all allowed). DMs and unmapped channels fall through to `default_group`.

**Tech Stack:** Rust, serde_json, existing ToolDefinition/ToolRegistry types

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `src/agent/channel_routing.rs` | **CREATE** | Config struct, JSON deserialization, `filter_tool_defs()` logic |
| `src/agent/mod.rs` | MODIFY (1 line) | Add `pub mod channel_routing;` |
| `src/agent/agent_loop.rs` | MODIFY (~5 lines) | Load config at startup, store in `AgentDeps` |
| `src/agent/dispatcher.rs` | MODIFY (~10 lines) | Apply filter at both `tool_definitions()` call sites |
| `src/app.rs` | MODIFY (~5 lines) | Load config from base dir, pass to `AgentDeps` |

---

## Chunk 1: Core Module + Tests

### Task 1: Create channel_routing.rs with config types and deserialization

**Files:**
- Create: `src/agent/channel_routing.rs`
- Modify: `src/agent/mod.rs`

- [ ] **Step 1: Write failing tests for config loading**

In `src/agent/channel_routing.rs`, write tests that exercise JSON deserialization and group resolution:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> ChannelRoutingConfig {
        let json = r#"{
            "groups": {
                "minimal": ["Archon"],
                "dev": ["Archon", "Kiro", "Notion"]
            },
            "builtin_whitelist": {
                "minimal": ["memory_search", "create_job"]
            },
            "channels": {
                "agentiffai-dev-issues": "dev"
            },
            "default_group": "minimal"
        }"#;
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn test_deserialize_config() {
        let config = sample_config();
        assert_eq!(config.groups.len(), 2);
        assert_eq!(config.default_group, "minimal");
        assert_eq!(config.channels["agentiffai-dev-issues"], "dev");
    }

    #[test]
    fn test_resolve_group_mapped_channel() {
        let config = sample_config();
        assert_eq!(config.resolve_group("agentiffai-dev-issues"), "dev");
    }

    #[test]
    fn test_resolve_group_unmapped_falls_to_default() {
        let config = sample_config();
        assert_eq!(config.resolve_group("random-channel"), "minimal");
    }

    #[test]
    fn test_resolve_group_dm_returns_none() {
        let config = sample_config();
        // DMs should bypass routing entirely
        assert!(config.is_dm("slack-dm"));
        assert!(config.is_dm("telegram-dm"));
        assert!(!config.is_dm("agentiffai-dev-issues"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib channel_routing -- -v 2>&1 | tail -5`
Expected: compilation error — module and types don't exist yet

- [ ] **Step 3: Write the config struct and deserialization**

```rust
//! Per-channel tool routing.
//!
//! Loads `~/.ironclaw/channel-routing.json` and filters which tools
//! (MCP and built-in) the LLM can see based on the originating channel.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::llm::ToolDefinition;

/// Channel-to-tool-group routing configuration.
///
/// Loaded from `channel-routing.json` in the IronClaw base directory.
/// When present, each incoming message's channel name is mapped to a group,
/// and only tools belonging to that group's allowed MCP servers (plus any
/// whitelisted built-in tools) are shown to the LLM.
#[derive(Debug, Clone, Deserialize)]
pub struct ChannelRoutingConfig {
    /// MCP server allowlist per group. Key = group name, value = server names.
    /// A tool named `ServerName_tool` belongs to server `ServerName`.
    pub groups: HashMap<String, Vec<String>>,

    /// Built-in tool allowlist per group. If a group is absent from this map,
    /// all built-in tools are available. If present, only listed tools are kept.
    #[serde(default)]
    pub builtin_whitelist: HashMap<String, Vec<String>>,

    /// Channel name → group name mapping.
    pub channels: HashMap<String, String>,

    /// Fallback group for channels not listed in `channels`.
    pub default_group: String,
}

/// Prefixes that identify direct messages (bypass routing entirely).
const DM_PREFIXES: &[&str] = &["slack-dm", "telegram-dm", "cli", "repl", "web"];

impl ChannelRoutingConfig {
    /// Load from `<base_dir>/channel-routing.json`. Returns `None` if the file
    /// doesn't exist or can't be parsed (logged as warning).
    pub fn load(base_dir: &Path) -> Option<Self> {
        let path = base_dir.join("channel-routing.json");
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                tracing::warn!("Failed to read {}: {}", path.display(), e);
                return None;
            }
        };
        match serde_json::from_str(&content) {
            Ok(config) => {
                tracing::info!(
                    groups = ?config.groups.keys().collect::<Vec<_>>(),
                    channels = config.channels.len(),
                    "Loaded channel routing config"
                );
                Some(config)
            }
            Err(e) => {
                tracing::warn!("Failed to parse {}: {}", path.display(), e);
                None
            }
        }
    }

    /// Resolve which group a channel belongs to.
    pub fn resolve_group(&self, channel: &str) -> &str {
        self.channels
            .get(channel)
            .map(|s| s.as_str())
            .unwrap_or(&self.default_group)
    }

    /// Whether this channel name represents a direct message (no filtering).
    pub fn is_dm(channel: &str) -> bool {
        DM_PREFIXES.iter().any(|p| channel.starts_with(p))
    }
}
```

- [ ] **Step 4: Add module declaration**

In `src/agent/mod.rs`, add after the existing module declarations:

```rust
pub mod channel_routing;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib channel_routing -- -v 2>&1 | tail -10`
Expected: 4 tests pass

- [ ] **Step 6: Commit**

```bash
git add src/agent/channel_routing.rs src/agent/mod.rs
git commit -m "feat(routing): add ChannelRoutingConfig struct and deserialization"
```

---

### Task 2: Implement filter_tool_defs

**Files:**
- Modify: `src/agent/channel_routing.rs`

- [ ] **Step 1: Write failing tests for tool filtering**

Add to the `tests` module in `channel_routing.rs`:

```rust
    fn make_tool_def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            parameters: serde_json::json!({}),
        }
    }

    #[test]
    fn test_filter_keeps_allowed_mcp_tools() {
        let config = sample_config();
        let tools = vec![
            make_tool_def("Archon_list_tasks"),
            make_tool_def("Kiro_run_task"),
            make_tool_def("Notion_post_search"),
            make_tool_def("Smartlead_send"),
        ];
        let filtered = config.filter_tool_defs("agentiffai-dev-issues", tools);
        let names: Vec<&str> = filtered.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"Archon_list_tasks"));
        assert!(names.contains(&"Kiro_run_task"));
        assert!(names.contains(&"Notion_post_search"));
        assert!(!names.contains(&"Smartlead_send"));
    }

    #[test]
    fn test_filter_restricts_builtins_when_whitelisted() {
        let config = sample_config();
        // "minimal" group has builtin_whitelist: ["memory_search", "create_job"]
        let tools = vec![
            make_tool_def("Archon_list_tasks"),
            make_tool_def("memory_search"),
            make_tool_def("create_job"),
            make_tool_def("shell"),
            make_tool_def("http_request"),
        ];
        let filtered = config.filter_tool_defs("unmapped-channel", tools);
        let names: Vec<&str> = filtered.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"Archon_list_tasks"));
        assert!(names.contains(&"memory_search"));
        assert!(names.contains(&"create_job"));
        assert!(!names.contains(&"shell"));
        assert!(!names.contains(&"http_request"));
    }

    #[test]
    fn test_filter_allows_all_builtins_when_no_whitelist() {
        let config = sample_config();
        // "dev" group has no builtin_whitelist entry → all builtins allowed
        let tools = vec![
            make_tool_def("Archon_list_tasks"),
            make_tool_def("shell"),
            make_tool_def("memory_search"),
        ];
        let filtered = config.filter_tool_defs("agentiffai-dev-issues", tools);
        assert_eq!(filtered.len(), 3);
    }

    #[test]
    fn test_filter_dm_returns_all_tools() {
        let config = sample_config();
        let tools = vec![
            make_tool_def("Archon_list_tasks"),
            make_tool_def("Smartlead_send"),
            make_tool_def("shell"),
        ];
        let filtered = config.filter_tool_defs("slack-dm", tools.clone());
        assert_eq!(filtered.len(), 3);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib channel_routing -- -v 2>&1 | tail -5`
Expected: compilation error — `filter_tool_defs` doesn't exist

- [ ] **Step 3: Implement filter_tool_defs**

Add to `impl ChannelRoutingConfig`:

```rust
    /// Filter tool definitions based on channel routing rules.
    ///
    /// MCP tools are identified by having an underscore-separated server prefix
    /// (e.g. `Notion_post_search` → server `Notion`). Tools without a known
    /// server prefix are treated as built-in tools.
    pub fn filter_tool_defs(
        &self,
        channel: &str,
        tools: Vec<ToolDefinition>,
    ) -> Vec<ToolDefinition> {
        // DMs bypass all filtering
        if Self::is_dm(channel) {
            return tools;
        }

        let group = self.resolve_group(channel);

        let allowed_servers = match self.groups.get(group) {
            Some(servers) => servers,
            None => {
                tracing::warn!(group, "Channel routing group not found, allowing all tools");
                return tools;
            }
        };

        let builtin_whitelist = self.builtin_whitelist.get(group);

        tools
            .into_iter()
            .filter(|tool| {
                if let Some(server) = self.extract_mcp_server(&tool.name, allowed_servers) {
                    // MCP tool — check if its server is in the allowed list
                    allowed_servers.iter().any(|s| s == server)
                } else {
                    // Built-in tool — check whitelist if one exists for this group
                    match builtin_whitelist {
                        Some(whitelist) => whitelist.iter().any(|w| w == &tool.name),
                        None => true,
                    }
                }
            })
            .collect()
    }

    /// Try to extract the MCP server name from a tool name.
    ///
    /// MCP tools are named `ServerName_tool_name`. We check if the prefix
    /// before the first `_` matches any known server name across all groups.
    fn extract_mcp_server<'a>(
        &self,
        tool_name: &'a str,
        _hint_servers: &[String],
    ) -> Option<&'a str> {
        // Check all known server names across all groups
        let all_servers = self.groups.values().flatten();
        for server in all_servers {
            let prefix = format!("{}_", server);
            if tool_name.starts_with(&prefix) {
                return Some(&tool_name[..server.len()]);
            }
        }
        None
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib channel_routing -- -v 2>&1 | tail -15`
Expected: 8 tests pass

- [ ] **Step 5: Run clippy**

Run: `cargo clippy --all --all-features 2>&1 | tail -3`
Expected: zero warnings

- [ ] **Step 6: Commit**

```bash
git add src/agent/channel_routing.rs
git commit -m "feat(routing): implement filter_tool_defs for per-channel tool filtering"
```

---

## Chunk 2: Wire Into Agent

### Task 3: Add config to AgentDeps and load at startup

**Files:**
- Modify: `src/agent/agent_loop.rs` (~3 lines)
- Modify: `src/app.rs` (~5 lines)

- [ ] **Step 1: Add field to AgentDeps**

In `src/agent/agent_loop.rs`, add to the `AgentDeps` struct after the `transcription` field:

```rust
    /// Per-channel tool routing config (loaded from channel-routing.json).
    pub channel_routing: Option<Arc<crate::agent::channel_routing::ChannelRoutingConfig>>,
```

Add `use std::sync::Arc;` if not already imported (it is — verify).

- [ ] **Step 2: Fix all AgentDeps construction sites**

Search for every place `AgentDeps` is constructed:

Run: `grep -rn 'AgentDeps {' src/` to find all sites.

Add `channel_routing: None,` to any test/non-app construction. The main one in `app.rs` will be handled in Step 3.

- [ ] **Step 3: Load config in app.rs**

In `src/app.rs`, find where `AgentDeps` is constructed (search for `AgentDeps {`). Before that block, add:

```rust
        let channel_routing = crate::agent::channel_routing::ChannelRoutingConfig::load(
            &crate::bootstrap::ironclaw_base_dir(),
        )
        .map(Arc::new);
```

Then add to the `AgentDeps` struct literal:

```rust
            channel_routing,
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check 2>&1 | tail -5`
Expected: clean (no errors)

- [ ] **Step 5: Commit**

```bash
git add src/agent/agent_loop.rs src/app.rs
git commit -m "feat(routing): load channel routing config into AgentDeps"
```

---

### Task 4: Apply filter in dispatcher.rs

**Files:**
- Modify: `src/agent/dispatcher.rs` (~15 lines)

- [ ] **Step 1: Add helper method on Agent**

In `src/agent/dispatcher.rs`, inside the `impl Agent` block, add a private helper before `run_agentic_loop`:

```rust
    /// Apply per-channel tool filtering if routing config is loaded.
    fn apply_channel_routing(
        &self,
        channel: &str,
        tools: Vec<crate::llm::ToolDefinition>,
    ) -> Vec<crate::llm::ToolDefinition> {
        if let Some(ref routing) = self.deps.channel_routing {
            let before = tools.len();
            let filtered = routing.filter_tool_defs(channel, tools);
            if filtered.len() < before {
                tracing::info!(
                    channel,
                    group = routing.resolve_group(channel),
                    before,
                    after = filtered.len(),
                    "Channel routing filtered tools"
                );
            }
            filtered
        } else {
            tools
        }
    }
```

- [ ] **Step 2: Apply at first call site (line ~147)**

Change:
```rust
        let initial_tool_defs = self.tools().tool_definitions().await;
```
To:
```rust
        let initial_tool_defs = self.tools().tool_definitions().await;
        let initial_tool_defs = self.apply_channel_routing(&message.channel, initial_tool_defs);
```

This must come BEFORE skill attenuation (which is the next line).

- [ ] **Step 3: Apply at second call site (line ~214)**

Change:
```rust
            let tool_defs = self.tools().tool_definitions().await;
```
To:
```rust
            let tool_defs = self.tools().tool_definitions().await;
            let tool_defs = self.apply_channel_routing(&message.channel, tool_defs);
```

Again, before skill attenuation.

- [ ] **Step 4: Verify compilation and clippy**

Run: `cargo clippy --all --all-features 2>&1 | tail -5`
Expected: zero warnings

- [ ] **Step 5: Run full test suite**

Run: `cargo test --lib 2>&1 | tail -5`
Expected: all tests pass (channel routing is None in tests, so no filtering applied)

- [ ] **Step 6: Commit**

```bash
git add src/agent/dispatcher.rs
git commit -m "feat(routing): apply per-channel tool filtering in dispatcher"
```

---

### Task 5: Add integration test

**Files:**
- Modify: `src/agent/channel_routing.rs`

- [ ] **Step 1: Write integration-style test covering the full flow**

Add to tests in `channel_routing.rs`:

```rust
    #[test]
    fn test_load_returns_none_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let config = ChannelRoutingConfig::load(dir.path());
        assert!(config.is_none());
    }

    #[test]
    fn test_load_parses_valid_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channel-routing.json");
        std::fs::write(
            &path,
            r#"{
                "groups": {"minimal": ["Archon"]},
                "channels": {},
                "default_group": "minimal"
            }"#,
        )
        .unwrap();
        let config = ChannelRoutingConfig::load(dir.path());
        assert!(config.is_some());
        assert_eq!(config.unwrap().default_group, "minimal");
    }

    #[test]
    fn test_load_returns_none_for_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channel-routing.json");
        std::fs::write(&path, "not json").unwrap();
        let config = ChannelRoutingConfig::load(dir.path());
        assert!(config.is_none());
    }

    #[test]
    fn test_full_routing_scenario() {
        // Simulate the real config shape
        let json = r#"{
            "groups": {
                "minimal": ["Archon"],
                "content": ["Archon", "Notion", "Kit"],
                "dev": ["Archon", "Kiro", "Notion"]
            },
            "builtin_whitelist": {
                "content": ["memory_search", "memory_write", "create_job"]
            },
            "channels": {
                "agentiffai-marketing": "content",
                "agentiffai-dev-issues": "dev"
            },
            "default_group": "minimal"
        }"#;
        let config: ChannelRoutingConfig = serde_json::from_str(json).unwrap();

        // Content channel: only Archon+Notion+Kit MCP tools + whitelisted builtins
        let all_tools = vec![
            make_tool_def("Archon_list_tasks"),
            make_tool_def("Notion_post_search"),
            make_tool_def("Kit_list_subscribers"),
            make_tool_def("Kiro_run_task"),
            make_tool_def("memory_search"),
            make_tool_def("shell"),
            make_tool_def("create_job"),
        ];

        let content_tools = config.filter_tool_defs("agentiffai-marketing", all_tools.clone());
        let content_names: Vec<&str> = content_tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(content_names, vec![
            "Archon_list_tasks",
            "Notion_post_search",
            "Kit_list_subscribers",
            "memory_search",
            "create_job",
        ]);

        // Dev channel: Archon+Kiro+Notion MCP tools, all builtins (no whitelist)
        let dev_tools = config.filter_tool_defs("agentiffai-dev-issues", all_tools.clone());
        let dev_names: Vec<&str> = dev_tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(dev_names, vec![
            "Archon_list_tasks",
            "Notion_post_search",
            "Kiro_run_task",
            "memory_search",
            "shell",
            "create_job",
        ]);

        // DM: everything
        let dm_tools = config.filter_tool_defs("slack-dm", all_tools.clone());
        assert_eq!(dm_tools.len(), 7);
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib channel_routing -- -v 2>&1 | tail -15`
Expected: all tests pass

- [ ] **Step 3: Run fmt and clippy**

Run: `cargo fmt && cargo clippy --all --all-features 2>&1 | tail -5`
Expected: clean

- [ ] **Step 4: Commit**

```bash
git add src/agent/channel_routing.rs
git commit -m "test(routing): add integration tests for channel routing config"
```

---

## Execution Notes

- The `metadata_key` field in the existing JSON config is unused — don't deserialize it (the field is from an earlier design iteration; `channel` on `IncomingMessage` is the resolved name)
- `tempfile` is already a dev-dependency in Cargo.toml
- The `Arc` wrapper on `ChannelRoutingConfig` is needed because `AgentDeps` fields are shared across async tasks
- If `channel-routing.json` is absent, the feature is fully inactive (no filtering)
- The `extract_mcp_server` method scans all known server names across all groups to identify MCP tools. This is O(servers × tool_name_len) per tool, which is negligible for <50 servers
