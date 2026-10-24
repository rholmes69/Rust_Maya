//! Model providers: local **Ollama** and cloud **OpenRouter** (OpenAI-compatible).
//! All functions are blocking and run on the AI worker thread.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};

pub const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Provider {
    Ollama,
    OpenRouter,
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Provider::Ollama => "Ollama",
            Provider::OpenRouter => "OpenRouter",
        }
    }
}

/// A model on a specific provider, e.g. Ollama `tev1:latest` or OpenRouter `qwen/qwen3-32b`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider: Provider,
    pub model: String,
}

impl ModelRef {
    pub fn new(provider: Provider, model: impl Into<String>) -> Self {
        ModelRef { provider, model: model.into() }
    }
    pub fn label(&self) -> String {
        format!("{} · {}", self.provider.label(), self.model)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    pub tools: bool,
    pub free: bool,
    pub context: u64,
    /// USD per million prompt tokens (OpenRouter only).
    pub price_in: f64,
    /// Accepts image input.
    pub vision: bool,
}

/// Heuristic for Ollama model names that accept images.
pub fn is_vision_name(model: &str) -> bool {
    let m = model.to_lowercase();
    ["llava", "vision", "-vl", "vl:", "qwen2.5vl", "qwen3-vl", "gemma3", "minicpm-v", "moondream", "bakllava", "llama4", "mistral-small3"].iter().any(|k| m.contains(k))
}

/// Attach a PNG to a user message in the provider's format.
pub fn user_message_with_image(provider: Provider, text: &str, png_b64: &str) -> Value {
    match provider {
        Provider::Ollama => json!({"role": "user", "content": text, "images": [png_b64]}),
        Provider::OpenRouter => json!({"role": "user", "content": [
            {"type": "text", "text": text},
            {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{png_b64}")}}
        ]}),
    }
}

/// Connection details the worker needs.
#[derive(Clone, Debug)]
pub struct Endpoints {
    pub ollama_host: String,
    pub openrouter_key: String,
    /// Overridable for tests / self-hosted gateways.
    pub openrouter_url: String,
}

pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(4)))
        .timeout_per_read(Some(std::time::Duration::from_secs(300)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn or_key(ep: &Endpoints) -> Result<String, String> {
    if !ep.openrouter_key.trim().is_empty() {
        return Ok(ep.openrouter_key.trim().to_string());
    }
    std::env::var("OPENROUTER_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .ok_or_else(|| "No OpenRouter API key. Add one in AI Settings or set the OPENROUTER_API_KEY environment variable.".to_string())
}

fn or_post(agent: &ureq::Agent, ep: &Endpoints, path: &str, body: &Value) -> Result<ureq::http::Response<ureq::Body>, String> {
    let key = or_key(ep)?;
    agent
        .post(&format!("{}{path}", ep.openrouter_url))
        .header("Authorization", &format!("Bearer {key}"))
        .header("HTTP-Referer", "https://github.com/rholmes69/Rust_Maya")
        .header("X-Title", "Rust Maya")
        .send_json(body)
        .map_err(|e| format!("OpenRouter request failed: {e}"))
}

fn error_text(v: &Value) -> Option<String> {
    match &v["error"] {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        e => Some(e["message"].as_str().map(String::from).unwrap_or_else(|| e.to_string())),
    }
}

// --------------------------------------------------------------------------------- listing

pub fn list_ollama(agent: &ureq::Agent, host: &str) -> Result<Vec<ModelInfo>, String> {
    let mut resp = agent
        .get(&format!("{host}/api/tags"))
        .call()
        .map_err(|e| format!("Ollama not reachable at {host} ({e}). Start the Ollama app or run `ollama serve`."))?;
    let v: Value = resp.body_mut().read_json().map_err(|e| e.to_string())?;
    Ok(v["models"]
        .as_array()
        .map(|a| a.iter().filter_map(|m| m["name"].as_str()).map(|n| ModelInfo { id: n.to_string(), free: true, vision: is_vision_name(n), ..Default::default() }).collect())
        .unwrap_or_default())
}

pub fn ollama_capabilities(agent: &ureq::Agent, host: &str, model: &str) -> Vec<String> {
    let Ok(mut resp) = agent.post(&format!("{host}/api/show")).send_json(json!({"model": model})) else { return vec![] };
    let v: Value = resp.body_mut().read_json().unwrap_or_default();
    v["capabilities"].as_array().map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect()).unwrap_or_default()
}

pub fn list_openrouter(agent: &ureq::Agent, ep: &Endpoints) -> Result<Vec<ModelInfo>, String> {
    let mut resp = agent
        .get(&format!("{}/models", ep.openrouter_url))
        .call()
        .map_err(|e| format!("OpenRouter not reachable ({e})"))?;
    let v: Value = resp.body_mut().read_json().map_err(|e| e.to_string())?;
    let mut out: Vec<ModelInfo> = v["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    let id = m["id"].as_str()?.to_string();
                    let params = m["supported_parameters"].as_array();
                    let tools = params.is_some_and(|p| p.iter().any(|x| x.as_str() == Some("tools")));
                    let price = |k: &str| m["pricing"][k].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                    let price_in = price("prompt") * 1e6;
                    let free = id.ends_with(":free") || (price("prompt") == 0.0 && price("completion") == 0.0);
                    let vision = m["architecture"]["input_modalities"].as_array().is_some_and(|a| a.iter().any(|x| x.as_str() == Some("image")))
                        || m["architecture"]["modality"].as_str().is_some_and(|x| x.contains("image"));
                    Some(ModelInfo { id, tools, free, context: m["context_length"].as_u64().unwrap_or(0), price_in, vision })
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Fraction of a loaded Ollama model resident in VRAM (`/api/ps`).
pub fn ollama_vram(agent: &ureq::Agent, host: &str, model: &str) -> Option<f32> {
    let mut resp = agent.get(&format!("{host}/api/ps")).call().ok()?;
    let v: Value = resp.body_mut().read_json().ok()?;
    let m = v["models"].as_array()?.iter().find(|m| m["name"].as_str() == Some(model) || m["model"].as_str() == Some(model))?;
    let size = m["size"].as_f64()?;
    let vram = m["size_vram"].as_f64().unwrap_or(0.0);
    (size > 0.0).then(|| (vram / size) as f32)
}

pub fn pull_ollama(agent: &ureq::Agent, host: &str, model: &str, progress: &mut dyn FnMut(String, f32)) -> Result<(), String> {
    let resp = agent
        .post(&format!("{host}/api/pull"))
        .send_json(json!({"model": model, "stream": true}))
        .map_err(|e| e.to_string())?;
    let reader = std::io::BufReader::new(resp.into_body().into_reader());
    for line in reader.lines() {
        let line = line.map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&line).unwrap_or_default();
        if let Some(e) = error_text(&v) {
            return Err(e);
        }
        let frac = match (v["completed"].as_f64(), v["total"].as_f64()) {
            (Some(c), Some(t)) if t > 0.0 => (c / t) as f32,
            _ => -1.0,
        };
        progress(v["status"].as_str().unwrap_or("").to_string(), frac);
    }
    Ok(())
}

// ------------------------------------------------------------------------------------ chat

/// Streamed chat completion. `json_schema` asks for structured JSON output.
/// Calls `on_chunk` with each content delta and returns the full text.
pub fn chat_stream(
    agent: &ureq::Agent,
    ep: &Endpoints,
    m: &ModelRef,
    messages: &[Value],
    json_schema: Option<&Value>,
    temperature: f32,
    cancel: &AtomicBool,
    on_chunk: &mut dyn FnMut(&str),
) -> Result<String, String> {
    match m.provider {
        Provider::Ollama => ollama_stream(agent, &ep.ollama_host, &m.model, messages, json_schema, temperature, cancel, on_chunk),
        Provider::OpenRouter => openrouter_stream(agent, ep, &m.model, messages, json_schema.is_some(), temperature, cancel, on_chunk),
    }
}

#[allow(clippy::too_many_arguments)]
fn ollama_stream(
    agent: &ureq::Agent,
    host: &str,
    model: &str,
    messages: &[Value],
    json_schema: Option<&Value>,
    temperature: f32,
    cancel: &AtomicBool,
    on_chunk: &mut dyn FnMut(&str),
) -> Result<String, String> {
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "think": false,
        "keep_alive": "30m",
        "options": {"temperature": temperature, "num_ctx": 8192}
    });
    if let Some(s) = json_schema {
        body["format"] = s.clone();
    }
    for attempt in 0..2 {
        let mut resp = agent.post(&format!("{host}/api/chat")).send_json(&body).map_err(|e| friendly(&format!("Ollama request failed: {e}"), model))?;
        if !resp.status().is_success() {
            let v: Value = resp.body_mut().read_json().unwrap_or_default();
            let err = error_text(&v).unwrap_or_else(|| format!("HTTP {}", resp.status()));
            if attempt == 0 && err.contains("think") {
                body.as_object_mut().unwrap().remove("think");
                continue;
            }
            return Err(format!("Ollama error: {err}"));
        }
        let reader = std::io::BufReader::new(resp.into_body().into_with_config().limit(64 << 20).reader());
        let mut all = String::new();
        for line in reader.lines() {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let line = line.map_err(|e| friendly(&e.to_string(), model))?;
            if line.trim().is_empty() {
                continue;
            }
            let v: Value = serde_json::from_str(&line).map_err(|e| format!("bad stream line: {e}"))?;
            if let Some(e) = error_text(&v) {
                return Err(format!("Ollama error: {e}"));
            }
            let chunk = v["message"]["content"].as_str().unwrap_or("");
            if !chunk.is_empty() {
                all.push_str(chunk);
                on_chunk(chunk);
            }
            if v["done"].as_bool() == Some(true) {
                break;
            }
        }
        return Ok(all);
    }
    Err("Ollama request failed".into())
}

#[allow(clippy::too_many_arguments)]
fn openrouter_stream(
    agent: &ureq::Agent,
    ep: &Endpoints,
    model: &str,
    messages: &[Value],
    json_mode: bool,
    temperature: f32,
    cancel: &AtomicBool,
    on_chunk: &mut dyn FnMut(&str),
) -> Result<String, String> {
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "temperature": temperature,
    });
    if json_mode {
        body["response_format"] = json!({"type": "json_object"});
    }
    for attempt in 0..2 {
        let mut resp = or_post(agent, ep, "/chat/completions", &body)?;
        if !resp.status().is_success() {
            let v: Value = resp.body_mut().read_json().unwrap_or_default();
            let err = error_text(&v).unwrap_or_else(|| format!("HTTP {}", resp.status()));
            // Some models reject response_format: retry without it (prompts already ask for JSON).
            if attempt == 0 && body.get("response_format").is_some() && (err.contains("response_format") || err.contains("json")) {
                body.as_object_mut().unwrap().remove("response_format");
                continue;
            }
            return Err(format!("OpenRouter error ({}): {err}", resp.status()));
        }
        let reader = std::io::BufReader::new(resp.into_body().into_with_config().limit(64 << 20).reader());
        let mut all = String::new();
        for line in reader.lines() {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let line = line.map_err(|e| e.to_string())?;
            let line = line.trim();
            // SSE: "data: {...}", comments start with ':'.
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                break;
            }
            let v: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Some(e) = error_text(&v) {
                return Err(format!("OpenRouter error: {e}"));
            }
            let chunk = v["choices"][0]["delta"]["content"].as_str().unwrap_or("");
            if !chunk.is_empty() {
                all.push_str(chunk);
                on_chunk(chunk);
            }
        }
        return Ok(all);
    }
    Err("OpenRouter request failed".into())
}

