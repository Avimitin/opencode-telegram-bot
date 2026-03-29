use crate::opencode::OpencodeClient;
use serde_json::{json, Value};

pub struct ModelEntry {
    pub full_id: String,
    pub label: String,
}

pub struct ModelCache {
    models: Option<Vec<ModelEntry>>,
}

const MODELS_PER_PAGE: usize = 6;

impl ModelCache {
    pub fn new() -> Self {
        ModelCache { models: None }
    }

    /// Returns the cached model list, fetching once on first call.
    /// The list is static for the lifetime of the opencode server.
    pub async fn get_models(&mut self, client: &OpencodeClient) -> anyhow::Result<Vec<ModelEntry>> {
        if let Some(ref models) = self.models {
            return Ok(models.clone());
        }

        let models = fetch_models(client).await?;
        self.models = Some(models.clone());
        Ok(models)
    }
}

async fn fetch_models(client: &OpencodeClient) -> anyhow::Result<Vec<ModelEntry>> {
    let result = client.provider_list().await?;
    let mut models = Vec::new();

    for provider in &result.all {
        let env_configured =
            provider.env.is_empty() || provider.env.iter().any(|e| std::env::var(e).is_ok());
        if !env_configured {
            continue;
        }

        for model in provider.models.values() {
            let full_id = format!("{}/{}", provider.id, model.id);
            let mut tags = Vec::new();
            if model.reasoning {
                tags.push("🧠");
            }
            if let Some(ref modalities) = model.modalities
                && modalities.input.contains(&"image".to_string()) {
                    tags.push("🖼");
                }
            if model.attachment {
                tags.push("📎");
            }
            let tag_str = if tags.is_empty() {
                String::new()
            } else {
                format!(" {}", tags.join(""))
            };
            models.push(ModelEntry {
                full_id: full_id.clone(),
                label: format!("{}{}", full_id, tag_str),
            });
        }
    }
    Ok(models)
}

impl Clone for ModelEntry {
    fn clone(&self) -> Self {
        ModelEntry {
            full_id: self.full_id.clone(),
            label: self.label.clone(),
        }
    }
}

pub fn build_model_keyboard(models: &[ModelEntry], page: usize) -> Value {
    let total_pages = models.len().div_ceil(MODELS_PER_PAGE);
    let start = page * MODELS_PER_PAGE;
    let page_models = &models[start..models.len().min(start + MODELS_PER_PAGE)];

    let mut rows: Vec<Value> = Vec::new();

    // Top nav
    if total_pages > 1 && page > 0 {
        rows.push(json!([{
            "text": format!("⬅️ Page {}", page),
            "callback_data": format!("modelpage:{}", page - 1)
        }]));
    }

    // Model buttons
    for m in page_models {
        rows.push(json!([{
            "text": &m.label,
            "callback_data": format!("model:{}", m.full_id)
        }]));
    }

    // Bottom nav
    if total_pages > 1 && page < total_pages - 1 {
        rows.push(json!([{
            "text": format!("Page {} ➡️", page + 2),
            "callback_data": format!("modelpage:{}", page + 1)
        }]));
    }

    json!({ "inline_keyboard": rows })
}
