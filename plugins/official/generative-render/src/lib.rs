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
    [("openrouter", "OpenRouter"), ("openai", "OpenAI"), ("gemini", "Google Gemini"), ("comfy", "ComfyUI on This Computer")];

/// The modes offered. "Selected Faces" is handled too, for when the app sends a selection mask (not yet).
pub const MODES: [&str; 2] = ["Surroundings", "Whole Picture"];

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
    /// With Keep My Model Exact: a PNG the view's size, see-through where the scene may change and solid on the model
    /// (OpenAI's edit mask). Services without masks get the instruction alone.
    pub mask_png: Option<Vec<u8>>,
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
    let exact = v.get("exact").and_then(Value::as_bool).unwrap_or(true);
    let mut model_menu = ui::menu("model", "Model", choices.clone(), &default_model, "Connect a service first");
    if v.str("model").is_empty() && !default_model.is_empty() {
        model_menu["value"] = Value::from(default_model.clone());
    }
    let mut count = ui::segmented("count", &["1", "2", "4"], "1");
    let go = json!({ "kind": "button", "id": "generate", "label": "Generate", "action": "generate", "primary": true,
                     "capture": true, "enabled": !choices.is_empty() });
    let go = match spend(v, &choices, &default_model) {
        Some((with, what, estimate)) => ui::spends(go, &with, &what, &estimate, v.str("prompt").trim()),
        None => go,
    };
    count["label"] = Value::from("Pictures");
    let count = ui::icons(count, &["pictures-1", "pictures-2", "pictures-4"]);
    let free = v.str("source") == SOURCES[1];
    // The sheet: the switch between the person's own AI and a free chat at the top, the picture, the prompt bar,
    // then a row of how (what changes, the model kept exact or free to move) and a row of who: the model and how
    // many, or which free chat. A separator starts a new row; an empty model list makes the app offer "Connect…".
    let p = Panel::new("Generative Render")
        .style("sheet")
        .with(ui::in_header(ui::icons(ui::segmented("source", &SOURCES, SOURCES[0]), &["service", "chat"])))
        .with(ui::stage())
        .with(ui::text_box("prompt", "Describe the scene, the light and the mood…"));
    let p = if free {
        let chat = chat_of(v);
        let (_, name, _) = CHATS.iter().find(|(id, ..)| *id == chat).copied().unwrap_or(CHATS[0]);
        p.with(json!({ "kind": "button", "id": "open-chat", "label": format!("Open {name}"), "action": format!("handoff:{chat}"),
                        "primary": true }))
    } else {
        p.with(go)
    };
    let p = p
        .with(ui::heading("What Changes"))
        .with(ui::icons(ui::segmented("mode", &MODES, &mode), &["surroundings", "whole-picture"]))
        .with(ui::heading("Your Model"))
        .with_if(mode != MODES[0], ui::note("Whole Picture restyles your model along with the scene.", "dim"))
        .with_if(mode == MODES[0], ui::icons(ui::toggle("exact", "Keep It Exact", true), &["keep-exact"]))
        .with_if(mode == MODES[0] && !exact, ui::icons(ui::toggle("move", "Let It Move", false), &["move-resize"]))
        .with(ui::separator());
    if free {
        // No account at all: the app copies the view and opens the chat; the words follow on a card.
        let names: Vec<&str> = CHATS.iter().map(|(_, n, _)| *n).collect();
        let chat = CHATS.iter().find(|(id, ..)| *id == chat_of(v)).map_or(CHATS[0].1, |c| c.1);
        let mut pick = ui::icons(ui::segmented("chat", &names, chat), &["chat"; 5]);
        pick["label"] = Value::from("Chat");
        p.with(pick)
            .with(ui::note(
                "Free, with that site's own limits. Your view goes on the clipboard and the site opens; paste the picture, then the words.",
                "dim",
            ))
            .with(ui::results())
    } else {
        p.with(model_menu).with(count).with(ui::results())
    }
}

/// The two ways of working: the person's own AI service (or ComfyUI), or a free chat site.
pub const SOURCES: [&str; 2] = ["Your AI Service", "Free Chat"];

/// The free chat picked (its id), from its name in the "chat" tiles.
fn chat_of(v: &Values) -> &'static str {
    let name = v.str("chat");
    CHATS.iter().find(|(_, n, _)| *n == name).map_or(CHATS[0].0, |c| c.0)
}

