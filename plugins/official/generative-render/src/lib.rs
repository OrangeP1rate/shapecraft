//! Generative Render: a picture of the model in a scene the person describes, made by the AI service they connect
//! (or one running on their own computer). The view comes from the host's capture; the model's own pixels can be
//! laid back over the result ("Keep My Model Exact") so the AI can't change its shape.

mod comfy;
mod gemini;
mod openai;
mod openrouter;

use shapecraft_plugin::{Guest, Panel, Value, Values, host, json, ui};

struct GenerativeRender;

/// The services, in the order they're listed.
pub const SERVICES: [(&str, &str); 4] =
    [("openrouter", "OpenRouter"), ("openai", "OpenAI"), ("gemini", "Google Gemini"), ("comfy", "On This Computer (ComfyUI)")];

pub const MODES: [&str; 3] = ["Surroundings", "Whole Picture", "Selected Faces"];

/// A model a service offers: id as the service names it, and what people see.
#[derive(Clone, Debug)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub detail: String,
}

/// One request: the picture, what to do, and how many.
pub struct Job<'a> {
    pub model: &'a str,
    pub prompt: String,
    /// PNG of the view.
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub count: u32,
}

/// What a service made: encoded pictures (PNG/JPEG/WebP) and a note (cost, seed).
pub struct Made {
    pub pictures: Vec<Vec<u8>>,
    pub note: String,
}

fn connected(service: &str) -> bool {
    match service {
        "openrouter" => host::secret("openrouter_key").is_some(),
        "openai" => host::secret("openai_key").is_some(),
        "gemini" => host::secret("gemini_key").is_some(),
        "comfy" => settings().bool("comfy_on"),
        _ => false,
    }
}

/// The settings panel's values (the host keeps each panel's values as `panel.<id>`).
pub fn settings() -> Values {
    Values::parse(&host::setting("panel.settings").unwrap_or_default())
}

fn models(service: &str) -> Vec<Model> {
    let cached: Vec<Model> = host::setting(&format!("models.{service}"))
        .and_then(|t| serde_json::from_str::<Vec<Value>>(&t).ok())
        .unwrap_or_default()
        .iter()
        .filter_map(|m| {
            Some(Model {
                id: m.get("id")?.as_str()?.to_string(),
                name: m.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                detail: m.get("detail").and_then(Value::as_str).unwrap_or("").to_string(),
            })
        })
        .collect();
    if !cached.is_empty() {
        return cached;
    }
    match service {
        "openrouter" => openrouter::defaults(),
        "openai" => openai::defaults(),
        "gemini" => gemini::defaults(),
        "comfy" => comfy::defaults(),
        _ => Vec::new(),
    }
}

fn save_models(service: &str, list: &[Model]) -> Result<(), String> {
    let v: Vec<Value> = list.iter().map(|m| json!({ "id": m.id, "name": m.name, "detail": m.detail })).collect();
    host::set_setting(&format!("models.{service}"), Some(&Value::from(v).to_string()))
}

/// Every model of every connected service, as menu choices ("service:model").
fn model_choices() -> Vec<Value> {
    let mut out = Vec::new();
    for (sid, sname) in SERVICES {
        if !connected(sid) {
            continue;
        }
        for m in models(sid) {
            let label = if m.name.is_empty() { m.id.clone() } else { m.name.clone() };
            out.push(ui::choice(&format!("{sid}:{}", m.id), &label, sname, &m.detail));
        }
    }
    out
}

fn main_panel(v: &Values) -> Panel {
    let choices = model_choices();
    let default_model = settings().str("default_model");
    let mode = if v.str("mode").is_empty() { MODES[0].to_string() } else { v.str("mode") };
    let mut model_menu = ui::menu("model", "Model", choices.clone(), &default_model, "Connect a service in Settings");
    if v.str("model").is_empty() && !default_model.is_empty() {
        model_menu["value"] = Value::from(default_model);
    }
    let mut p = Panel::new("Generative Render");
    p = p
        .style("sheet")
        .with(ui::stage())
        .with(ui::text_box("prompt", "Describe the scene, the light and the mood…"))
        .with(json!({ "kind": "button", "id": "generate", "label": "Generate", "action": "generate", "primary": true,
                      "capture": true, "enabled": !choices.is_empty() }))
        .with(ui::segmented("mode", &MODES, &mode))
        .with_if(mode == MODES[0], ui::toggle("exact", "Keep My Model Exact", true))
        .with(ui::segmented("count", &["1", "2", "4"], "1"))
        .with(model_menu);
    if choices.is_empty() {
        p = p.with(ui::note(
            "Connect OpenRouter, OpenAI, Google Gemini or a ComfyUI on your computer in Preferences ▸ Plugins ▸ Generative Render.",
            "warning",
        ));
    }
    p.with(ui::results())
}

