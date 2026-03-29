use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

pub struct OpencodeServer {
    pub url: String,
    child: Child,
}

impl OpencodeServer {
    /// Spawn `opencode serve` and wait for it to be listening.
    pub async fn spawn(config: &Value) -> Result<Self, String> {
        let mut cmd = Command::new("opencode");
        cmd.args(["serve", "--hostname=127.0.0.1", "--port=0"])
            .env("OPENCODE_CONFIG_CONTENT", config.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            format!("Failed to spawn opencode serve: {}. Is 'opencode' in PATH?", e)
        })?;

        let stdout = child.stdout.take().ok_or("No stdout from opencode")?;
        let mut reader = BufReader::new(stdout).lines();

        // Wait for the "opencode server listening on http://..." line
        let url = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let mut output = String::new();
            while let Ok(Some(line)) = reader.next_line().await {
                output.push_str(&line);
                output.push('\n');
                if line.contains("opencode server listening") {
                    if let Some(start) = line.find("http") {
                        let url = line[start..].trim().to_string();
                        return Ok(url);
                    }
                }
            }
            Err(format!(
                "opencode exited without printing listen URL. Output: {}",
                output
            ))
        })
        .await
        .map_err(|_| "Timeout waiting for opencode serve to start".to_string())??;

        // Spawn a task to drain remaining stdout so the pipe doesn't block
        tokio::spawn(async move {
            while let Ok(Some(_)) = reader.next_line().await {}
        });

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
