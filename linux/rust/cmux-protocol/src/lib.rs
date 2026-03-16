use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

fn default_params() -> Value {
    Value::Object(Map::new())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V2Request {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default = "default_params")]
    pub params: Value,
}

impl V2Request {
    pub fn new(method: impl Into<String>, params: Value) -> Self {
        Self {
            id: None,
            method: method.into(),
            params,
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(Value::String(id.into()));
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V2Response {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<V2Error>,
}

impl V2Response {
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(
        id: Option<Value>,
        code: i64,
        message: impl Into<String>,
        data: Option<Value>,
    ) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(V2Error {
                code,
                message: message.into(),
                data,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V2Error {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct V1CommandEnvelope {
    pub command: String,
    pub params: Value,
}

impl V1CommandEnvelope {
    pub fn into_v2_request(self, id: Option<Value>) -> V2Request {
        V2Request {
            id,
            method: self.command,
            params: self.params,
        }
    }
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("invalid json payload: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("v1 envelope must be a json object")]
    ExpectedObject,
    #[error("v1 envelope is missing command field")]
    MissingCommand,
    #[error("v1 command must be a non-empty string")]
    InvalidCommand,
    #[error("v1 params/args/payload must be a json object")]
    InvalidParams,
}

pub fn parse_v1_command_envelope(input: &str) -> Result<V1CommandEnvelope, ProtocolError> {
    let value: Value = serde_json::from_str(input)?;
    parse_v1_command_envelope_value(value)
}

pub fn parse_v1_command_envelope_value(value: Value) -> Result<V1CommandEnvelope, ProtocolError> {
    let object = value.as_object().ok_or(ProtocolError::ExpectedObject)?;

    let command = object
        .get("command")
        .or_else(|| object.get("cmd"))
        .or_else(|| object.get("method"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(ProtocolError::MissingCommand)?
        .to_owned();

    let params = object
        .get("params")
        .or_else(|| object.get("args"))
        .or_else(|| object.get("payload"))
        .cloned()
        .unwrap_or_else(default_params);

    if !params.is_object() {
        return Err(ProtocolError::InvalidParams);
    }

    Ok(V1CommandEnvelope { command, params })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn v2_request_roundtrip_serialization() {
        let request = V2Request {
            id: Some(Value::String("req-1".to_string())),
            method: "system.ping".to_string(),
            params: json!({"verbose": true}),
        };

        let encoded = serde_json::to_string(&request).expect("request should serialize");
        let decoded: V2Request =
            serde_json::from_str(&encoded).expect("request should deserialize");

        assert_eq!(decoded, request);
    }

    #[test]
    fn v2_response_error_serialization() {
        let response = V2Response::error(
            Some(Value::String("req-9".to_string())),
            -32601,
            "unknown method",
            Some(json!({"method": "bad.call"})),
        );

        let encoded = serde_json::to_value(response).expect("response should serialize");
        assert_eq!(encoded["id"], "req-9");
        assert_eq!(encoded["ok"], false);
        assert_eq!(encoded["error"]["code"], -32601);
        assert_eq!(encoded["error"]["message"], "unknown method");
    }

    #[test]
    fn parse_v1_command_with_args() {
        let parsed =
            parse_v1_command_envelope(r#"{"command":"workspace.create","args":{"name":"dev"}}"#)
                .expect("v1 envelope should parse");

        assert_eq!(parsed.command, "workspace.create");
        assert_eq!(parsed.params, json!({"name": "dev"}));
    }

    #[test]
    fn parse_v1_command_accepts_cmd_payload_aliases() {
        let parsed =
            parse_v1_command_envelope(r#"{"cmd":"window.create","payload":{"title":"shell"}}"#)
                .expect("v1 envelope should parse");

        assert_eq!(parsed.command, "window.create");
        assert_eq!(parsed.params, json!({"title": "shell"}));
    }

    #[test]
    fn parse_v1_command_rejects_non_object_params() {
        let err = parse_v1_command_envelope(r#"{"command":"system.ping","params":"bad"}"#)
            .expect_err("params must be an object");

        assert!(matches!(err, ProtocolError::InvalidParams));
    }
}
