//! willdeep 自己的 MCP 端点，挂在 Web 服务上。
//!
//! 与插件网关的差别：插件网关只绑 `127.0.0.1`、只转发到插件；这里跟着 Web
//! 服务走，所以能被另一台机器访问到。代价是必须有 token——谁能连上这个地址，
//! 谁就能读到下面这些工具返回的内容。
//!
//! 第一版只有只读工具。提交一轮、执行命令需要单独的权限设计，不在这里做。
//! 传输用 Streamable HTTP 的 JSON 响应模式：一个 POST 进、一个 JSON 出，
//! 不维持 SSE 长连接。

use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use super::mcp_tokens;

const PROTOCOL_VERSION: &str = "2025-06-18";

/// `POST /mcp`。GET 不提供：没有需要持续推送的内容。
pub(crate) async fn handle(
    State(state): State<Arc<crate::web::WebState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(token) = bearer(&headers) else {
        return unauthorized();
    };
    if !mcp_tokens::authenticate(state.home(), &token) {
        return unauthorized();
    }
    let message: Value = match serde_json::from_slice(&body) {
        Ok(message) => message,
        Err(_) => return json_response(&rpc_error(Value::Null, -32700, "Parse error")),
    };
    let Some(object) = message.as_object() else {
        return json_response(&rpc_error(
            Value::Null,
            -32600,
            "Invalid Request: expected a single JSON-RPC object",
        ));
    };
    let id = object.get("id").cloned();
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        // 客户端对服务端请求的响应：这里不发请求，收下即可。
        if id.is_some() && (object.contains_key("result") || object.contains_key("error")) {
            return StatusCode::ACCEPTED.into_response();
        }
        return json_response(&rpc_error(
            id.unwrap_or(Value::Null),
            -32600,
            "Invalid Request",
        ));
    };
    // 通知没有 id，也没有需要回的内容。
    let Some(id) = id else {
        return StatusCode::ACCEPTED.into_response();
    };
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "initialize" => json_response(&initialize(id)),
        "ping" => json_response(&json!({"jsonrpc": "2.0", "id": id, "result": {}})),
        "tools/list" => {
            json_response(&json!({"jsonrpc": "2.0", "id": id, "result": {"tools": tools()}}))
        }
        "tools/call" => json_response(&call_tool(state.home(), id, &params).await),
        _ => json_response(&rpc_error(id, -32601, "Method not found")),
    }
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim().to_owned())
        .filter(|token| !token.is_empty())
}

fn unauthorized() -> Response {
    let mut response = (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "unauthorized"})),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, "Bearer".parse().unwrap());
    response
}

fn json_response(body: &Value) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn initialize(id: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "willdeep", "version": env!("CARGO_PKG_VERSION")},
        }
    })
}

fn tools() -> Vec<Value> {
    vec![
        tool(
            "daemon_status",
            "willdeep Runtime 守护进程是否在运行，以及它的版本与运行时长。",
            json!({"type": "object", "properties": {}}),
        ),
        tool(
            "list_sessions",
            "列出这个 willdeep 上的会话：状态、工作区、模型、有没有正在跑的轮次。",
            json!({"type": "object", "properties": {}}),
        ),
        tool(
            "list_tasks",
            "列出这个 willdeep 上的任务及其状态。",
            json!({"type": "object", "properties": {}}),
        ),
    ]
}

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({"name": name, "description": description, "inputSchema": schema})
}

async fn call_tool(home: &Path, id: Value, params: &Value) -> Value {
    let name = params["name"].as_str().unwrap_or("");
    let outcome = match name {
        "daemon_status" => daemon_status(home).await,
        "list_sessions" => list_sessions(home).await,
        "list_tasks" => list_tasks(home).await,
        _ => return rpc_error(id, -32602, "Unknown tool"),
    };
    let (text, is_error) = match outcome {
        Ok(text) => (text, false),
        Err(error) => (error.to_string(), true),
    };
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{"type": "text", "text": text}],
            "isError": is_error,
        }
    })
}

async fn daemon_status(home: &Path) -> anyhow::Result<String> {
    super::status(home).await
}

fn api_data<T>(response: willdeep_runtime_protocol::ApiResponse<T>) -> anyhow::Result<T> {
    match response {
        willdeep_runtime_protocol::ApiResponse::Ok { data, .. } => Ok(data),
        willdeep_runtime_protocol::ApiResponse::Error { error, .. } => {
            anyhow::bail!("Runtime API error: {}", error.message)
        }
    }
}

async fn list_sessions(home: &Path) -> anyhow::Result<String> {
    let state = super::require_running(home).await?;
    let sessions = api_data(super::runtime_client(&state)?.sessions().await?)?;
    let rows: Vec<Value> = sessions
        .iter()
        .map(|session| {
            json!({
                "id": session.id,
                "status": session.status,
                "workspace": session.workspace,
                "profile": session.profile,
                "model": session.model,
                "active_turn_id": session.active_turn_id,
                "updated_at": session.updated_at,
            })
        })
        .collect();
    Ok(serde_json::to_string_pretty(&rows)?)
}

async fn list_tasks(home: &Path) -> anyhow::Result<String> {
    let state = super::require_running(home).await?;
    let tasks = api_data(super::runtime_client(&state)?.tasks().await?)?;
    let rows: Vec<Value> = tasks
        .iter()
        .map(|task| {
            json!({
                "id": task.id,
                "session_id": task.session_id,
                "status": task.status,
                "workspace": task.workspace,
                "prompt_excerpt": task.prompt_excerpt,
                "created_at": task.created_at,
                "completed_at": task.completed_at,
            })
        })
        .collect();
    Ok(serde_json::to_string_pretty(&rows)?)
}
