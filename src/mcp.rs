//! MCP over stdio: newline-delimited JSON-RPC 2.0. No network. Logs go to stderr only.
//! Tool failures come back as a result with isError true; JSON-RPC errors are for unknown tools and bad arguments.

use std::io::{self, BufRead, Write};

use anyhow::Result;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::store::Session;

const PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendArgs {
    to: String,
    kind: String,
    body: String,
    #[serde(rename = "ref")]
    ref_: Option<String>,
    idem: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InboxArgs {
    since: Option<i64>,
    limit: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdArgs {
    id: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RedactArgs {
    id: i64,
    reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskUpdateArgs {
    id: String,
    state: String,
    owner: Option<String>,
    label: Option<String>,
    note: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DigestArgs {
    since: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

pub fn serve(session: &mut Session) -> Result<()> {
    let stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    for line in stdin.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(session, &msg),
            Err(_) => Some(error_reply(Value::Null, -32700, "parse error")),
        };
        if let Some(reply) = reply {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

fn error_reply(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Returns None for notifications (no id), which get no reply.
fn handle(session: &mut Session, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned()?;
    let result = match msg.get("method").and_then(Value::as_str) {
        Some("initialize") => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "grokbot-claude-relay", "version": env!("CARGO_PKG_VERSION")},
        })),
        Some("ping") => Ok(json!({})),
        Some("tools/list") => Ok(json!({"tools": tool_defs()})),
        Some("tools/call") => call_tool(session, msg.get("params").unwrap_or(&Value::Null)),
        Some(_) => Err((-32601, "method not found".to_string())),
        None => Err((-32600, "invalid request".to_string())),
    };
    Some(match result {
        Ok(value) => json!({"jsonrpc": "2.0", "id": id, "result": value}),
        Err((code, message)) => error_reply(id, code, &message),
    })
}

type RpcResult = std::result::Result<Value, (i64, String)>;

fn parse_args<T: DeserializeOwned>(args: Value) -> std::result::Result<T, (i64, String)> {
    serde_json::from_value(args).map_err(|e| (-32602, format!("invalid arguments: {e}")))
}

fn call_tool(session: &mut Session, params: &Value) -> RpcResult {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "tools/call needs a tool name".to_string()))?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let outcome = match name {
        "send" => {
            let a: SendArgs = parse_args(args)?;
            session
                .send(
                    &a.to,
                    &a.kind,
                    &a.body,
                    a.ref_.as_deref(),
                    a.idem.as_deref(),
                )
                .map(|id| id.to_string())
        }
        "inbox" => {
            let a: InboxArgs = parse_args(args)?;
            session.inbox(a.since, a.limit.unwrap_or(50), false, true)
        }
        "ack" => {
            let a: IdArgs = parse_args(args)?;
            session.ack(a.id)
        }
        "redact" => {
            let a: RedactArgs = parse_args(args)?;
            session.redact(a.id, &a.reason).map(|id| id.to_string())
        }
        "tasks" => {
            let _: NoArgs = parse_args(args)?;
            session.tasks(true)
        }
        "task_update" => {
            let a: TaskUpdateArgs = parse_args(args)?;
            session.task_update(
                &a.id,
                &a.state,
                a.owner.as_deref(),
                a.label.as_deref(),
                &a.note,
            )
        }
        "health" => {
            let _: NoArgs = parse_args(args)?;
            session.health()
        }
        "digest" => {
            let a: DigestArgs = parse_args(args)?;
            session.digest(&a.since)
        }
        other => return Err((-32602, format!("unknown tool '{other}'"))),
    };
    match outcome {
        Ok(text) => Ok(json!({"content": [{"type": "text", "text": text}], "isError": false})),
        Err(err) => {
            Ok(json!({"content": [{"type": "text", "text": format!("{err:#}")}], "isError": true}))
        }
    }
}

fn tool_defs() -> Value {
    json!([
        {
            "name": "send",
            "description": "Append a message from this participant to claude, grok or owner. Returns the message id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "to": {"type": "string", "enum": ["claude", "grok", "owner"]},
                    "kind": {"type": "string", "enum": ["status", "question", "instruction", "done", "alert"]},
                    "body": {"type": "string", "maxLength": 8192},
                    "ref": {"type": "string", "description": "Task id or label name"},
                    "idem": {"type": "string", "description": "Idempotency key; a repeat returns the original id"}
                },
                "required": ["to", "kind", "body"],
                "additionalProperties": false
            }
        },
        {
            "name": "inbox",
            "description": "Messages addressed to this participant after its read cursor (or after since), oldest first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "since": {"type": "integer", "description": "Message id; show messages after it"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 500}
                },
                "additionalProperties": false
            }
        },
        {
            "name": "ack",
            "description": "Move this participant's read cursor through a message addressed to it. Earlier unread messages are hidden; the reply says how many.",
            "inputSchema": {
                "type": "object",
                "properties": {"id": {"type": "integer"}},
                "required": ["id"],
                "additionalProperties": false
            }
        },
        {
            "name": "redact",
            "description": "Blank the body of one of this participant's own messages. Metadata is kept.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "integer"},
                    "reason": {"type": "string", "maxLength": 8192}
                },
                "required": ["id", "reason"],
                "additionalProperties": false
            }
        },
        {
            "name": "tasks",
            "description": "The shared task board as JSON.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
        },
        {
            "name": "task_update",
            "description": "Create or update a task on the shared board.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "maxLength": 32},
                    "state": {"type": "string", "maxLength": 64},
                    "owner": {"type": "string", "maxLength": 64},
                    "label": {"type": "string", "maxLength": 64},
                    "note": {"type": "string", "maxLength": 8192}
                },
                "required": ["id", "state", "note"],
                "additionalProperties": false
            }
        },
        {
            "name": "health",
            "description": "Per-participant last_seen and unread counts, db size and schema version.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
        },
        {
            "name": "digest",
            "description": "Plain-text summary of relay messages since an RFC3339 time.",
            "inputSchema": {
                "type": "object",
                "properties": {"since": {"type": "string", "description": "RFC3339, e.g. 2026-01-01T00:00:00Z"}},
                "required": ["since"],
                "additionalProperties": false
            }
        }
    ])
}