/// Non-streamed chat with tool definitions. Returns an Ollama-shaped message:
/// `{"content": "...", "tool_calls": [{"function": {"name": .., "arguments": ..}}]}`.
pub fn chat_tools(
    agent: &ureq::Agent,
    ep: &Endpoints,
    m: &ModelRef,
    messages: &[Value],
    tools: Option<&Value>,
    json_schema: Option<&Value>,
) -> Result<Value, String> {
    match m.provider {
        Provider::Ollama => {
            let mut body = json!({
                "model": m.model,
                "messages": messages,
                "stream": false,
                "keep_alive": "30m",
                "options": {"temperature": 0.0, "num_ctx": 4096}
            });
            if let Some(t) = tools {
                body["tools"] = t.clone();
            }
            if let Some(s) = json_schema {
                body["format"] = s.clone();
            }
            let mut resp = agent
                .post(&format!("{}/api/chat", ep.ollama_host))
                .send_json(&body)
                .map_err(|e| friendly(&format!("decision model request failed: {e}"), &m.model))?;
            let ok = resp.status().is_success();
            let v: Value = resp.body_mut().read_json().map_err(|e| format!("bad response: {e}"))?;
            if !ok || v.get("error").is_some() {
                return Err(format!("Ollama error: {}", error_text(&v).unwrap_or_default()));
            }
            Ok(v["message"].clone())
        }
        Provider::OpenRouter => {
            let mut body = json!({"model": m.model, "messages": messages, "temperature": 0.0});
            if let Some(t) = tools {
                body["tools"] = t.clone();
            } else if json_schema.is_some() {
                body["response_format"] = json!({"type": "json_object"});
            }
            let mut resp = or_post(agent, ep, "/chat/completions", &body)?;
            let ok = resp.status().is_success();
            let v: Value = resp.body_mut().read_json().map_err(|e| format!("bad response: {e}"))?;
            if !ok || v.get("error").is_some() {
                return Err(format!("OpenRouter error: {}", error_text(&v).unwrap_or_default()));
            }
            Ok(v["choices"][0]["message"].clone())
        }
    }
}

pub fn friendly(e: &str, model: &str) -> String {
    let l = e.to_lowercase();
    if l.contains("timed out") || l.contains("timeout") {
        format!("{model} stopped responding. A local model that doesn't fit in VRAM runs very slowly on the CPU — try a smaller model or an OpenRouter model. ({e})")
    } else if l.contains("connection refused") || (l.contains("connect") && l.contains("fail")) || l.contains("os error 10061") {
        format!("Can't reach Ollama — is it running? Start the Ollama app or run `ollama serve`. ({e})")
    } else {
        e.to_string()
    }
}
