//! Zoho Mail WASM Tool for IronClaw.
//!
//! Provides Zoho Mail integration for reading, searching, and sending emails.
//!
//! # Capabilities Required
//!
//! - HTTP: `mail.zoho.com.au/api/*` (GET, POST)
//! - Secrets: `zoho_oauth_token` (OAuth 2.0 token, injected automatically)
//!
//! # Supported Actions
//!
//! - `get_accounts`: Get account info (needed for account ID)
//! - `search_messages`: Search emails with query string
//! - `get_message`: Get a specific email by ID
//! - `list_folders`: List mail folders
//!
//! # Example Usage
//!
//! ```json
//! {"action": "search_messages", "query": "newsletter AI", "limit": 20}
//! ```

wit_bindgen::generate!({
    world: "sandboxed-tool",
    path: "../../wit/tool.wit",
});

struct ZohoMailTool;

impl exports::near::agent::tool::Guest for ZohoMailTool {
    fn execute(req: exports::near::agent::tool::Request) -> exports::near::agent::tool::Response {
        match execute_inner(&req.params) {
            Ok(result) => exports::near::agent::tool::Response {
                output: Some(result),
                error: None,
            },
            Err(e) => exports::near::agent::tool::Response {
                output: None,
                error: Some(e),
            },
        }
    }

    fn schema() -> String {
        r#"{
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["get_accounts", "search_messages", "get_message", "list_folders", "list_tags", "create_tag", "tag_message", "untag_message"],
                    "description": "The Zoho Mail operation to perform"
                },
                "account_id": {
                    "type": "string",
                    "description": "Zoho account ID (get from get_accounts). Required for all actions except get_accounts"
                },
                "query": {
                    "type": "string",
                    "description": "Search query string. Used by: search_messages"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results to return (default: 20). Used by: search_messages",
                    "default": 20
                },
                "message_id": {
                    "type": "string",
                    "description": "Message ID. Required for: get_message, tag_message, untag_message"
                },
                "tag_id": {
                    "type": "string",
                    "description": "Tag/label ID. Required for: tag_message, untag_message"
                },
                "tag_name": {
                    "type": "string",
                    "description": "Name for new tag. Required for: create_tag"
                },
                "tag_color": {
                    "type": "string",
                    "description": "Color hex for new tag (e.g. '#FF0000'). Optional for: create_tag"
                }
            }
        }"#
        .to_string()
    }

    fn description() -> String {
        "Zoho Mail integration for reading, searching, and tagging emails. \
         First call get_accounts to obtain the account_id, then use it for other actions. \
         search_messages supports keyword queries. Tag operations: list_tags, create_tag, \
         tag_message, untag_message. Requires Zoho OAuth token."
            .to_string()
    }
}

export!(ZohoMailTool);

fn execute_inner(params_json: &str) -> Result<String, String> {
    let params: serde_json::Value =
        serde_json::from_str(params_json).map_err(|e| format!("Invalid JSON: {}", e))?;

    let action = params["action"]
        .as_str()
        .ok_or("Missing 'action' field")?;

    match action {
        "get_accounts" => get_accounts(),
        "search_messages" => {
            let account_id = params["account_id"]
                .as_str()
                .ok_or("Missing 'account_id'")?;
            let query = params["query"].as_str().unwrap_or("");
            let limit = params["limit"].as_u64().unwrap_or(20) as u32;
            search_messages(account_id, query, limit)
        }
        "get_message" => {
            let account_id = params["account_id"]
                .as_str()
                .ok_or("Missing 'account_id'")?;
            let message_id = params["message_id"]
                .as_str()
                .ok_or("Missing 'message_id'")?;
            get_message(account_id, message_id)
        }
        "list_folders" => {
            let account_id = params["account_id"]
                .as_str()
                .ok_or("Missing 'account_id'")?;
            list_folders(account_id)
        }
        "list_tags" => {
            let account_id = params["account_id"]
                .as_str()
                .ok_or("Missing 'account_id'")?;
            list_tags(account_id)
        }
        "create_tag" => {
            let account_id = params["account_id"]
                .as_str()
                .ok_or("Missing 'account_id'")?;
            let tag_name = params["tag_name"]
                .as_str()
                .ok_or("Missing 'tag_name'")?;
            let tag_color = params["tag_color"].as_str();
            create_tag(account_id, tag_name, tag_color)
        }
        "tag_message" => {
            let account_id = params["account_id"]
                .as_str()
                .ok_or("Missing 'account_id'")?;
            let message_id = params["message_id"]
                .as_str()
                .ok_or("Missing 'message_id'")?;
            let tag_id = params["tag_id"]
                .as_str()
                .ok_or("Missing 'tag_id'")?;
            tag_message(account_id, message_id, tag_id)
        }
        "untag_message" => {
            let account_id = params["account_id"]
                .as_str()
                .ok_or("Missing 'account_id'")?;
            let message_id = params["message_id"]
                .as_str()
                .ok_or("Missing 'message_id'")?;
            let tag_id = params["tag_id"]
                .as_str()
                .ok_or("Missing 'tag_id'")?;
            untag_message(account_id, message_id, tag_id)
        }
        _ => Err(format!(
            "Unknown action '{}'. Use: get_accounts, search_messages, get_message, list_folders, list_tags, create_tag, tag_message, untag_message",
            action
        )),
    }
}

