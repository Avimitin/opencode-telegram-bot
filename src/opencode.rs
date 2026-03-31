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

use anyhow::Context;
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
    pub async fn spawn(config: &Value) -> anyhow::Result<Self> {
        let port = find_free_port().context("find free port")?;
        let url = format!("http://127.0.0.1:{}", port);

        let mut cmd = Command::new("opencode");
        cmd.args(["serve", "--hostname=127.0.0.1", &format!("--port={}", port)])
            .env("OPENCODE_CONFIG_CONTENT", config.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let child = cmd.spawn().context("spawn opencode serve — is 'opencode' in PATH?")?;

        // Poll health endpoint until server is ready
        let client = Client::builder().no_proxy().build()?;
        let health_url = format!("{}/global/health", url);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!("timeout waiting for opencode serve to start");
            }
            if let Ok(resp) = client.get(&health_url).send().await
                && resp.status().is_success()
            {
                break;
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
        let client = Client::builder()
            .no_proxy()
            .build()
            .expect("failed to build reqwest client");
        OpencodeClient {
            client,
            base_url: base_url.to_string(),
        }
    }

    pub async fn session_create(&self, title: &str) -> anyhow::Result<Session> {
        let resp = self
            .client
            .post(format!("{}/session", self.base_url))
            .json(&json!({ "title": title }))
            .send()
            .await
            .context("session.create")?
            .error_for_status()
            .context("session.create")?;
        resp.json().await.context("session.create parse")
    }

    pub async fn session_prompt(
        &self,
        session_id: &str,
        parts: Vec<PromptPart>,
        model: Option<ModelRef>,
    ) -> anyhow::Result<()> {
        let mut body = json!({
            "sessionID": session_id,
            "parts": parts,
        });
        if let Some(m) = model {
            body["model"] = json!({ "providerID": m.provider_id, "modelID": m.model_id });
        }
        self.client
            .post(format!("{}/session/{}/message", self.base_url, session_id))
            .json(&body)
            .send()
            .await
            .context("session.prompt")?
            .error_for_status()
            .context("session.prompt")?;
        Ok(())
    }

    pub async fn provider_list(&self) -> anyhow::Result<ProviderListResponse> {
        let resp = self
            .client
            .get(format!("{}/provider", self.base_url))
            .send()
            .await
            .context("provider.list")?
            .error_for_status()
            .context("provider.list")?;
        resp.json().await.context("provider.list parse")
    }

    pub async fn session_list(&self) -> anyhow::Result<Vec<Session>> {
        let resp = self
            .client
            .get(format!("{}/session", self.base_url))
            .send()
            .await
            .context("session.list")?
            .error_for_status()
            .context("session.list")?;
        resp.json().await.context("session.list parse")
    }

    pub async fn session_messages(&self, session_id: &str) -> anyhow::Result<Vec<Value>> {
        let resp = self
            .client
            .get(format!("{}/session/{}/message", self.base_url, session_id))
            .send()
            .await
            .context("session.messages")?
            .error_for_status()
            .context("session.messages")?;
        resp.json().await.context("session.messages parse")
    }

    pub async fn session_abort(&self, session_id: &str) -> anyhow::Result<()> {
        self.client
            .post(format!("{}/session/{}/abort", self.base_url, session_id))
            .send()
            .await
            .context("session.abort")?
            .error_for_status()
            .context("session.abort")?;
        Ok(())
    }

    /// Subscribe to SSE events. Returns the raw response for streaming.
    pub async fn event_subscribe(&self) -> anyhow::Result<reqwest::Response> {
        self.client
            .get(format!("{}/event", self.base_url))
            .send()
            .await
            .context("event.subscribe")
    }
}
