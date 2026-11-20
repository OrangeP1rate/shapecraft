//! ComfyUI on this computer or the local network: free, private, offline. The built-in workflow is FLUX.1 Kontext
//! [dev] (an image-editing model that keeps what it's told to keep); people can paste their own workflow instead.

use shapecraft_plugin::{Value, Values, host, http, json};

use crate::{Job, Made, Model, http_error, settings};

pub const DEFAULT_URL: &str = "http://127.0.0.1:8188";

pub fn defaults() -> Vec<Model> {
    let custom = !settings().str("comfy_workflow").trim().is_empty();
    let mut v = vec![Model { id: "kontext".into(), name: "FLUX.1 Kontext [dev]".into(), detail: "free".into() }];
    if custom {
        v.push(Model { id: "custom".into(), name: "My Workflow".into(), detail: "free".into() });
    }
    v
}

fn base(v: &Values) -> String {
    let u = v.str("comfy_url");
    let u = if u.trim().is_empty() { DEFAULT_URL.to_string() } else { u.trim().to_string() };
    let u = if u.contains("://") { u } else { format!("http://{u}") };
    u.trim_end_matches('/').to_string()
}

pub fn test(v: &Values) -> Result<String, String> {
    let b = base(v);
    let r = http::get(&format!("{b}/system_stats"), &[])?;
    if r.status != 200 {
        return Err(http_error("ComfyUI", &r));
    }
    let j = http::json(&r)?;
    let version = j.pointer("/system/comfyui_version").and_then(Value::as_str).unwrap_or("?");
    let gpu = j.pointer("/devices/0/name").and_then(Value::as_str).unwrap_or("no GPU listed");
    let kontext = match find_models(&b) {
        Ok(_) => "FLUX.1 Kontext is ready.".to_string(),
        Err(e) => e,
    };
    shapecraft_plugin::message(&format!("ComfyUI {version} is running ({gpu}). {kontext}"))
}

/// The options a loader node offers (file names in its models folder).
fn options(b: &str, node: &str, input: &str) -> Result<Vec<String>, String> {
    let r = http::get(&format!("{b}/object_info/{node}"), &[])?;
    if r.status != 200 {
        return Err(http_error("ComfyUI", &r));
    }
    let j = http::json(&r)?;
    let list = j
        .pointer(&format!("/{node}/input/required/{input}/0"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>());
    list.ok_or_else(|| format!("ComfyUI has no {node} node (update ComfyUI)."))
}

struct Files {
    unet: String,
    clip_l: String,
    t5: String,
    vae: String,
}

fn find_models(b: &str) -> Result<Files, String> {
    let pick = |list: &[String], want: &[&str]| -> Option<String> {
        list.iter().find(|f| want.iter().all(|w| f.to_lowercase().contains(w))).cloned()
    };
    let unets = options(b, "UNETLoader", "unet_name")?;
    let clips = options(b, "DualCLIPLoader", "clip_name1")?;
    let vaes = options(b, "VAELoader", "vae_name")?;
    for node in ["FluxKontextImageScale", "ReferenceLatent"] {
        let r = http::get(&format!("{b}/object_info/{node}"), &[])?;
        if r.status != 200 || http::json(&r).map(|j| j.get(node).is_none()).unwrap_or(true) {
            return Err(format!("ComfyUI lacks the {node} node: update ComfyUI for FLUX.1 Kontext."));
        }
    }
    let mut missing = Vec::new();
    let unet = pick(&unets, &["kontext"]);
    let clip_l = pick(&clips, &["clip_l"]);
    let t5 = pick(&clips, &["t5xxl"]);
    let vae = pick(&vaes, &["ae"]).or_else(|| pick(&vaes, &["flux"]));
    if unet.is_none() {
        missing.push("a FLUX.1 Kontext model in models/diffusion_models");
    }
    if clip_l.is_none() {
        missing.push("clip_l in models/text_encoders");
    }
    if t5.is_none() {
        missing.push("a t5xxl encoder in models/text_encoders");
    }
    if vae.is_none() {
        missing.push("ae.safetensors in models/vae");
    }
    match (unet, clip_l, t5, vae) {
        (Some(unet), Some(clip_l), Some(t5), Some(vae)) => Ok(Files { unet, clip_l, t5, vae }),
        _ => Err(format!("FLUX.1 Kontext isn't set up: missing {}.", missing.join(", "))),
    }
}

fn kontext(f: &Files, image: &str, prompt: &str, seed: u64) -> Value {
    json!({
        "1": { "class_type": "UNETLoader", "inputs": { "unet_name": f.unet, "weight_dtype": "default" } },
        "2": { "class_type": "DualCLIPLoader", "inputs": { "clip_name1": f.clip_l, "clip_name2": f.t5, "type": "flux", "device": "default" } },
        "3": { "class_type": "VAELoader", "inputs": { "vae_name": f.vae } },
        "4": { "class_type": "LoadImage", "inputs": { "image": image } },
        "5": { "class_type": "FluxKontextImageScale", "inputs": { "image": ["4", 0] } },
        "6": { "class_type": "VAEEncode", "inputs": { "pixels": ["5", 0], "vae": ["3", 0] } },
        "7": { "class_type": "CLIPTextEncode", "inputs": { "text": prompt, "clip": ["2", 0] } },
        "8": { "class_type": "ReferenceLatent", "inputs": { "conditioning": ["7", 0], "latent": ["6", 0] } },
        "9": { "class_type": "FluxGuidance", "inputs": { "conditioning": ["8", 0], "guidance": 2.5 } },
        "10": { "class_type": "ConditioningZeroOut", "inputs": { "conditioning": ["7", 0] } },
        "11": { "class_type": "KSampler", "inputs": {
            "model": ["1", 0], "positive": ["9", 0], "negative": ["10", 0], "latent_image": ["6", 0],
            "seed": seed, "steps": 20, "cfg": 1.0, "sampler_name": "euler", "scheduler": "simple", "denoise": 1.0 } },
        "12": { "class_type": "VAEDecode", "inputs": { "samples": ["11", 0], "vae": ["3", 0] } },
        "13": { "class_type": "SaveImage", "inputs": { "images": ["12", 0], "filename_prefix": "ShapeCraft" } }
    })
}

/// The person's own workflow with {{image}}, {{prompt}} and {{seed}} filled in.
fn custom(text: &str, image: &str, prompt: &str, seed: u64) -> Result<Value, String> {
    let quoted = |s: &str| Value::from(s).to_string();
    let filled = text
        .replace("\"{{seed}}\"", &seed.to_string())
        .replace("{{seed}}", &seed.to_string())
        .replace("\"{{image}}\"", &quoted(image))
        .replace("\"{{prompt}}\"", &quoted(prompt));
    let v: Value = serde_json::from_str(&filled).map_err(|e| format!("Your workflow isn't valid JSON ({e})."))?;
    let v = v.get("prompt").cloned().unwrap_or(v);
    if !v.is_object() || v.as_object().is_some_and(|o| o.values().any(|n| n.get("class_type").is_none())) {
        return Err("Your workflow needs the API format (in ComfyUI: Workflow ▸ Export (API)).".into());
    }
    Ok(v)
}

fn upload(b: &str, png: &[u8]) -> Result<String, String> {
    let name = format!("shapecraft-{}.png", http::hex(&host::random(6)));
    let parts = [
        http::Part { name: "image", filename: Some(&name), content_type: Some("image/png"), data: png },
        http::Part { name: "overwrite", filename: None, content_type: None, data: b"true" },
    ];
    let r = http::post_multipart(&format!("{b}/upload/image"), &[], &parts, 60)?;
    if r.status != 200 {
        return Err(http_error("ComfyUI", &r));
    }
    let j = http::json(&r)?;
    let n = j.get("name").and_then(Value::as_str).ok_or("ComfyUI didn't take the picture.")?;
    Ok(match j.get("subfolder").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        Some(sub) => format!("{sub}/{n}"),
        None => n.to_string(),
    })
}