/// What Generate would cost with the picked model: whose account, what it makes, the price when the service lists
/// one. None for ComfyUI on the person's own computer (free).
fn spend(v: &Values, choices: &[Value], default_model: &str) -> Option<(String, String, String)> {
    let pick = v.str("model");
    let pick = if !pick.is_empty() {
        pick
    } else if !default_model.is_empty() {
        default_model.to_string()
    } else {
        choices.first()?.get("value")?.as_str()?.to_string()
    };
    let (service, _) = pick.split_once(':')?;
    if service == "comfy" {
        return None;
    }
    let choice = choices.iter().find(|c| c.get("value").and_then(Value::as_str) == Some(pick.as_str()));
    let field = |k: &str| choice.and_then(|c| c.get(k)).and_then(Value::as_str).unwrap_or("").to_string();
    let n = v.str("count").parse::<u32>().unwrap_or(1).clamp(1, 4);
    let model = field("label");
    let what =
        format!("{n} {} with {}", if n == 1 { "picture" } else { "pictures" }, if model.is_empty() { "the picked model" } else { &model });
    let with = SERVICES.iter().find(|(s, _)| *s == service).map(|(_, n)| n.to_string()).unwrap_or_else(|| service.to_string());
    let price = field("detail");
    let estimate = if price.starts_with('$') { format!("{price} a picture on {with}'s list") } else { String::new() };
    Some((with, what, estimate))
}

/// The free chats people can take the view to: name and address.
const CHATS: [(&str, &str, &str); 5] = [
    ("chatgpt", "ChatGPT", "https://chatgpt.com/"),
    ("gemini", "Gemini", "https://gemini.google.com/app"),
    ("copilot", "Copilot", "https://copilot.microsoft.com/"),
    ("grok", "Grok", "https://grok.com/"),
    ("meta", "Meta AI", "https://www.meta.ai/"),
];

/// Sends the person to a free chat with the words ready: the app puts the view on the clipboard, opens the site, and
/// offers the description to paste after the picture (addresses that pre-fill a prompt can send it before the
/// picture is attached, so none is used).
fn handoff(site: &str, v: &Values) -> Result<String, String> {
    let (_, name, url) = CHATS.iter().find(|(id, ..)| *id == site).ok_or("unknown chat")?;
    let mode = if v.str("mode").is_empty() { MODES[0].to_string() } else { v.str("mode") };
    let exact = mode == "Surroundings" && v.get("exact").and_then(Value::as_bool).unwrap_or(true);
    let free = mode == "Surroundings" && !exact && v.bool("move");
    let text = format!("Make an image from the attached picture. {}", instruction(&mode, &v.str("prompt"), exact, free));
    Ok(json!({ "handoff": { "site": name, "url": url, "text": text } }).to_string())
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
            .with(ui::note("Leave the address empty to find ComfyUI on this computer by itself (ports 8188, 8000 and 8189). For another machine, type its address.", "dim"))
            .with(json!({ "kind": "text", "id": "comfy_url", "label": "Address", "placeholder": "Found by itself, or like 192.168.1.20:8188" }))
            .with(ui::button("comfy-find", "Find ComfyUI", "comfy_find"))
            .with(ui::button("comfy-test", "Test Connection", "comfy_test"))
            .with(ui::note("Built in: FLUX.2 klein (4B runs on a laptop) and FLUX.1 Kontext [dev]. Or paste your own workflow (API format) with {{image}}, {{prompt}} and {{seed}} where they go.", "dim"))
            .with(ui::link("Setting up FLUX.2 klein in ComfyUI", "https://huggingface.co/Comfy-Org/Flux-2-Klein-4B"))
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
            "comfy_find" => comfy::find(&v),
            a if a.starts_with("handoff:") => handoff(&a["handoff:".len()..], &v),
            // The app runs this after a key is saved: the new service's models are ready right away.
            "secret-saved" => {
                let id = v.str("id");
                let service = id.trim_end_matches("_key");
                match refresh(service) {
                    Ok(n) => shapecraft_plugin::message(&format!("Saved. {n} models are ready.")),
                    Err(e) => Err(format!("Saved, but the model list couldn't be loaded: {e}")),
                }
            }
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
        "comfy" => comfy::list()?,
        _ => Vec::new(),
    };
    save_models(service, &list)?;
    Ok(list.len())
}

