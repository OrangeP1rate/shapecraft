//! ComfyUI on this computer or the local network: free, private, offline. Built in: FLUX.2 klein (small and quick,
//! edits pictures; 4B runs on a laptop) and FLUX.1 Kontext [dev]; people can paste their own workflow instead.
//! The address is found on its own (the usual ports on this computer) or typed in (another machine on the network).

use shapecraft_plugin::{Value, Values, host, http, json};

use crate::{Job, Made, Model, http_error, settings};

/// Where ComfyUI usually listens: the server's default, then ComfyUI Desktop's.
const LOOK_AT: [&str; 4] = ["http://127.0.0.1:8188", "http://127.0.0.1:8000", "http://127.0.0.1:8189", "http://localhost:8188"];

pub fn defaults() -> Vec<Model> {
    let custom = !settings().str("comfy_workflow").trim().is_empty();
    let mut v = vec![
        Model { id: "klein".into(), name: "FLUX.2 klein".into(), detail: "free".into() },
        Model { id: "kontext".into(), name: "FLUX.1 Kontext [dev]".into(), detail: "free".into() },
    ];
    if custom {
        v.push(Model { id: "custom".into(), name: "My Workflow".into(), detail: "free".into() });
    }
    v
}

fn typed(v: &Values) -> Option<String> {
    let u = v.str("comfy_url");
    let u = u.trim();
    if u.is_empty() {
        return None;
    }
    let u = if u.contains("://") { u.to_string() } else { format!("http://{u}") };
    Some(u.trim_end_matches('/').to_string())
}

/// Whether ComfyUI answers at `b` (a quick look).
fn answers(b: &str) -> Option<Value> {
    let r = http::send("GET", &format!("{b}/system_stats"), Vec::new(), Vec::new(), 3).ok()?;
    (r.status == 200).then(|| http::json(&r).ok()).flatten()
}

/// The address to use: the typed one, or the first usual one that answers.
fn base(v: &Values) -> Result<String, String> {
    if let Some(t) = typed(v) {
        return Ok(t);
    }
    LOOK_AT.iter().find(|u| answers(u).is_some()).map(|u| u.to_string()).ok_or_else(|| {
        "ComfyUI isn't running on this computer (looked at ports 8188, 8000 and 8189). Start it, or type its address \
         in Preferences ▸ Plugins ▸ Generative Render."
            .to_string()
    })
}

/// Looks for ComfyUI and fills in the address it found.
pub fn find(v: &Values) -> Result<String, String> {
    let candidates: Vec<String> = typed(v).into_iter().chain(LOOK_AT.iter().map(|s| s.to_string())).collect();
    for u in candidates {
        if let Some(j) = answers(&u) {
            let version = j.pointer("/system/comfyui_version").and_then(Value::as_str).unwrap_or("?");
            let ready = ready_text(&u);
            return Ok(json!({ "message": format!("Found ComfyUI {version} at {u}. {ready}"), "set": { "comfy_url": u } }).to_string());
        }
    }
    Err("No ComfyUI answered on this computer (ports 8188, 8000, 8189). If it runs on another machine, type its \
         address, like 192.168.1.20:8188 (start it with --listen so others can reach it)."
        .into())
}

/// Which built-in models are installed there, in words.
fn ready_text(b: &str) -> String {
    let mut ok = Vec::new();
    let mut missing = Vec::new();
    match klein_files(b) {
        Ok(_) => ok.push("FLUX.2 klein"),
        Err(e) => missing.push(e),
    }
    match find_models(b) {
        Ok(_) => ok.push("FLUX.1 Kontext"),
        Err(e) => missing.push(e),
    }
    if ok.is_empty() {
        format!("No built-in model is installed yet: {}", missing.first().cloned().unwrap_or_default())
    } else {
        format!("Ready: {}.", ok.join(" and "))
    }
}

/// The models this ComfyUI can run now (for the Model menu).
pub fn list() -> Result<Vec<Model>, String> {
    let v = settings();
    let b = base(&v)?;
    let mut out = Vec::new();
    if let Ok(f) = klein_files(&b) {
        let size = if f.unet.contains("9b") { "9B" } else { "4B" };
        out.push(Model { id: "klein".into(), name: format!("FLUX.2 klein {size}"), detail: "free".into() });
    }
    if find_models(&b).is_ok() {
        out.push(Model { id: "kontext".into(), name: "FLUX.1 Kontext [dev]".into(), detail: "free".into() });
    }
    if !v.str("comfy_workflow").trim().is_empty() {
        out.push(Model { id: "custom".into(), name: "My Workflow".into(), detail: "free".into() });
    }
    if out.is_empty() {
        return Err(ready_text(&b));
    }
    Ok(out)
}

