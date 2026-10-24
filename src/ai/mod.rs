//! AI layer: providers (Ollama + OpenRouter), the tev1 decision router, the builder model,
//! one-shot assistant tasks and settings. Everything runs on a background worker thread and
//! talks to the UI through channels.

pub mod context;
pub mod parse;
pub mod prompts;
pub mod provider;
pub mod router;

use crossbeam_channel::{Receiver, Sender, unbounded};
use parse::MelStreamParser;
use prompts::Task;
use provider::{Endpoints, ModelInfo, ModelRef, Provider};
use router::Route;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChatMsg {
    pub role: String,
    pub content: String,
}

/// Ollama models ranked for structured output + MEL generation on a workstation GPU.
pub const PREFERRED_OLLAMA: &[&str] = &["qwen3:8b", "qwen3:14b", "qwen3", "qwen3:4b", "qwen2.5-coder", "qwen2.5:7b", "qwen2.5", "llama3.1:8b", "llama3.1", "mistral-nemo", "gemma3", "llama3.2", "phi4", "mistral"];

/// Suggested OpenRouter models (the full live catalogue is browsable in AI Settings).
pub const SUGGESTED_OPENROUTER: &[&str] = &[
    "openai/gpt-4o-mini",
    "google/gemini-2.5-flash",
    "anthropic/claude-sonnet-4",
    "qwen/qwen3-32b",
    "meta-llama/llama-3.3-70b-instruct",
];

pub fn pick_builder(installed: &[String]) -> Option<String> {
    for pref in PREFERRED_OLLAMA {
        if let Some(m) = installed.iter().find(|m| m.as_str() == *pref || m.starts_with(&format!("{pref}:")) || (pref.contains(':') && m.starts_with(pref))) {
            return Some(m.clone());
        }
    }
    installed.iter().find(|m| !m.contains("embed") && !m.contains("whisper") && !m.to_lowercase().starts_with("tev")).cloned()
}

// ------------------------------------------------------------------------------- settings

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSettings {
    pub ollama_host: String,
    /// Stored in the settings file if entered; the OPENROUTER_API_KEY env var is used otherwise.
    pub openrouter_key: String,
    pub openrouter_url: String,
    /// tev1-style decision model. `None` = off.
    pub router: Option<ModelRef>,
    /// Automatically use an installed `tev1` as decision model.
    pub auto_router: bool,
    pub builder: Option<ModelRef>,
    /// Used for one-shot assistant tasks; falls back to the builder.
    pub helper: Option<ModelRef>,
    /// Tried when the builder fails (e.g. Ollama offline -> OpenRouter).
    pub fallback: Option<ModelRef>,
    /// Model that can see images (render check). `None` = automatic.
    pub vision: Option<ModelRef>,
    pub temperature: f32,
    pub whisper_model: String,
    pub hands_free: bool,
    pub vad_threshold: f32,
    pub silence_secs: f32,
    pub speak_replies: bool,
    /// Voice transcripts are sent straight to the AI (otherwise they go into the prompt box).
    pub voice_auto_send: bool,
    pub prompt_history: Vec<String>,
    pub prompt_shelf: Vec<(String, String)>,
}

impl Default for AiSettings {
    fn default() -> Self {
        AiSettings {
            ollama_host: "http://localhost:11434".into(),
            openrouter_key: String::new(),
            openrouter_url: provider::OPENROUTER_URL.into(),
            router: None,
            auto_router: true,
            builder: None,
            helper: None,
            fallback: None,
            vision: None,
            temperature: 0.15,
            whisper_model: "base.en".into(),
            hands_free: true,
            vad_threshold: 0.02,
            silence_secs: 1.2,
            speak_replies: false,
            voice_auto_send: true,
            prompt_history: Vec::new(),
            prompt_shelf: Vec::new(),
        }
    }
}