/// How the person's words become the instruction for each mode. `exact`: the model's pixels go back on top (so it
/// mustn't move); `free`: it may be moved, resized and tilted to suit the scene (a coin floating in the air).
fn instruction(mode: &str, prompt: &str, exact: bool, free: bool) -> String {
    let p = prompt.trim();
    match mode {
        "Surroundings" => {
            let scene = if p.is_empty() { "a clean, softly lit product photo setting" } else { p };
            let keep = "Keep the object itself exactly as it is: the same shape, proportions, parts, colors, materials and \
                        details; don't add, remove or reshape anything on it, and don't add another copy of it anywhere.";
            let place = if exact {
                "It must stay at exactly the same position, size and angle in the frame: don't move, resize, crop or \
                 redraw it, and keep the picture's framing. Replace only the background and surroundings around it, and \
                 make the shadows it casts fit the new scene."
            } else if free {
                "You may move it, make it bigger or smaller, tilt it, or have it float in the air, wherever it looks best \
                 in the scene, and reframe the picture around it. Light it like the scene."
            } else {
                "Keep it where it is and at the same size and angle, but light it like the scene: its highlights, \
                 reflections and shadows should belong there."
            };
            format!("Place the object from this picture in a new scene: {scene}. {keep} {place} Photorealistic.")
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
    let free = mode == "Surroundings" && !exact && v.bool("move");
    let prompt = v.str("prompt");

    host::progress(-1.0, "Generating scene…");
    // For Selected Faces the picked faces are tinted so the AI can see which part is meant.
    let sent = match (&mode[..], &cap.selection) {
        ("Selected Faces", Some(sel)) => highlight(&cap.view, sel)?,
        _ => cap.view.clone(),
    };
    let small = scaled(&sent, 1536)?;
    let png = host::encode_png(&small)?;
    let mask_png = if exact {
        // The model mask as alpha (solid = keep), scaled like the picture.
        let alpha: Vec<u8> = cap.model.values.iter().flat_map(|a| [0, 0, 0, *a]).collect();
        let m = host::Image { width: cap.model.width, height: cap.model.height, pixels: alpha };
        Some(host::encode_png(&host::fit(&m, small.width, small.height)?)?)
    } else {
        None
    };
    let job =
        Job { model, prompt: instruction(&mode, &prompt, exact, free), png, width: small.width, height: small.height, count, mask_png };
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
    let service_name = match service {
        "comfy" => "ComfyUI",
        "gemini" => "Gemini",
        other => SERVICES.iter().find(|(s, _)| *s == other).map(|(_, n)| *n).unwrap_or(other),
    };
    let model_name =
        models(service).into_iter().find(|m| m.id == model).map(|m| m.name).filter(|n| !n.is_empty()).unwrap_or_else(|| model.to_string());
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
        // The whole prompt as the title (History shows it), and short facts as details: the app shows them as pills,
        // the first one on thumbnails.
        let title = if prompt.trim().is_empty() { "(no description)".to_string() } else { prompt.trim().to_string() };
        let how = match (&mode[..], exact, free) {
            ("Surroundings", true, _) => "Model exact",
            ("Surroundings", false, true) => "Moved freely",
            ("Surroundings", false, false) => "Relit",
            _ => "Whole picture",
        };
        let mut details = format!("{service_name} · {model_name} · {how}");
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
    let low = said.to_lowercase();
    let no_credit =
        ["no credits", "insufficient_quota", "exceeded your current quota", "billing", "insufficient credits", "payment required"]
            .iter()
            .any(|k| low.contains(k));
    if no_credit || r.status == 402 {
        let page = match service {
            "OpenAI" => " Add some at platform.openai.com ▸ Settings ▸ Billing.",
            "Google Gemini" => " Turn on billing for the key's project at aistudio.google.com.",
            "OpenRouter" => " Add some at openrouter.ai ▸ Credits.",
            _ => "",
        };
        return format!(
            "{service} says this account has no credit for pictures. API use is paid separately from chat subscriptions \
             like ChatGPT Plus or Gemini Advanced.{page}"
        );
    }
    match r.status {
        401 | 403 => format!("{service} didn't accept the key or sign-in. Check it in Preferences ▸ Plugins. ({said})"),
        429 => format!("{service} is busy or the account's rate limit was reached; try again in a moment. ({said})"),
        s => format!("{service} answered {s}: {said}"),
    }
}

shapecraft_plugin::export!(GenerativeRender with_types_in shapecraft_plugin::bindings);