fn run_one(b: &str, workflow: Value) -> Result<Vec<u8>, String> {
    let client = http::hex(&host::random(8));
    let r = http::post_json(&format!("{b}/prompt"), &[], &json!({ "prompt": workflow, "client_id": client }), 60)?;
    if r.status != 200 {
        let j = http::json(&r).unwrap_or(Value::Null);
        let why = j.pointer("/error/message").and_then(Value::as_str).unwrap_or("");
        let details = j.pointer("/node_errors").map(|n| n.to_string().chars().take(300).collect::<String>()).unwrap_or_default();
        return Err(format!("ComfyUI refused the workflow: {why} {details}"));
    }
    let id = http::json(&r)?.get("prompt_id").and_then(Value::as_str).map(str::to_string).ok_or("ComfyUI didn't queue it.")?;
    let start = host::now();
    loop {
        if !host::wait(1000) {
            let _ = http::post_json(&format!("{b}/interrupt"), &[], &json!({}), 10);
            return Err("cancelled".into());
        }
        let secs = (host::now().saturating_sub(start)) / 1000;
        host::progress(-1.0, &format!("Generating scene on your computer… {secs} s"));
        let r = http::get(&format!("{b}/history/{id}"), &[])?;
        if r.status != 200 {
            continue;
        }
        let j = http::json(&r)?;
        let Some(entry) = j.get(&id) else { continue };
        if entry.pointer("/status/status_str").and_then(Value::as_str) == Some("error") {
            return Err("ComfyUI stopped with an error (see its window or log).".into());
        }
        let outputs = entry.get("outputs").and_then(Value::as_object);
        let image = outputs.into_iter().flat_map(|o| o.values()).find_map(|o| o.pointer("/images/0").cloned());
        if let Some(img) = image {
            let q = |k: &str| shapecraft_plugin::oauth::encode(img.get(k).and_then(Value::as_str).unwrap_or(""));
            let url = format!("{b}/view?filename={}&subfolder={}&type={}", q("filename"), q("subfolder"), q("type"));
            let r = http::get(&url, &[])?;
            if r.status != 200 {
                return Err(http_error("ComfyUI", &r));
            }
            return Ok(r.body);
        }
        if secs > 1800 {
            return Err("ComfyUI took over 30 minutes; stopped waiting.".into());
        }
    }
}

pub fn generate(job: &Job) -> Result<Made, String> {
    let v = settings();
    let b = base(&v);
    let image = upload(&b, &job.png)?;
    let files = if job.model == "custom" { None } else { Some(find_models(&b)?) };
    let mut pictures = Vec::new();
    let mut seeds = Vec::new();
    for _ in 0..job.count {
        let r = host::random(6);
        let seed = r.iter().fold(0u64, |a, x| (a << 8) | u64::from(*x));
        let wf = match &files {
            Some(f) => kontext(f, &image, &job.prompt, seed),
            None => custom(&v.str("comfy_workflow"), &image, &job.prompt, seed)?,
        };
        pictures.push(run_one(&b, wf)?);
        seeds.push(seed.to_string());
    }
    Ok(Made { pictures, note: format!("seed {}", seeds.join(", ")) })
}
