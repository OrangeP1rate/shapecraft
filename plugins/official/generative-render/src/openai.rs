//! OpenAI with the person's API key: the image edits endpoint (GPT Image models), the view as the image to edit.

use shapecraft_plugin::{Value, host, http};

use crate::{Job, Made, Model, http_error};

const API: &str = "https://api.openai.com/v1";

pub fn defaults() -> Vec<Model> {
    vec![Model { id: "gpt-image-1".into(), name: "GPT Image 1".into(), detail: String::new() }]
}

fn key() -> Result<String, String> {
    host::secret("openai_key").ok_or_else(|| "Add an OpenAI API key in Preferences ▸ Plugins first.".to_string())
}

/// The image models the key can use.
pub fn list() -> Result<Vec<Model>, String> {
    let auth = format!("Bearer {}", key()?);
    let r = http::get(&format!("{API}/models"), &[("Authorization", &auth)])?;
    if r.status != 200 {
        return Err(http_error("OpenAI", &r));
    }
    let mut out: Vec<Model> = http::json(&r)?
        .get("data")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|m| m.get("id")?.as_str())
        .filter(|id| id.starts_with("gpt-image"))
        .map(|id| Model { id: id.to_string(), name: pretty(id), detail: String::new() })
        .collect();
    out.sort_by(|a, b| b.id.cmp(&a.id));
    if out.is_empty() {
        return Ok(defaults());
    }
    Ok(out)
}

fn pretty(id: &str) -> String {
    id.replace("gpt-image", "GPT Image").replace('-', " ")
}

pub fn generate(job: &Job) -> Result<Made, String> {
    let auth = format!("Bearer {}", key()?);
    let n = job.count.to_string();
    let size = if job.width > job.height * 6 / 5 {
        "1536x1024"
    } else if job.height > job.width * 6 / 5 {
        "1024x1536"
    } else {
        "1024x1024"
    };
    let parts = [
        http::Part { name: "model", filename: None, content_type: None, data: job.model.as_bytes() },
        http::Part { name: "prompt", filename: None, content_type: None, data: job.prompt.as_bytes() },
        http::Part { name: "n", filename: None, content_type: None, data: n.as_bytes() },
        http::Part { name: "size", filename: None, content_type: None, data: size.as_bytes() },
        http::Part { name: "image", filename: Some("view.png"), content_type: Some("image/png"), data: &job.png },
    ];
    let r = http::post_multipart(&format!("{API}/images/edits"), &[("Authorization", &auth)], &parts, 300)?;
    if r.status != 200 {
        return Err(http_error("OpenAI", &r));
    }
    let j = http::json(&r)?;
    let mut pictures = Vec::new();
    for d in j.get("data").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
        if let Some(b) = d.get("b64_json").and_then(Value::as_str) {
            pictures.push(host::base64_decode(b)?);
        }
    }
    Ok(Made { pictures, note: String::new() })
}