impl AiSettings {
    pub fn path() -> std::path::PathBuf {
        dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("rust_maya").join("ai_settings.json")
    }
    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save(&self) {
        let p = Self::path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(p, s);
        }
    }
    pub fn endpoints(&self) -> Endpoints {
        Endpoints { ollama_host: self.ollama_host.trim_end_matches('/').to_string(), openrouter_key: self.openrouter_key.clone(), openrouter_url: self.openrouter_url.trim_end_matches('/').to_string() }
    }
    pub fn has_openrouter_key(&self) -> bool {
        !self.openrouter_key.trim().is_empty() || std::env::var("OPENROUTER_API_KEY").is_ok_and(|k| !k.trim().is_empty())
    }
}

// ------------------------------------------------------------------------------ messages

pub enum AiRequest {
    ListModels,
    Pull(String),
    Vram(String),
    Chat { router: Option<ModelRef>, builder: ModelRef, fallback: Option<ModelRef>, history: Vec<ChatMsg>, scene: String, prompt: String, temperature: f32 },
    /// One-shot task. `image` (PNG bytes) is attached for vision models; if the model rejects
    /// images the task is retried as text only.
    Task { task: Task, model: ModelRef, scene: String, prompt: String, image: Option<Vec<u8>> },
}

#[derive(Debug)]
pub enum AiEvent {
    Models { ollama: Result<Vec<ModelInfo>, String>, openrouter: Result<Vec<ModelInfo>, String> },
    PullProgress(String, f32),
    PullDone(Result<String, String>),
    Vram(String, f32),
    /// Decision model chose a route.
    Routed { router: String, route: String, detail: String, secs: f32 },
    RouterFailed(String),
    /// Builder failed; retrying on the fallback model.
    Fallback { from: String, to: String, error: String },
    /// Streaming progress: reply so far + MEL statements completed since the last delta.
    Delta { reply: String, mel: Vec<String> },
    /// End of a chat request. `mel` holds statements not already sent in deltas.
    Reply { reply: String, mel: Vec<String>, model: String, secs: f32 },
    TaskDone { task: Task, reply: String, mel: Vec<String>, model: String, secs: f32 },
    Error(String),
}

pub struct AiClient {
    tx: Sender<AiRequest>,
    pub rx: Receiver<AiEvent>,
    pub cancel: Arc<AtomicBool>,
    endpoints: Arc<std::sync::Mutex<Endpoints>>,
}

impl AiClient {
    pub fn new(settings: &AiSettings, repaint: impl Fn() + Send + 'static) -> Self {
        let (tx, req_rx) = unbounded::<AiRequest>();
        let (ev_tx, rx) = unbounded::<AiEvent>();
        let cancel = Arc::new(AtomicBool::new(false));
        let endpoints = Arc::new(std::sync::Mutex::new(settings.endpoints()));
        let (c2, e2) = (cancel.clone(), endpoints.clone());
        std::thread::Builder::new()
            .name("ai-worker".into())
            .spawn(move || worker(req_rx, ev_tx, c2, e2, repaint))
            .expect("spawn AI worker");
        AiClient { tx, rx, cancel, endpoints }
    }

    pub fn send(&self, r: AiRequest) {
        let _ = self.tx.send(r);
    }

