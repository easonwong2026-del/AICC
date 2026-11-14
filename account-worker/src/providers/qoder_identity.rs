//! 从解出来的会话 JSON 里取"能可靠证明身份"的字段。
//!
//! 两个版本的会话结构**不完全一样**，这里的策略是：
//!
//! 1. 先看明文字段里有没有现成的 uid / email；
//! 2. 再看 token 是不是 JWT，能解出 payload 就取 `sub` / `email` / `preferred_username`；
//! 3. 都拿不到就返回 `None`，**绝不根据目录名、邮箱猜测或拼一个假 UID**（PRD §4.4）。
//!
//! 注意：这里只读，不校验签名。签名的可信方是服务端，AICC 只把 JWT 当作
//! "客户端自己写下来的身份声明"，用于展示与去重，不作为鉴权依据。

use base64::Engine;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub uid: Option<String>,
    pub email: Option<String>,
    /// 身份来源：`field` / `jwt` / 空（没识别出来）。
    pub source: Option<&'static str>,
}

impl Identity {
    pub fn is_empty(&self) -> bool {
        self.uid.is_none() && self.email.is_none()
    }
}

const UID_KEYS: &[&str] = &["uid", "userId", "user_id", "userID", "id"];
const EMAIL_KEYS: &[&str] = &["email", "mail", "userEmail", "user_email"];

pub fn extract(session: &Value) -> Identity {
    let mut identity = Identity::default();

    if let Some(obj) = session.as_object() {
        for key in UID_KEYS {
            if let Some(value) = obj.get(*key).and_then(scalar_string) {
                identity.uid = Some(value);
                identity.source = Some("field");
                break;
            }
        }
        for key in EMAIL_KEYS {
            if let Some(value) = obj.get(*key).and_then(scalar_string) {
                identity.email = Some(value);
                identity.source.get_or_insert("field");
                break;
            }
        }
        // 常见嵌套：{"user": {"id": "...", "email": "..."}}
        if identity.is_empty() {
            for nested in ["user", "account", "profile", "currentUser"] {
                let Some(value) = obj.get(nested) else {
                    continue;
                };
                if !value.is_object() {
                    continue;
                }
                if let Some(uid) = UID_KEYS
                    .iter()
                    .find_map(|k| value.get(*k).and_then(scalar_string))
                {
                    identity.uid = Some(uid);
                    identity.source = Some("field");
                }
                if let Some(email) = EMAIL_KEYS
                    .iter()
                    .find_map(|k| value.get(*k).and_then(scalar_string))
                {
                    identity.email = Some(email);
                    identity.source.get_or_insert("field");
                }
                if !identity.is_empty() {
                    break;
                }
            }
        }
    }

    if identity.uid.is_none() {
        if let Some(token) = session.get("token").and_then(Value::as_str) {
            if let Some(claims) = jwt_claims(token) {
                if let Some(uid) = ["sub", "uid", "user_id", "userId", "id"]
                    .iter()
                    .find_map(|k| claims.get(*k).and_then(scalar_string))
                {
                    identity.uid = Some(uid);
                    identity.source = Some("jwt");
                }
                if identity.email.is_none() {
                    if let Some(email) = ["email", "preferred_username", "mail"]
                        .iter()
                        .find_map(|k| claims.get(*k).and_then(scalar_string))
                    {
                        identity.email = Some(email);
                        identity.source.get_or_insert("jwt");
                    }
                }
            }
        }
    }

    identity
}

fn scalar_string(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > 320 {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 只解 payload，不验签。不是 JWT 就返回 None。
pub fn jwt_claims(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    if parts.next().is_none() {
        return None;
    }
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(payload))
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn jwt(payload: Value) -> String {
        let enc = |v: &Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string().as_bytes())
        };
        format!(
            "{}.{}.{}",
            enc(&json!({"alg":"none"})),
            enc(&payload),
            "sig"
        )
    }

    #[test]
    fn reads_flat_fields() {
        let id = extract(&json!({"uid":"u-1","email":"a@b.c","token":"x"}));
        assert_eq!(id.uid.as_deref(), Some("u-1"));
        assert_eq!(id.email.as_deref(), Some("a@b.c"));
        assert_eq!(id.source, Some("field"));
    }

    #[test]
    fn reads_nested_user_object() {
        let id = extract(&json!({"user":{"id":"u-2","email":"z@y.c"}}));
        assert_eq!(id.uid.as_deref(), Some("u-2"));
        assert_eq!(id.email.as_deref(), Some("z@y.c"));
    }

    #[test]
    fn falls_back_to_jwt_claims() {
        let token = jwt(json!({"sub":"user-9","email":"j@w.t"}));
        let id = extract(&json!({"token": token}));
        assert_eq!(id.uid.as_deref(), Some("user-9"));
        assert_eq!(id.email.as_deref(), Some("j@w.t"));
        assert_eq!(id.source, Some("jwt"));
    }

    #[test]
    fn opaque_token_yields_no_identity() {
        let id = extract(&json!({"token":"not-a-jwt"}));
        assert!(id.is_empty());
        assert!(id.source.is_none());
    }

    #[test]
    fn never_invents_identity_from_nothing() {
        let id = extract(&json!({"nickname":"someone"}));
        assert!(id.is_empty());
    }
}
