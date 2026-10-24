//! Two-model pipeline (after Rust_Blender's router): a small, fast **decision model** such as
//! `tev1` sees every request first and
//!   * runs simple edits itself (`run_mel`),
//!   * answers questions (`answer`), or
//!   * hands creative / multi-step builds to the larger builder model with a plan
//!     (`delegate_to_builder`).
//! Uses native tool calling when available (Ollama `tools` capability or any OpenRouter model
//! listing "tools"), otherwise a JSON response with the same three routes.

use super::ChatMsg;
use super::parse::{split_statements, strip_think};
use super::prompts;
use super::provider::{self, Endpoints, ModelRef};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub enum Route {
    Direct { reply: String, mel: Vec<String> },
    Delegate { plan: String },
    Answer(String),
}

impl Route {
    pub fn name(&self) -> &'static str {
        match self {
            Route::Direct { .. } => "direct",
            Route::Delegate { .. } => "delegate",
            Route::Answer(_) => "answer",
        }
    }
}

/// Pick an installed decision model: anything named tev1 / tev.
pub fn pick_decision_model(installed: &[String]) -> Option<String> {
    for pref in ["tev1", "tev"] {
        if let Some(m) = installed.iter().find(|m| {
            let l = m.to_lowercase();
            l.starts_with(pref) || l.contains(&format!("/{pref}"))
        }) {
            return Some(m.clone());
        }
    }
    None
}

pub fn decide(
    agent: &ureq::Agent,
    ep: &Endpoints,
    m: &ModelRef,
    history: &[ChatMsg],
    scene: &str,
    prompt: &str,
    tool_mode: bool,
) -> Result<Route, String> {
    let mut messages = vec![json!({"role": "system", "content": prompts::decision_system(scene, tool_mode)})];
    for h in history.iter().rev().take(4).rev() {
        let c: String = h.content.chars().take(600).collect();
        messages.push(json!({"role": h.role, "content": c}));
    }
    messages.push(json!({"role": "user", "content": prompt}));
    let msg = if tool_mode {
        provider::chat_tools(agent, ep, m, &messages, Some(&prompts::decision_tools()), None)?
    } else {
        provider::chat_tools(agent, ep, m, &messages, None, Some(&prompts::decision_schema()))?
    };
    parse_decision(&msg)
}

fn args_of(f: &Value) -> Value {
    match &f["arguments"] {
        Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
        v => v.clone(),
    }
}

pub fn parse_decision(msg: &Value) -> Result<Route, String> {
    if let Some(calls) = msg["tool_calls"].as_array().filter(|c| !c.is_empty()) {
        let mut mel = Vec::new();
        let mut reply = String::new();
        let mut answer = None;
        for call in calls {
            let f = &call["function"];
            let args = args_of(f);
            match f["name"].as_str().unwrap_or("") {
                "delegate_to_builder" => return Ok(Route::Delegate { plan: args["plan"].as_str().unwrap_or("").into() }),
                "answer" => answer = Some(args["text"].as_str().unwrap_or("").to_string()),
                "run_mel" => {
                    let script = match &args["script"] {
                        Value::Array(a) => a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(";"),
                        v => v.as_str().unwrap_or("").to_string(),
                    };
                    mel.extend(split_statements(&script));
                    if let Some(r) = args["reply"].as_str() {
                        reply = r.to_string();
                    }
                }
                // Some small models call a MEL command name directly as a tool.
                name if !name.is_empty() => {
                    let rest: Vec<String> = args.as_object().map(|o| o.values().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
                    mel.push(format!("{name} {}", rest.join(" ")).trim().to_string());
                }
                _ => {}
            }
        }
        if !mel.is_empty() {
            if reply.is_empty() {
                reply = msg["content"].as_str().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("Done.").to_string();
            }
            return Ok(Route::Direct { reply, mel });
        }
        if let Some(a) = answer {
            return Ok(Route::Answer(a));
        }
        return Err("decision model made no usable tool call".into());
    }
    let content = strip_think(msg["content"].as_str().unwrap_or("")).trim();
    let start = content.find('{').ok_or_else(|| format!("decision model gave no decision: {content}"))?;
    let v: Value = serde_json::Deserializer::from_str(&content[start..]).into_iter::<Value>().next().ok_or("empty decision")?.map_err(|e| e.to_string())?;
    let reply = v["reply"].as_str().unwrap_or("").to_string();
    let plan = v["plan"].as_str().unwrap_or("").to_string();
    match v["route"].as_str().unwrap_or("delegate") {
        "direct" => {
            let mel: Vec<String> = match &v["mel"] {
                Value::Array(a) => a.iter().filter_map(|x| x.as_str()).flat_map(split_statements).collect(),
                Value::String(s) => split_statements(s),
                _ => vec![],
            };
            if mel.is_empty() {
                Ok(Route::Delegate { plan })
            } else {
                Ok(Route::Direct { reply: if reply.is_empty() { "Done.".into() } else { reply }, mel })
            }
        }
        "answer" => Ok(Route::Answer(if reply.is_empty() { plan } else { reply })),
        _ => Ok(Route::Delegate { plan }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_calls() {
        let m = json!({"content": "", "tool_calls": [
            {"function": {"name": "run_mel", "arguments": {"script": "polySphere -n b; setColor blue", "reply": "Blue ball."}}},
        ]});
        assert_eq!(parse_decision(&m).unwrap(), Route::Direct { reply: "Blue ball.".into(), mel: vec!["polySphere -n b".into(), "setColor blue".into()] });
        // OpenAI/OpenRouter style: arguments as a JSON string
        let m = json!({"tool_calls": [{"type": "function", "function": {"name": "delegate_to_builder", "arguments": "{\"plan\":\"1. top 2. legs\"}"}}]});
        assert_eq!(parse_decision(&m).unwrap(), Route::Delegate { plan: "1. top 2. legs".into() });
        let m = json!({"tool_calls": [{"function": {"name": "answer", "arguments": {"text": "Press W."}}}]});
        assert_eq!(parse_decision(&m).unwrap(), Route::Answer("Press W.".into()));
        let m = json!({"tool_calls": [{"function": {"name": "undo", "arguments": {}}}]});
        assert!(matches!(parse_decision(&m).unwrap(), Route::Direct { .. }));
    }

    #[test]
    fn json_routes() {
        let m = json!({"content": "{\"route\":\"direct\",\"reply\":\"ok\",\"mel\":[\"viewSet -top\"]}"});
        assert!(matches!(parse_decision(&m).unwrap(), Route::Direct { .. }));
        let m = json!({"content": "{\"route\":\"direct\",\"reply\":\"x\",\"mel\":[]}"});
        assert!(matches!(parse_decision(&m).unwrap(), Route::Delegate { .. }), "empty direct falls back to the builder");
        let m = json!({"content": "<think>hmm</think>{\"route\":\"answer\",\"reply\":\"Use F11.\"}"});
        assert_eq!(parse_decision(&m).unwrap(), Route::Answer("Use F11.".into()));
    }

    #[test]
    fn picks_tev1() {
        let inst = vec!["qwen3:8b".to_string(), "tev1:latest".to_string()];
        assert_eq!(pick_decision_model(&inst).as_deref(), Some("tev1:latest"));
        assert_eq!(pick_decision_model(&["qwen3:8b".to_string()]), None);
    }
}
