//! OpenRouter: sign in through the browser (OAuth PKCE; the person's own key comes back), then any image model it
//! routes to. Images go through `/api/v1/images`; older routes through chat completions with image output.

use shapecraft_plugin::{Value, host, http, json, oauth};

use crate::{Job, Made, Model, http_error};

const API: &str = "https://openrouter.ai/api/v1";

pub fn defaults() -> Vec<Model> {
    [
        ("google/gemini-2.5-flash-image", "Gemini 2.5 Flash Image (Nano Banana)"),
        ("openai/gpt-image-1", "GPT Image 1"),
        ("black-forest-labs/flux.2-pro", "FLUX.2 Pro"),
        ("bytedance-seed/seedream-4.5", "Seedream 4.5"),
    ]
    .into_iter()
    .map(|(id, name)| Model { id: id.into(), name: name.into(), detail: String::new() })
    .collect()
}

fn key() -> Result<String, String> {
    host::secret("openrouter_key").ok_or_else(|| "Sign in to OpenRouter in Preferences ▸ Plugins first.".to_string())
}

pub fn sign_in() -> Result<(), String> {
    let (verifier, challenge) = oauth::pkce();
    let url = format!(
        "https://openrouter.ai/auth?callback_url={{redirect}}&code_challenge={challenge}&code_challenge_method=S256&key_label={}",
        oauth::encode("ShapeCraft Generative Render")
    );
    let back = host::sign_in(&url)?;
    let code = oauth::query_value(&back.query, "code").ok_or("OpenRouter didn't send a sign-in code back.")?;
    let r = http::post_json(
        &format!("{API}/auth/keys"),
        &[],
        &json!({ "code": code, "code_verifier": verifier, "code_challenge_method": "S256" }),
        30,
    )?;
    if r.status != 200 {
        return Err(http_error("OpenRouter", &r));
    }
    let k = http::json(&r)?.get("key").and_then(Value::as_str).map(str::to_string).ok_or("OpenRouter sent no key.")?;
    host::set_secret("openrouter_key", Some(&k))
}

/// Image models, cheapest first when prices are given.
pub fn list() -> Result<Vec<Model>, String> {
    let k = key()?;
    let auth = format!("Bearer {k}");
    let mut r = http::get(&format!("{API}/images/models"), &[("Authorization", &auth)])?;
    if r.status == 404 {
        r = http::get(&format!("{API}/models?output_modalities=image"), &[("Authorization", &auth)])?;
    }
    if r.status != 200 {
        return Err(http_error("OpenRouter", &r));
    }
    let j = http::json(&r)?;
    let mut out: Vec<Model> = j
        .get("data")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?.to_string();
            let name = m.get("name").and_then(Value::as_str).unwrap_or(&id).to_string();
            let detail = m
                .pointer("/pricing/image")
                .or_else(|| m.pointer("/pricing/per_image"))
                .and_then(|p| p.as_str().map(str::to_string).or_else(|| p.as_f64().map(|f| f.to_string())))
                .and_then(|p| p.parse::<f64>().ok())
                .filter(|p| *p > 0.0)
                .map(|p| format!("${p:.3}"))
                .unwrap_or_default();
            Some(Model { id, name, detail })
        })
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    if out.is_empty() {
        return Err("OpenRouter listed no image models.".into());
    }
    Ok(out)
}

pub fn generate(job: &Job) -> Result<Made, String> {
    let k = key()?;
    let auth = format!("Bearer {k}");
    let data_url = format!("data:image/png;base64,{}", host::base64_encode(&job.png));
    let headers = [("Authorization", auth.as_str()), ("X-Title", "ShapeCraft")];
    let body = json!({
        "model": job.model,
        "prompt": job.prompt,
        "n": job.count,
        "input_references": [{ "type": "image_url", "image_url": { "url": data_url } }],
        "aspect_ratio": aspect(job.width, job.height),
    });
    let r = http::post_json(&format!("{API}/images"), &headers, &body, 300)?;
    if r.status == 404 || r.status == 405 {
        return chat(job, &headers, &data_url);
    }
    if r.status != 200 {
        return Err(http_error("OpenRouter", &r));
    }
    let j = http::json(&r)?;
    let mut pictures = Vec::new();
    for d in j.get("data").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
        if let Some(b) = d.get("b64_json").and_then(Value::as_str) {
            pictures.push(host::base64_decode(b)?);
        } else if let Some(u) = d.get("url").and_then(Value::as_str) {
            pictures.push(fetch_url(u)?);
        }
    }
    let note = j.pointer("/usage/cost").and_then(Value::as_f64).map(|c| format!("${c:.3}")).unwrap_or_default();
    Ok(Made { pictures, note })
}

/// The chat route: image models that answer with pictures in the message.
fn chat(job: &Job, headers: &[(&str, &str)], data_url: &str) -> Result<Made, String> {
    let mut pictures = Vec::new();
    let mut cost = 0.0;
    for _ in 0..job.count {
        let body = json!({
            "model": job.model,
            "modalities": ["image", "text"],
            "messages": [{ "role": "user", "content": [
                { "type": "text", "text": job.prompt },
                { "type": "image_url", "image_url": { "url": data_url } }
            ]}],
        });
        let r = http::post_json(&format!("{API}/chat/completions"), headers, &body, 300)?;
        if r.status != 200 {
            return Err(http_error("OpenRouter", &r));
        }
        let j = http::json(&r)?;
        cost += j.pointer("/usage/cost").and_then(Value::as_f64).unwrap_or(0.0);
        for img in j.pointer("/choices/0/message/images").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
            if let Some(u) = img.pointer("/image_url/url").and_then(Value::as_str) {
                pictures.push(from_data_url(u).map_or_else(|| fetch_url(u), Ok)?);
            }
        }
        if !host::progress(-1.0, "Generating scene…") {
            return Err("cancelled".into());
        }
    }
    let note = if cost > 0.0 { format!("${cost:.3}") } else { String::new() };
    Ok(Made { pictures, note })
}

fn from_data_url(u: &str) -> Option<Vec<u8>> {
    let (_, b64) = u.strip_prefix("data:")?.split_once(";base64,")?;
    host::base64_decode(b64).ok()
}

fn fetch_url(u: &str) -> Result<Vec<u8>, String> {
    let r = http::get(u, &[])?;
    if r.status != 200 {
        return Err(http_error("OpenRouter", &r));
    }
    Ok(r.body)
}

/// The nearest common aspect ratio ("3:2").
pub fn aspect(w: u32, h: u32) -> &'static str {
    let r = w as f64 / h.max(1) as f64;
    let options = [
        ("21:9", 21.0 / 9.0),
        ("16:9", 16.0 / 9.0),
        ("3:2", 1.5),
        ("4:3", 4.0 / 3.0),
        ("1:1", 1.0),
        ("3:4", 0.75),
        ("2:3", 2.0 / 3.0),
        ("9:16", 9.0 / 16.0),
    ];
    options.iter().min_by(|a, b| (a.1 - r).abs().total_cmp(&(b.1 - r).abs())).map_or("1:1", |o| o.0)
}