fn settings_panel(v: &Values) -> Panel {
    let or_in = host::secret("openrouter_key").is_some();
    let mut p = Panel::new("Generative Render")
        .with(ui::note(
            "Pictures are made by the service you connect, on your own account or computer. There are no limits but theirs.",
            "dim",
        ))
        .with(ui::heading("OpenRouter"))
        .with(ui::note(
            "One account for many image models (Gemini, GPT Image, FLUX, Seedream…). You pay OpenRouter for what you use.",
            "dim",
        ));
    p = if or_in {
        p.with(ui::note("Signed in.", "good")).with(ui::button("or-out", "Sign Out", "openrouter_sign_out"))
    } else {
        p.with(ui::button("or-in", "Sign In with OpenRouter", "openrouter_sign_in"))
    };
    p = p
        .with(ui::link("OpenRouter pricing", "https://openrouter.ai/models?output_modalities=image"))
        .with(ui::separator())
        .with(ui::heading("OpenAI"))
        .with(ui::secret("openai_key", "API Key", "sk-…"))
        .with(ui::link("Get an OpenAI API key", "https://platform.openai.com/api-keys"))
        .with(ui::separator())
        .with(ui::heading("Google Gemini"))
        .with(ui::secret("gemini_key", "API Key", "AIza…"))
        .with(ui::note("Gemini's image models need billing turned on for the key (they have no free tier).", "dim"))
        .with(ui::link("Get a Gemini API key", "https://aistudio.google.com/apikey"))
        .with(ui::separator())
        .with(ui::heading("On This Computer"))
        .with(ui::toggle("comfy_on", "Use ComfyUI", false));
    if v.bool("comfy_on") {
        p = p
            .with(json!({ "kind": "text", "id": "comfy_url", "label": "Address", "placeholder": comfy::DEFAULT_URL }))
            .with(ui::button("comfy-test", "Test Connection", "comfy_test"))
            .with(ui::note("Built in: FLUX.1 Kontext [dev]. Or paste your own workflow (API format) with {{image}}, {{prompt}} and {{seed}} where they go.", "dim"))
            .with(json!({ "kind": "text", "id": "comfy_workflow", "label": "Own Workflow", "multiline": true,
                          "placeholder": "Optional: workflow JSON (Export (API) in ComfyUI)" }))
            .with(ui::link("Setting up FLUX.1 Kontext in ComfyUI", "https://docs.comfy.org/tutorials/flux/flux-1-kontext-dev"));
    }
    let choices = model_choices();
    p.with(ui::separator())
        .with(ui::menu("default_model", "Default Model", choices, &v.str("default_model"), "Connect a service first"))
        .with(ui::button("refresh", "Refresh Model Lists", "refresh_models"))
}

impl Guest for GenerativeRender {
    fn start() -> Result<(), String> {
        Ok(())
    }

    fn panel(id: String, values: String) -> Result<String, String> {
        let v = Values::parse(&values);
        match id.as_str() {
            "main" => Ok(main_panel(&v).json()),
            "settings" => Ok(settings_panel(&v).json()),
            other => Err(format!("no panel \"{other}\"")),
        }
    }

    fn run(action: String, values: String) -> Result<String, String> {
        let v = Values::parse(&values);
        match action.as_str() {
            "generate" => generate(&v),
            "openrouter_sign_in" => {
                openrouter::sign_in()?;
                let _ = refresh("openrouter");
                shapecraft_plugin::message("Signed in to OpenRouter.")
            }
            "openrouter_sign_out" => {
                host::set_secret("openrouter_key", None)?;
                shapecraft_plugin::message("Signed out. The key is removed from this computer (revoke it at openrouter.ai/settings/keys).")
            }
            "comfy_test" => comfy::test(&v),
            "refresh_models" => {
                let mut lines = Vec::new();
                for (sid, sname) in SERVICES {
                    if !connected(sid) {
                        continue;
                    }
                    match refresh(sid) {
                        Ok(n) => lines.push(format!("{sname}: {n} models")),
                        Err(e) => lines.push(format!("{sname}: {e}")),
                    }
                }
                if lines.is_empty() {
                    return Err("Nothing is connected yet.".into());
                }
                shapecraft_plugin::message(&lines.join(" · "))
            }
            other => Err(format!("no action \"{other}\"")),
        }
    }
}

fn refresh(service: &str) -> Result<usize, String> {
    let list = match service {
        "openrouter" => openrouter::list()?,
        "openai" => openai::list()?,
        "gemini" => gemini::list()?,
        "comfy" => comfy::defaults(),
        _ => Vec::new(),
    };
    save_models(service, &list)?;
    Ok(list.len())
}

/// How the person's words become the instruction for each mode.
fn instruction(mode: &str, prompt: &str) -> String {
    let p = prompt.trim();
    match mode {
        "Surroundings" => {
            let scene = if p.is_empty() { "a clean, softly lit product photo setting" } else { p };
            format!(
                "Place the object from this picture in a new scene: {scene}. Keep the object exactly as it is: the same \
                 shape, proportions, parts, colors, materials and camera angle; don't add, remove or reshape anything \
                 on it. Replace only the background and surroundings, and make the lighting, reflections and shadows \
                 fit the new scene. Photorealistic."
            )
        }
        "Selected Faces" => format!(
            "Change only the highlighted part of the object: {p}. Keep everything else, the camera angle and the \
             composition exactly as they are."
        ),
        _ => {
            if p.is_empty() {
                "Make this a photorealistic product photo, keeping the object's shape and the camera angle.".to_string()
            } else {
                format!("{p}. Keep the camera angle and composition.")
            }
        }
    }
}