pub fn test(v: &Values) -> Result<String, String> {
    let b = base(v)?;
    let j = answers(&b).ok_or_else(|| format!("Nothing is answering at {b} (is ComfyUI running?)"))?;
    let version = j.pointer("/system/comfyui_version").and_then(Value::as_str).unwrap_or("?");
    let gpu = j.pointer("/devices/0/name").and_then(Value::as_str).unwrap_or("no GPU listed");
    shapecraft_plugin::message(&format!("ComfyUI {version} is running at {b} ({gpu}). {}", ready_text(&b)))
}

struct KleinFiles {
    unet: String,
    te: String,
    vae: String,
}

fn klein_files(b: &str) -> Result<KleinFiles, String> {
    let pick = |list: &[String], want: &str| list.iter().find(|f| f.to_lowercase().contains(want)).cloned();
    let unets = options(b, "UNETLoader", "unet_name")?;
    let tes = options(b, "CLIPLoader", "clip_name")?;
    let vaes = options(b, "VAELoader", "vae_name")?;
    let unet = unets
        .iter()
        .find(|f| f.to_lowercase().contains("klein") && !f.to_lowercase().contains("base"))
        .cloned()
        .or_else(|| pick(&unets, "klein"));
    let te = pick(&tes, "qwen_3");
    let vae = pick(&vaes, "flux2");
    match (unet, te, vae) {
        (Some(unet), Some(te), Some(vae)) => Ok(KleinFiles { unet, te, vae }),
        (u, t, va) => {
            let mut m = Vec::new();
            if u.is_none() {
                m.push("a FLUX.2 klein model in models/diffusion_models");
            }
            if t.is_none() {
                m.push("qwen_3_4b in models/text_encoders");
            }
            if va.is_none() {
                m.push("flux2-vae in models/vae");
            }
            Err(format!("FLUX.2 klein isn't set up: missing {}.", m.join(", ")))
        }
    }
}

/// FLUX.2 klein editing the view (as ComfyUI's own "Image Edit (FLUX.2 Klein 4B Distilled)" template): the view as
/// a reference, 4 steps, no guidance. With a repaint `mask` (Keep My Model Exact) it starts from the view itself and
/// repaints only where the mask is white, so the model stays where it is and can't come back as a second copy.
fn klein(f: &KleinFiles, image: &str, mask: Option<&str>, prompt: &str, seed: u64) -> Value {
    let steps = if f.unet.contains("base") { 28 } else { 4 };
    let cfg = if f.unet.contains("base") { 4.0 } else { 1.0 };
    let mut w = json!({
        "1": { "class_type": "UNETLoader", "inputs": { "unet_name": f.unet, "weight_dtype": "default" } },
        "2": { "class_type": "CLIPLoader", "inputs": { "clip_name": f.te, "type": "flux2", "device": "default" } },
        "3": { "class_type": "VAELoader", "inputs": { "vae_name": f.vae } },
        "4": { "class_type": "LoadImage", "inputs": { "image": image } },
        "5": { "class_type": "ImageScaleToTotalPixels", "inputs": { "image": ["4", 0], "upscale_method": "lanczos", "megapixels": 1.0, "resolution_steps": 1 } },
        "6": { "class_type": "GetImageSize", "inputs": { "image": ["5", 0] } },
        "7": { "class_type": "CLIPTextEncode", "inputs": { "text": prompt, "clip": ["2", 0] } },
        "8": { "class_type": "ConditioningZeroOut", "inputs": { "conditioning": ["7", 0] } },
        "9": { "class_type": "VAEEncode", "inputs": { "pixels": ["5", 0], "vae": ["3", 0] } },
        "10": { "class_type": "ReferenceLatent", "inputs": { "conditioning": ["7", 0], "latent": ["9", 0] } },
        "11": { "class_type": "ReferenceLatent", "inputs": { "conditioning": ["8", 0], "latent": ["9", 0] } },
        "12": { "class_type": "CFGGuider", "inputs": { "model": ["1", 0], "positive": ["10", 0], "negative": ["11", 0], "cfg": cfg } },
        "13": { "class_type": "KSamplerSelect", "inputs": { "sampler_name": "euler" } },
        "14": { "class_type": "Flux2Scheduler", "inputs": { "steps": steps, "width": ["6", 0], "height": ["6", 1] } },
        "15": { "class_type": "EmptyFlux2LatentImage", "inputs": { "width": ["6", 0], "height": ["6", 1], "batch_size": 1 } },
        "16": { "class_type": "RandomNoise", "inputs": { "noise_seed": seed } },
        "17": { "class_type": "SamplerCustomAdvanced", "inputs": { "noise": ["16", 0], "guider": ["12", 0], "sampler": ["13", 0], "sigmas": ["14", 0], "latent_image": ["15", 0] } },
        "18": { "class_type": "VAEDecode", "inputs": { "samples": ["17", 0], "vae": ["3", 0] } },
        "19": { "class_type": "SaveImage", "inputs": { "images": ["18", 0], "filename_prefix": "ShapeCraft" } }
    });
    if let Some(m) = mask {
        // The mask is scaled by the same node as the view, so the two line up exactly.
        add_mask(
            &mut w,
            m,
            json!({ "class_type": "ImageScaleToTotalPixels", "inputs": {
            "image": ["m1", 0], "upscale_method": "lanczos", "megapixels": 1.0, "resolution_steps": 1 } }),
            "9",
        );
        w["17"]["inputs"]["latent_image"] = json!(["m4", 0]);
    }
    w
}

