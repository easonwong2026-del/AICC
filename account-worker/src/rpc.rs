//! JSON-RPC 2.0 消息类型与错误码。
//!
//! 只用最小子集：请求里必须有 `method`，`id` 原样回显（字符串或数字都行，
//! 所以用 `Value` 承载）。进程只走 stdin/stdout，不监听端口（PRD §9.3）。

use serde::Deserialize;
use serde_json::{json, Value};

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

/// 业务错误码：错误分类要能直接进任务日志，不能只有一句话。
pub const E_STORAGE: i64 = 1001;
pub const E_SECURESTORE: i64 = 1002;
pub const E_NOT_FOUND: i64 = 1003;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    #[allow(dead_code)]
    pub jsonrpc: Option<String>,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

pub type RpcResult<T> = Result<T, RpcError>;

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: sanitize(&message.into()),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(METHOD_NOT_FOUND, format!("未知方法：{method}"))
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(E_NOT_FOUND, message)
    }

    pub fn storage(err: impl std::fmt::Display) -> Self {
        Self::new(E_STORAGE, format!("数据库错误：{err}"))
    }

    pub fn internal(err: impl std::fmt::Display) -> Self {
        Self::new(INTERNAL_ERROR, format!("内部错误：{err}"))
    }

    pub fn to_value(&self) -> Value {
        let mut obj = json!({ "code": self.code, "message": self.message });
        if let Some(data) = &self.data {
            obj["data"] = data.clone();
        }
        obj
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl From<rusqlite::Error> for RpcError {
    fn from(err: rusqlite::Error) -> Self {
        Self::storage(err)
    }
}

impl From<std::io::Error> for RpcError {
    fn from(err: std::io::Error) -> Self {
        Self::new(E_STORAGE, format!("IO 错误：{err}"))
    }
}

/// 敏感串过滤：凭据、令牌这类长串绝不能进日志或错误消息。
///
/// 刻意**不把 `/` 当令牌字符**：路径是日志里最有诊断价值的东西
/// （`/Users/x/Library/Application Support/...`），如果 `/` 参与成串，
/// 整条路径会被打码，等于把最有用的信息删掉。
/// 令牌（JWT / hex / base64url）用 `. - _ ~ + =`，不依赖 `/`；
/// 标准 base64 里的 `/` 会把串切断，但断开后的每段仍然会被独立判定。
pub fn sanitize(text: &str) -> String {
    const MIN_RUN: usize = 32;
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();

    let is_token_char =
        |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '+' | '-' | '=');

    let flush = |run: &mut String, out: &mut String| {
        if run.chars().count() >= MIN_RUN {
            out.push_str("<redacted>");
        } else {
            out.push_str(run);
        }
        run.clear();
    };

    for c in text.chars() {
        if is_token_char(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

pub fn response_ok(id: Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": result })
}

pub fn response_err(id: Option<Value>, err: &RpcError) -> Value {
    json!({ "jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "error": err.to_value() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_redacts_long_token() {
        let msg = "请求失败 token=abcDEF1234567890abcDEF1234567890 已放弃";
        let cleaned = sanitize(msg);
        assert!(cleaned.contains("<redacted>"));
        assert!(!cleaned.contains("abcDEF1234567890"));
        assert!(cleaned.contains("请求失败"));
    }

    #[test]
    fn sanitize_keeps_short_words() {
        assert_eq!(
            sanitize("数据库错误：no such table"),
            "数据库错误：no such table"
        );
    }

    #[test]
    fn sanitize_keeps_paths() {
        // 路径是最重要的诊断信息，必须原样保留。
        let p = "/Users/x/Library/Application Support/AICC-Dashboard/accounts/accounts.sqlite";
        assert_eq!(sanitize(p), p);
    }

    #[test]
    fn sanitize_redacts_jwt_like_token() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let cleaned = sanitize(&format!("token={jwt} 失败"));
        assert!(cleaned.contains("<redacted>"), "{cleaned}");
        assert!(!cleaned.contains("eyJhbGciOiJIUzI1NiJ9"), "{cleaned}");
    }
}
