//! Google Gemini with the person's API key: generateContent with the view and the instruction, pictures back.

use shapecraft_plugin::{Value, host, http, json};

use crate::{Job, Made, Model, http_error};

const API: &str = "https://generativelanguage.googleapis.com/v1beta";

pub fn defaults() -> Vec<Model> {
    vec![
        Model { id: "gemini-3.1-flash-image".into(), name: "Gemini 3.1 Flash Image".into(), detail: String::new() },
        Model { id: "gemini-2.5-flash-image".into(), name: "Gemini 2.5 Flash Image".into(), detail: String::new() },
    ]
}

fn key() -> Result<String, String> {
    host::secret("gemini_key").ok_or_else(|| "Add a Gemini API key in Preferences ▸ Plugins first.".to_string())
}

/// Models whose name says they make images and that answer generateContent.
pub fn list() -> Result<Vec<Model>, String> {
    let k = key()?;
    let r = http::get(&format!("{API}/models?pageSize=200"), &[("x-goog-api-key", &k)])?;
    if r.status != 200 {
        return Err(http_error("Google Gemini", &r));
    }
    let out: Vec<Model> = http::json(&r)?
        .get("models")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|m| {
            m.get("supportedGenerationMethods")
                .and_then(Value::as_array)
                .is_some_and(|a| a.iter().any(|x| x.as_str() == Some("generateContent")))
        })
        .filter_map(|m| {
            let id = m.get("name")?.as_str()?.trim_start_matches("models/").to_string();
            (id.contains("image") && !id.contains("imagen")).then(|| Model {
                name: m.get("displayName").and_then(Value::as_str).unwrap_or(&id).to_string(),
                id,
                detail: String::new(),
            })
        })
        .collect();
    if out.is_empty() {
        return Ok(defaults());
    }
    Ok(out)
}

pub fn generate(job: &Job) -> Result<Made, String> {
    let k = key()?;
    let body = json!({
        "contents": [{ "parts": [
            { "text": job.prompt },
            { "inline_data": { "mime_type": "image/png", "data": host::base64_encode(&job.png) } }
        ]}],
        // The view's shape, so the scene is framed like the view (and the model stays where it was).
        "generationConfig": { "responseModalities": ["TEXT", "IMAGE"], "imageConfig": { "aspectRatio": crate::openrouter::aspect(job.width, job.height) } },
    });
    let url = format!("{API}/models/{}:generateContent", job.model);
    let mut pictures = Vec::new();
    for i in 0..job.count {
        if i > 0 && !host::progress(i as f32 / job.count as f32, "Generating scene…") {
            return Err("cancelled".into());
        }
        let r = http::post_json(&url, &[("x-goog-api-key", &k)], &body, 300)?;
        if r.status != 200 {
            return Err(http_error("Google Gemini", &r));
        }
        let j = http::json(&r)?;
        let parts = j.pointer("/candidates/0/content/parts").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        for p in parts {
            let data = p.pointer("/inlineData/data").or_else(|| p.pointer("/inline_data/data")).and_then(Value::as_str);
            if let Some(b) = data {
                pictures.push(host::base64_decode(b)?);
            }
        }
        if pictures.is_empty()
            && let Some(reason) =
                j.pointer("/candidates/0/finishReason").or_else(|| j.pointer("/promptFeedback/blockReason")).and_then(Value::as_str)
        {
            return Err(format!("Google Gemini made no picture ({reason}). Try different words."));
        }
    }
    Ok(Made { pictures, note: String::new() })
}