/// Nodes m1..m4: the uploaded repaint mask, scaled by `scale` (reading from m1), made a mask (white = repaint),
/// grown a little so contact shadows reach the model's edge, and set on the view's latent (`latent` node).
fn add_mask(w: &mut Value, mask: &str, scale: Value, latent: &str) {
    w["m1"] = json!({ "class_type": "LoadImage", "inputs": { "image": mask } });
    w["m2"] = scale;
    w["m3"] = json!({ "class_type": "ImageToMask", "inputs": { "image": ["m2", 0], "channel": "red" } });
    w["m5"] = json!({ "class_type": "GrowMask", "inputs": { "mask": ["m3", 0], "expand": 2, "tapered_corners": true } });
    w["m4"] = json!({ "class_type": "SetLatentNoiseMask", "inputs": { "samples": [latent, 0], "mask": ["m5", 0] } });
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

fn kontext(f: &Files, image: &str, mask: Option<&str>, prompt: &str, seed: u64) -> Value {
    let mut w = json!({
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
    });
    if let Some(m) = mask {
        add_mask(&mut w, m, json!({ "class_type": "FluxKontextImageScale", "inputs": { "image": ["m1", 0] } }), "6");
        w["11"]["inputs"]["latent_image"] = json!(["m4", 0]);
    }
    w
}

/// The person's own workflow with {{image}}, {{prompt}}, {{seed}} and {{mask}} (the repaint mask: white around the
/// model; the view itself without Keep My Model Exact) filled in.
fn custom(text: &str, image: &str, mask: Option<&str>, prompt: &str, seed: u64) -> Result<Value, String> {
    let quoted = |s: &str| Value::from(s).to_string();
    let filled = text
        .replace("\"{{mask}}\"", &quoted(mask.unwrap_or(image)))
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
    let b = base(&v)?;
    host::progress(-1.0, "Sending the view to ComfyUI…");
    let image = upload(&b, &job.png)?;
    // Keep My Model Exact: repaint only around the model.
    let mask = match &job.mask_png {
        Some(m) => Some(upload(&b, &repaint_mask(m)?)?),
        None => None,
    };
    enum Flow {
        Klein(KleinFiles),
        Kontext(Files),
        Custom,
    }
    let flow = match job.model {
        "custom" => Flow::Custom,
        "kontext" => Flow::Kontext(find_models(&b)?),
        _ => Flow::Klein(klein_files(&b)?),
    };
    let mut pictures = Vec::new();
    let mut seeds = Vec::new();
    for _ in 0..job.count {
        let r = host::random(6);
        let seed = r.iter().fold(0u64, |a, x| (a << 8) | u64::from(*x));
        let wf = match &flow {
            Flow::Klein(f) => klein(f, &image, mask.as_deref(), &job.prompt, seed),
            Flow::Kontext(f) => kontext(f, &image, mask.as_deref(), &job.prompt, seed),
            Flow::Custom => custom(&v.str("comfy_workflow"), &image, mask.as_deref(), &job.prompt, seed)?,
        };
        pictures.push(run_one(&b, wf)?);
        seeds.push(seed.to_string());
    }
    Ok(Made { pictures, note: format!("seed {}", seeds.join(", ")) })
}

/// The edit mask (solid on the model) turned into a picture ComfyUI can scale like the view: white where the scene is
/// repainted, black on the model.
fn repaint_mask(edit_mask_png: &[u8]) -> Result<Vec<u8>, String> {
    let m = host::decode_image(edit_mask_png)?;
    let pixels = m.pixels.as_chunks::<4>().0.iter().flat_map(|p| {
        let v = 255 - p[3];
        [v, v, v, 255]
    });
    host::encode_png(&host::Image { width: m.width, height: m.height, pixels: pixels.collect() })
}