const ZOHO_API_BASE: &str = "https://mail.zoho.com.au/api";

fn api_call(method: &str, path: &str, body: Option<&str>) -> Result<String, String> {
    let url = format!("{}/{}", ZOHO_API_BASE, path);

    // Headers are empty — the host injects the Zoho-oauthtoken credential
    // automatically based on the capabilities.json host_patterns match.
    let headers = if body.is_some() {
        r#"{"Content-Type": "application/json"}"#
    } else {
        "{}"
    };

    let body_bytes = body.map(|b| b.as_bytes().to_vec());

    near::agent::host::log(
        near::agent::host::LogLevel::Debug,
        &format!("Zoho Mail API: {} {}", method, path),
    );

    let response =
        near::agent::host::http_request(method, &url, headers, body_bytes.as_deref(), None)?;

    if response.status < 200 || response.status >= 300 {
        let body_text = String::from_utf8_lossy(&response.body);
        return Err(format!(
            "Zoho API returned status {}: {}",
            response.status, body_text
        ));
    }

    if response.body.is_empty() {
        return Ok("{}".to_string());
    }

    String::from_utf8(response.body).map_err(|e| format!("Invalid UTF-8 in response: {}", e))
}

fn get_accounts() -> Result<String, String> {
    api_call("GET", "accounts", None)
}

fn search_messages(account_id: &str, query: &str, limit: u32) -> Result<String, String> {
    let encoded_query = url_encode(query);
    let path = format!(
        "accounts/{}/messages/search?searchKey={}&limit={}",
        url_encode(account_id),
        encoded_query,
        limit
    );
    let response = api_call("GET", &path, None)?;

    // Parse and extract useful fields
    let parsed: serde_json::Value =
        serde_json::from_str(&response).map_err(|e| format!("Parse error: {}", e))?;

    // Zoho returns { "status": { "code": 200 }, "data": [...] }
    let messages = parsed["data"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let summaries: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| {
            serde_json::json!({
                "messageId": m["messageId"],
                "subject": m["subject"],
                "sender": m["sender"],
                "from": m["fromAddress"],
                "receivedTime": m["receivedTime"],
                "summary": m["summary"],
                "folderId": m["folderId"],
            })
        })
        .collect();

    serde_json::to_string_pretty(&serde_json::json!({
        "count": summaries.len(),
        "messages": summaries
    }))
    .map_err(|e| e.to_string())
}

fn get_message(account_id: &str, message_id: &str) -> Result<String, String> {
    let path = format!(
        "accounts/{}/messages/{}",
        url_encode(account_id),
        url_encode(message_id)
    );
    api_call("GET", &path, None)
}

fn list_folders(account_id: &str) -> Result<String, String> {
    let path = format!("accounts/{}/folders", url_encode(account_id));
    api_call("GET", &path, None)
}

fn list_tags(account_id: &str) -> Result<String, String> {
    let path = format!("accounts/{}/tags", url_encode(account_id));
    api_call("GET", &path, None)
}

fn create_tag(account_id: &str, name: &str, color: Option<&str>) -> Result<String, String> {
    let mut body = serde_json::json!({ "tagName": name });
    if let Some(c) = color {
        body["color"] = serde_json::Value::String(c.to_string());
    }
    let body_str = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let path = format!("accounts/{}/tags", url_encode(account_id));
    api_call("POST", &path, Some(&body_str))
}

fn tag_message(account_id: &str, message_id: &str, tag_id: &str) -> Result<String, String> {
    let path = format!(
        "accounts/{}/messages/{}",
        url_encode(account_id),
        url_encode(message_id)
    );
    let body = serde_json::json!({ "mode": "addTag", "tagid": tag_id });
    let body_str = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    api_call("PUT", &path, Some(&body_str))
}

fn untag_message(account_id: &str, message_id: &str, tag_id: &str) -> Result<String, String> {
    let path = format!(
        "accounts/{}/messages/{}",
        url_encode(account_id),
        url_encode(message_id)
    );
    let body = serde_json::json!({ "mode": "removeTag", "tagid": tag_id });
    let body_str = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    api_call("PUT", &path, Some(&body_str))
}

fn url_encode(s: &str) -> String {
    let mut encoded = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            _ => {
                encoded.push('%');
                encoded.push(char::from(HEX[(b >> 4) as usize]));
                encoded.push(char::from(HEX[(b & 0x0F) as usize]));
            }
        }
    }
    encoded
}

const HEX: [u8; 16] = *b"0123456789ABCDEF";
