// OpenCode REST/SSE client — written against opencode v1.3.0
//
// API endpoints used:
//   GET    /global/health        — readiness check on startup
//   POST   /session              — create session
//   GET    /session              — list sessions
//   POST   /session/:id/message  — send prompt
//   GET    /session/:id/message  — list messages (for /stat)
//   POST   /session/:id/abort    — cancel in-flight request
//   GET    /provider             — list providers and models
//   GET    /event                — SSE event stream
//
// SSE event types consumed:
//   message.part.updated  — streaming reasoning/text/tool updates
//   session.idle          — completion signal
//   session.error         — model/provider errors (e.g. content filter)
//
// Note: opencode puts the event type in the JSON data.type field,
// NOT in the SSE `event:` header.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::process::{Child, Command};

fn find_free_port() -> std::io::Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

pub struct OpencodeServer {
    pub url: String,
    child: Child,
}

impl OpencodeServer {
    /// Spawn `opencode serve` and poll the health endpoint until ready.
    pub async fn spawn(config: &Value) -> Result<Self, String> {
        let port = find_free_port().map_err(|e| format!("Failed to find free port: {}", e))?;
        let url = format!("http://127.0.0.1:{}", port);

        let mut cmd = Command::new("opencode");
        cmd.args(["serve", "--hostname=127.0.0.1", &format!("--port={}", port)])
            .env("OPENCODE_CONFIG_CONTENT", config.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let child = cmd.spawn().map_err(|e| {
            format!("Failed to spawn opencode serve: {}. Is 'opencode' in PATH?", e)
        })?;

        // Poll health endpoint until server is ready
        let client = Client::new();
        let health_url = format!("{}/global/health", url);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err("Timeout waiting for opencode serve to start".to_string());
            }
            if let Ok(resp) = client.get(&health_url).send().await {
                if resp.status().is_success() {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }

        Ok(OpencodeServer { url, child })
    }

    pub fn kill(&mut self) {
        let _ = self.child.start_kill();
    }
}

impl Drop for OpencodeServer {
    fn drop(&mut self) {
        self.kill();
    }
}

// ── REST Client ────────────────────────────────────────────────────────────

pub struct OpencodeClient {
    client: Client,
    pub base_url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Session {
    pub id: String,
    pub slug: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptPart {
    #[serde(rename = "type")]
    pub part_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRef {
    pub provider_id: String,
    pub model_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Provider {
    pub id: String,
    pub name: Option<String>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub models: std::collections::HashMap<String, ModelInfo>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ModelInfo {
    pub id: String,
    pub name: Option<String>,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub attachment: bool,
    pub modalities: Option<Modalities>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Modalities {
    #[serde(default)]
    pub input: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderListResponse {
    pub all: Vec<Provider>,
}

impl OpencodeClient {
    pub fn new(base_url: &str) -> Self {
        OpencodeClient {
            client: Client::new(),
            base_url: base_url.to_string(),
        }
    }

    pub async fn session_create(&self, title: &str) -> Result<Session, String> {
        let resp = self
            .client
            .post(format!("{}/session", self.base_url))
            .json(&json!({ "title": title }))
            .send()
            .await
            .map_err(|e| format!("session.create: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("session.create: HTTP {}", resp.status()));
        }
        resp.json()
            .await
            .map_err(|e| format!("session.create parse: {}", e))
    }

    pub async fn session_prompt(
        &self,
        session_id: &str,
        parts: Vec<PromptPart>,
        model: Option<ModelRef>,
    ) -> Result<(), String> {
        let mut body = json!({
            "sessionID": session_id,
            "parts": parts,
        });
        if let Some(m) = model {
            body["model"] = json!({ "providerID": m.provider_id, "modelID": m.model_id });
        }
        let resp = self
            .client
            .post(format!("{}/session/{}/message", self.base_url, session_id))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("session.prompt: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("session.prompt: HTTP {} — {}", status, body));
        }
        Ok(())
    }

    pub async fn provider_list(&self) -> Result<ProviderListResponse, String> {
        let resp = self
            .client
            .get(format!("{}/provider", self.base_url))
            .send()
            .await
            .map_err(|e| format!("provider.list: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("provider.list: HTTP {}", resp.status()));
        }
        resp.json()
            .await
            .map_err(|e| format!("provider.list parse: {}", e))
    }

    pub async fn session_list(&self) -> Result<Vec<Session>, String> {
        let resp = self
            .client
            .get(format!("{}/session", self.base_url))
            .send()
            .await
            .map_err(|e| format!("session.list: {}", e))?;
        if !resp.status().is_success() {
            return Err(format!("session.list: HTTP {}", resp.status()));
        }
        resp.json()
            .await
            .map_err(|e| format!("session.list parse: {}", e))
    }

    pub async fn session_messages(&self, session_id: &str) -> Result<Vec<Value>, String> {
        let resp = self
            .client
            .get(format!("{}/session/{}/message", self.base_url, session_id))
            .send()
            .await
            .map_err(|e| format!("session.messages: {}", e))?;
        if !resp.status().is_success() {
            return Err(format!("session.messages: HTTP {}", resp.status()));
        }
        resp.json()
            .await
            .map_err(|e| format!("session.messages parse: {}", e))
    }

    pub async fn session_abort(&self, session_id: &str) -> Result<(), String> {
        let resp = self
            .client
            .post(format!("{}/session/{}/abort", self.base_url, session_id))
            .send()
            .await
            .map_err(|e| format!("session.abort: {}", e))?;
        if !resp.status().is_success() {
            return Err(format!("session.abort: HTTP {}", resp.status()));
        }
        Ok(())
    }

    /// Subscribe to SSE events. Returns the raw response for streaming.
    pub async fn event_subscribe(&self) -> Result<reqwest::Response, String> {
        self.client
            .get(format!("{}/event", self.base_url))
            .send()
            .await
            .map_err(|e| format!("event.subscribe: {}", e))
    }
}