    pub fn set_endpoints(&self, ep: Endpoints) {
        *self.endpoints.lock().unwrap() = ep;
    }

    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn worker(rx: Receiver<AiRequest>, tx: Sender<AiEvent>, cancel: Arc<AtomicBool>, endpoints: Arc<std::sync::Mutex<Endpoints>>, repaint: impl Fn()) {
    let agent = provider::agent();
    let mut tool_caps: HashMap<ModelRef, bool> = HashMap::new();
    let send = |e: AiEvent| {
        let _ = tx.send(e);
        repaint();
    };
    for req in rx {
        let ep = endpoints.lock().unwrap().clone();
        cancel.store(false, Ordering::Relaxed);
        match req {
            AiRequest::ListModels => {
                let ollama = provider::list_ollama(&agent, &ep.ollama_host);
                let openrouter = provider::list_openrouter(&agent, &ep);
                send(AiEvent::Models { ollama, openrouter });
            }
            AiRequest::Pull(model) => {
                let r = provider::pull_ollama(&agent, &ep.ollama_host, &model, &mut |s, f| {
                    let _ = tx.send(AiEvent::PullProgress(s, f));
                    repaint();
                });
                send(AiEvent::PullDone(r.map(|_| model)));
            }
            AiRequest::Vram(model) => {
                if let Some(f) = provider::ollama_vram(&agent, &ep.ollama_host, &model) {
                    send(AiEvent::Vram(model, f));
                }
            }
            AiRequest::Task { task, model, scene, prompt, image } => {
                let t0 = std::time::Instant::now();
                let run = |user: Value| {
                    let messages = vec![json!({"role": "system", "content": task.system(&scene)}), user];
                    provider::chat_stream(&agent, &ep, &model, &messages, Some(&prompts::builder_schema()), 0.2, &cancel, &mut |_| {}).and_then(|raw| parse::parse_reply(&raw))
                };
                let mut note = String::new();
                let r = match &image {
                    Some(png) => {
                        let b64 = crate::recipes::base64(png);
                        match run(provider::user_message_with_image(model.provider, &prompt, &b64)) {
                            Ok(x) => {
                                note = " · saw the image".into();
                                Ok(x)
                            }
                            Err(e) if e != "cancelled" => {
                                note = format!(" · text only (image rejected: {})", e.chars().take(80).collect::<String>());
                                run(json!({"role": "user", "content": prompt}))
                            }
                            Err(e) => Err(e),
                        }
                    }
                    None => run(json!({"role": "user", "content": prompt})),
                };
                match r {
                    Ok((reply, mel)) => send(AiEvent::TaskDone { task, reply, mel, model: format!("{}{note}", model.label()), secs: t0.elapsed().as_secs_f32() }),
                    Err(e) => send(AiEvent::Error(format!("{}: {e}", task.label()))),
                }
            }
            AiRequest::Chat { router, builder, fallback, history, scene, prompt, temperature } => {
                let t0 = std::time::Instant::now();
                let mut prompt = prompt;
                // ---- stage 1: decision model (tev1) ----
                if let Some(rm) = router.filter(|r| *r != builder) {
                    let tool_mode = *tool_caps.entry(rm.clone()).or_insert_with(|| match rm.provider {
                        Provider::Ollama => provider::ollama_capabilities(&agent, &ep.ollama_host, &rm.model).iter().any(|c| c == "tools"),
                        Provider::OpenRouter => true,
                    });
                    let mut decision = router::decide(&agent, &ep, &rm, &history, &scene, &prompt, tool_mode);
                    if decision.is_err() && tool_mode {
                        // Tool calling not supported after all: retry in JSON mode and remember.
                        let r2 = router::decide(&agent, &ep, &rm, &history, &scene, &prompt, false);
                        if r2.is_ok() {
                            tool_caps.insert(rm.clone(), false);
                        }
                        decision = decision.or(r2);
                    }
                    let secs = t0.elapsed().as_secs_f32();
                    match decision {
                        Ok(route) => {
                            let detail = match &route {
                                Route::Delegate { plan } => plan.clone(),
                                _ => String::new(),
                            };
                            send(AiEvent::Routed { router: rm.model.clone(), route: route.name().into(), detail, secs });
                            match route {
                                Route::Direct { reply, mel } => {
                                    send(AiEvent::Reply { reply, mel, model: rm.label(), secs });
                                    continue;
                                }
                                Route::Answer(text) => {
                                    send(AiEvent::Reply { reply: text, mel: vec![], model: rm.label(), secs });
                                    continue;
                                }
                                Route::Delegate { plan } => {
                                    if !plan.trim().is_empty() {
                                        prompt = format!("{prompt}\n\n(Plan from the decision model — follow it if it helps: {plan})");
                                    }
                                }
                            }
                        }
                        Err(e) => send(AiEvent::RouterFailed(format!("{}: {e}", rm.model))),
                    }
                    if cancel.load(Ordering::Relaxed) {
                        send(AiEvent::Error("cancelled".into()));
                        continue;
                    }
                }
                // ---- stage 2: builder (with optional fallback) ----
                let mut targets = vec![builder.clone()];
                if let Some(f) = fallback.filter(|f| *f != builder) {
                    targets.push(f);
                }
                let mut last_err = String::new();
                for (i, m) in targets.iter().enumerate() {
                    if i > 0 {
                        send(AiEvent::Fallback { from: targets[0].label(), to: m.label(), error: last_err.clone() });
                    }
                    match build(&agent, &ep, m, &history, &scene, &prompt, temperature, &cancel, &tx, &repaint) {
                        Ok((reply, mel)) => {
                            send(AiEvent::Reply { reply, mel, model: m.label(), secs: t0.elapsed().as_secs_f32() });
                            last_err.clear();
                            break;
                        }
                        Err(e) => {
                            last_err = e;
                            if last_err == "cancelled" {
                                break;
                            }
                        }
                    }
                }
                if !last_err.is_empty() {
                    send(AiEvent::Error(last_err));
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build(
    agent: &ureq::Agent,
    ep: &Endpoints,
    m: &ModelRef,
    history: &[ChatMsg],
    scene: &str,
    prompt: &str,
    temperature: f32,
    cancel: &AtomicBool,
    tx: &Sender<AiEvent>,
    repaint: &dyn Fn(),
) -> Result<(String, Vec<String>), String> {
    let mut messages = vec![json!({"role": "system", "content": prompts::builder_system(scene)})];
    messages.extend(prompts::builder_examples());
    let mut budget = 6000usize;
    let mut recent = Vec::new();
    for h in history.iter().rev().take(8) {
        let c: String = h.content.chars().take(1500).collect();
        if c.len() > budget {
            break;
        }
        budget -= c.len();
        recent.push(json!({"role": h.role, "content": c}));
    }
    messages.extend(recent.into_iter().rev());
    messages.push(json!({"role": "user", "content": prompt}));
    let mut parser = MelStreamParser::default();
    let mut sent = 0usize;
    let mut last_reply = 0usize;
    let raw = provider::chat_stream(agent, ep, m, &messages, Some(&prompts::builder_schema()), temperature, cancel, &mut |chunk| {
        let new = parser.push(chunk);
        let reply = parser.reply_so_far();
        if !new.is_empty() || reply.len() != last_reply {
            last_reply = reply.len();
            sent += new.iter().map(|s| parse::split_statements(s).len()).sum::<usize>();
            let _ = tx.send(AiEvent::Delta { reply, mel: new });
            repaint();
        }
    })?;
    let (reply, mel) = match parse::parse_reply(&raw) {
        Ok(r) => r,
        Err(_) if sent > 0 => (parser.reply_so_far(), vec![]),
        Err(e) => return Err(e),
    };
    // Only what the streaming parser didn't already deliver.
    Ok((reply, mel.into_iter().skip(sent).collect()))
}

/// Value helper used by the UI for chat transcripts.
pub fn transcript_json(reply: &str, mel: &[String]) -> String {
    json!({"reply": reply, "mel": mel}).to_string()
}

#[cfg(test)]
mod mock_tests {
    //! End-to-end tests against the mock Ollama/OpenRouter server
    //! (`python3 mock_ai.py 8765`, then `MOCK_AI=http://127.0.0.1:8765 cargo test mock -- --ignored`).
    use super::*;

    fn client(builder_ollama_host: &str) -> (AiClient, String) {
        let base = std::env::var("MOCK_AI").unwrap_or_else(|_| "http://127.0.0.1:8765".into());
        let s = AiSettings { ollama_host: builder_ollama_host.replace("{base}", &base), openrouter_key: "sk-or-test".into(), openrouter_url: format!("{base}/api/v1"), ..Default::default() };
        (AiClient::new(&s, || {}), base)
    }

    fn collect(c: &AiClient) -> Vec<AiEvent> {
        let mut out = vec![];
        while let Ok(ev) = c.rx.recv_timeout(std::time::Duration::from_secs(20)) {
            let done = matches!(ev, AiEvent::Reply { .. } | AiEvent::Error(_) | AiEvent::TaskDone { .. } | AiEvent::Models { .. });
            out.push(ev);
            if done {
                break;
            }
        }
        out
    }

    fn all_mel(evs: &[AiEvent]) -> Vec<String> {
        evs.iter()
            .flat_map(|e| match e {
                AiEvent::Delta { mel, .. } | AiEvent::Reply { mel, .. } => mel.clone(),
                _ => vec![],
            })
            .collect()
    }

    fn chat(router: Option<ModelRef>, builder: ModelRef, fallback: Option<ModelRef>, prompt: &str) -> AiRequest {
        AiRequest::Chat { router, builder, fallback, history: vec![], scene: "(empty scene)".into(), prompt: prompt.into(), temperature: 0.1 }
    }

    #[test]
    #[ignore]
    fn mock_tev1_delegates_and_ollama_streams() {
        let (c, _) = client("{base}");
        c.send(chat(Some(ModelRef::new(Provider::Ollama, "tev1:latest")), ModelRef::new(Provider::Ollama, "qwen3:8b"), None, "make a table with four legs"));
        let evs = collect(&c);
        assert!(evs.iter().any(|e| matches!(e, AiEvent::Routed { route, .. } if route == "delegate")), "{evs:?}");
        assert!(evs.iter().filter(|e| matches!(e, AiEvent::Delta { mel, .. } if !mel.is_empty())).count() > 3, "statements should stream");
        assert_eq!(all_mel(&evs).len(), 14, "{:?}", all_mel(&evs));
    }

    #[test]
    #[ignore]
    fn mock_tev1_direct_and_answer() {
        let (c, _) = client("{base}");
        let r = Some(ModelRef::new(Provider::Ollama, "tev1:latest"));
        c.send(chat(r.clone(), ModelRef::new(Provider::Ollama, "qwen3:8b"), None, "add a blue ball"));
        let evs = collect(&c);
        assert_eq!(all_mel(&evs), vec!["polySphere -r 0.5 -n blue_ball", "move 0 3 0", "setColor blue"]);
        c.send(chat(r, ModelRef::new(Provider::Ollama, "qwen3:8b"), None, "how do I move things?"));
        let evs = collect(&c);
        assert!(evs.iter().any(|e| matches!(e, AiEvent::Reply { reply, mel, .. } if reply.contains("Move tool") && mel.is_empty())));
    }

    #[test]
    #[ignore]
    fn mock_openrouter_router_and_builder() {
        let (c, _) = client("{base}");
        let or = |m: &str| ModelRef::new(Provider::OpenRouter, m);
        c.send(chat(Some(or("openai/gpt-4o-mini")), or("anthropic/claude-sonnet-4"), None, "build a table"));
        let evs = collect(&c);
        assert!(evs.iter().any(|e| matches!(e, AiEvent::Routed { route, detail, .. } if route == "delegate" && detail == "cloud plan")), "{evs:?}");
        assert_eq!(all_mel(&evs).len(), 14, "{:?}", all_mel(&evs));
    }

    #[test]
    #[ignore]
    fn mock_fallback_when_ollama_is_down() {
        let (c, _) = client("http://127.0.0.1:9"); // nothing listens here
        c.send(chat(None, ModelRef::new(Provider::Ollama, "qwen3:8b"), Some(ModelRef::new(Provider::OpenRouter, "openai/gpt-4o-mini")), "make a table"));
        let evs = collect(&c);
        assert!(evs.iter().any(|e| matches!(e, AiEvent::Fallback { .. })), "{evs:?}");
        assert!(evs.iter().any(|e| matches!(e, AiEvent::Reply { model, .. } if model.contains("OpenRouter"))));
    }

    #[test]
    #[ignore]
    fn mock_lists_models_and_runs_tasks() {
        let (c, _) = client("{base}");
        c.send(AiRequest::ListModels);
        match collect(&c).pop() {
            Some(AiEvent::Models { ollama, openrouter }) => {
                let o: Vec<String> = ollama.unwrap().into_iter().map(|m| m.id).collect();
                assert_eq!(router::pick_decision_model(&o).as_deref(), Some("tev1:latest"));
                assert_eq!(pick_builder(&o).as_deref(), Some("qwen3:8b"));
                let or = openrouter.unwrap();
                assert!(or.iter().any(|m| m.id.ends_with(":free") && m.free && m.tools));
                assert!(or.iter().any(|m| !m.tools));
            }
            e => panic!("{e:?}"),
        }
        c.send(AiRequest::Task { task: Task::SmartRename, model: ModelRef::new(Provider::Ollama, "qwen3:8b"), scene: String::new(), prompt: "pCube1 ...".into(), image: None });
        match collect(&c).pop() {
            Some(AiEvent::TaskDone { mel, .. }) => assert_eq!(mel, vec!["rename pCube1 crate", "rename pSphere1 red_ball"]),
            e => panic!("{e:?}"),
        }
        c.send(AiRequest::Task { task: Task::FixMel, model: ModelRef::new(Provider::OpenRouter, "openai/gpt-4o-mini"), scene: String::new(), prompt: "polyCub".into(), image: None });
        assert!(matches!(collect(&c).pop(), Some(AiEvent::TaskDone { mel, .. }) if mel == vec!["polyCube".to_string()]));
    }

    #[test]
    #[ignore]
    fn mock_render_check_with_and_without_vision() {
        let (c, _) = client("{base}");
        let png = vec![0x89, b'P', b'N', b'G', 1, 2, 3];
        let task = |m: ModelRef| AiRequest::Task { task: Task::RenderCheck, model: m, scene: String::new(), prompt: "Review this preview render".into(), image: Some(png.clone()) };
        // Vision model on Ollama sees the image.
        c.send(task(ModelRef::new(Provider::Ollama, "gemma3:4b")));
        match collect(&c).pop() {
            Some(AiEvent::TaskDone { reply, mel, model, .. }) => {
                assert!(reply.contains("I can see"), "{reply}");
                assert!(model.contains("saw the image"));
                assert_eq!(mel[0], "lightRig -preset studio");
            }
            e => panic!("{e:?}"),
        }
        // Text model rejects the image: retried as text only.
        c.send(task(ModelRef::new(Provider::Ollama, "qwen3:8b")));
        match collect(&c).pop() {
            Some(AiEvent::TaskDone { reply, model, .. }) => {
                assert!(reply.contains("statistics"), "{reply}");
                assert!(model.contains("text only"), "{model}");
            }
            e => panic!("{e:?}"),
        }
        // OpenRouter multimodal content.
        c.send(task(ModelRef::new(Provider::OpenRouter, "openai/gpt-4o-mini")));
        assert!(matches!(collect(&c).pop(), Some(AiEvent::TaskDone { reply, .. }) if reply.contains("I can see")));
        // Vision flags in the catalogue.
        c.send(AiRequest::ListModels);
        match collect(&c).pop() {
            Some(AiEvent::Models { ollama, openrouter }) => {
                assert!(ollama.unwrap().iter().any(|m| m.id == "gemma3:4b" && m.vision));
                assert!(openrouter.unwrap().iter().any(|m| m.id == "openai/gpt-4o-mini" && m.vision));
            }
            e => panic!("{e:?}"),
        }
    }

    #[test]
    #[ignore]
    fn mock_openrouter_bad_key_is_reported() {
        let base = std::env::var("MOCK_AI").unwrap_or_else(|_| "http://127.0.0.1:8765".into());
        let s = AiSettings { openrouter_key: "wrong".into(), openrouter_url: format!("{base}/api/v1"), ..Default::default() };
        let c = AiClient::new(&s, || {});
        c.send(chat(None, ModelRef::new(Provider::OpenRouter, "openai/gpt-4o-mini"), None, "make a table"));
        assert!(matches!(collect(&c).pop(), Some(AiEvent::Error(e)) if e.contains("No auth credentials")));
    }
}