/// The view scaled so its longest side is at most `max` (providers resize anyway; smaller uploads are faster).
fn scaled(view: &host::Image, max: u32) -> Result<host::Image, String> {
    let long = view.width.max(view.height);
    if long <= max {
        return Ok(view.clone());
    }
    let k = max as f64 / long as f64;
    let (w, h) = (((view.width as f64 * k).round() as u32).max(16), ((view.height as f64 * k).round() as u32).max(16));
    host::fit(view, w, h)
}

fn generate(v: &Values) -> Result<String, String> {
    let cap = host::capture().ok_or("No picture of the view came with this request.")?;
    let pick = v.str("model");
    let pick = if pick.is_empty() { settings().str("default_model") } else { pick };
    let (service, model) = pick.split_once(':').ok_or("Pick a model first.")?;
    if !connected(service) {
        return Err("That service isn't connected any more: reconnect it in Preferences ▸ Plugins.".into());
    }
    let mode = if v.str("mode").is_empty() { MODES[0].to_string() } else { v.str("mode") };
    if mode == "Selected Faces" && cap.selection.is_none() {
        return Err("Pick the faces to change first, then Generate.".into());
    }
    let count = v.str("count").parse::<u32>().unwrap_or(1).clamp(1, 4);
    let exact = mode == "Surroundings" && v.get("exact").and_then(Value::as_bool).unwrap_or(true);
    let prompt = v.str("prompt");

    host::progress(-1.0, "Generating scene…");
    // For Selected Faces the picked faces are tinted so the AI can see which part is meant.
    let sent = match (&mode[..], &cap.selection) {
        ("Selected Faces", Some(sel)) => highlight(&cap.view, sel)?,
        _ => cap.view.clone(),
    };
    let small = scaled(&sent, 1536)?;
    let png = host::encode_png(&small)?;
    let job = Job { model, prompt: instruction(&mode, &prompt), png, width: small.width, height: small.height, count };
    let made = match service {
        "openrouter" => openrouter::generate(&job)?,
        "openai" => openai::generate(&job)?,
        "gemini" => gemini::generate(&job)?,
        "comfy" => comfy::generate(&job)?,
        other => return Err(format!("unknown service \"{other}\"")),
    };
    if made.pictures.is_empty() {
        return Err("The service answered without a picture (it may have declined the prompt). Try different words.".into());
    }
    let service_name = SERVICES.iter().find(|(s, _)| *s == service).map(|(_, n)| *n).unwrap_or(service);
    let mut kept = 0;
    for bytes in &made.pictures {
        let pic = host::decode_image(bytes)?;
        let out = match (&mode[..], &cap.selection) {
            // The model's own pixels over the new scene, at the view's size.
            _ if exact => host::composite(&pic, &cap.view, &cap.model)?,
            // Only the picked faces change.
            ("Selected Faces", Some(sel)) => {
                let fitted = host::fit(&pic, cap.view.width, cap.view.height)?;
                host::composite(&cap.view, &fitted, sel)?
            }
            _ => pic,
        };
        let title = if prompt.trim().is_empty() { mode.clone() } else { prompt.trim().chars().take(80).collect() };
        let mut details = format!("{mode} · {service_name} · {model}");
        if exact {
            details.push_str(" · model kept exact");
        }
        if !made.note.is_empty() {
            details.push_str(&format!(" · {}", made.note));
        }
        host::add_result(&out, &title, &details);
        kept += 1;
    }
    shapecraft_plugin::message(&if kept == 1 { "Done.".to_string() } else { format!("{kept} pictures.") })
}

/// The view with the picked faces tinted orange (what Selected Faces sends).
fn highlight(view: &host::Image, sel: &host::Mask) -> Result<host::Image, String> {
    let tint =
        host::Image { width: view.width, height: view.height, pixels: [255u8, 120, 0, 255].repeat((view.width * view.height) as usize) };
    let half = host::Mask { width: sel.width, height: sel.height, values: sel.values.iter().map(|v| v / 2).collect() };
    host::composite(view, &tint, &half)
}

/// A readable error from a service's HTTP answer.
pub fn http_error(service: &str, r: &host::Response) -> String {
    let body = String::from_utf8_lossy(&r.body);
    let said = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|j| {
            j.pointer("/error/message")
                .or_else(|| j.pointer("/error"))
                .or_else(|| j.pointer("/message"))
                .and_then(|m| m.as_str().map(str::to_string))
        })
        .unwrap_or_else(|| body.chars().take(200).collect());
    match r.status {
        401 | 403 => format!("{service} didn't accept the key or sign-in. Check it in Preferences ▸ Plugins. ({said})"),
        402 => format!("{service} says the account is out of credit. ({said})"),
        429 => format!("{service} is busy or the account's rate limit was reached; try again in a moment. ({said})"),
        s => format!("{service} answered {s}: {said}"),
    }
}

shapecraft_plugin::export!(GenerativeRender with_types_in shapecraft_plugin::bindings);
