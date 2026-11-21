// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Probing a model server: which models it lists, which APIs answer and
//! its context size. A local agent is Claude Code, so a server without the
//! Anthropic Messages API (`/v1/messages`) can't run one, and the probe
//! says so. A server that wants a token gets it both ways servers read
//! one: `Authorization: Bearer` and `x-api-key`.

use std::time::Duration;

use maxhq_protocol::local_models::{
    ModelCatalog, ServerApis, ServerFlavor, ServerModel, ServerProbe,
};
use maxhq_protocol::secret::Secret;
use maxhq_protocol::time::Timestamp;
use serde_json::{json, Value};

use super::catalog;

/// How long a list or settings call may take.
const LOOK: Duration = Duration::from_secs(10);
/// How long a 1-token call may take: a server may load the model first.
const TRY: Duration = Duration::from_secs(120);

/// What a probe found.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// The probe as the server's entity shows it.
    pub probe: ServerProbe,
    /// What answers there.
    pub flavor: ServerFlavor,
    /// The models it lists, matched to the catalog where they can be.
    pub models: Vec<ServerModel>,
}

/// Probes the server at `base`. A managed server is only asked for the
/// Messages API; it's llama-server, so chat and responses answer too.
pub async fn probe(
    base: &str,
    token: Option<&Secret>,
    catalog: &ModelCatalog,
    managed: bool,
) -> Found {
    let base = base.trim_end_matches('/');
    let at = Timestamp::now();
    let listed = get(base, "/v1/models", token).await;
    let tags = get(base, "/api/tags", token).await;
    let ollama = tags
        .as_ref()
        .ok()
        .and_then(|v| v.get("models"))
        .and_then(Value::as_array)
        .cloned();
    let mut models = match (&listed, &ollama) {
        (Ok(list), _) => openai_models(list),
        (Err(_), Some(tags)) => tags
            .iter()
            .filter_map(|m| m.get("name").and_then(Value::as_str))
            .map(|id| (id.to_owned(), None))
            .collect(),
        (Err(_), None) => Vec::new(),
    };
    if let (Err(Fail::Refused(status)), None) = (&listed, &ollama) {
        let error = if *status == 401 || *status == 403 {
            "The server wants a token, or a different one.".to_owned()
        } else {
            format!("The server answered {status} for its model list.")
        };
        return failed(at, error);
    }
    if let (Err(Fail::Unreachable(why)), None) = (&listed, &ollama) {
        return failed(at, format!("Couldn't reach the server: {why}"));
    }
    let first = models.first().map(|(id, _)| id.clone());
    let mut apis = ServerApis::default();
    if let Some(model) = first.as_deref() {
        apis.messages = post(
            base,
            "/v1/messages",
            token,
            &json!({"model": model, "max_tokens": 1, "messages": [{"role": "user", "content": "Hi"}]}),
        )
        .await
        .is_some_and(|v| v.get("content").is_some() || v.get("type").and_then(Value::as_str) == Some("message"));
        if managed {
            apis.chat = true;
            apis.responses = true;
        } else {
            apis.chat = post(
                base,
                "/v1/chat/completions",
                token,
                &json!({"model": model, "max_tokens": 1, "messages": [{"role": "user", "content": "Hi"}]}),
            )
            .await
            .is_some_and(|v| v.get("choices").is_some());
            apis.responses = post(
                base,
                "/v1/responses",
                token,
                &json!({"model": model, "max_output_tokens": 16, "input": "Hi"}),
            )
            .await
            .is_some_and(|v| v.get("output").is_some());
        }
    }
    let props = get(base, "/props", token).await.ok();
    let llama_ctx = props.as_ref().and_then(|p| {
        p.pointer("/default_generation_settings/n_ctx")
            .or_else(|| p.get("n_ctx"))
            .and_then(Value::as_u64)
    });
    let lm_studio = match (&ollama, llama_ctx) {
        (None, None) => get(base, "/api/v0/models", token).await.ok(),
        _ => None,
    };
    let running = match &ollama {
        Some(_) => get(base, "/api/ps", token).await.ok(),
        None => None,
    };
    let flavor = if ollama.is_some() {
        ServerFlavor::Ollama
    } else if llama_ctx.is_some() {
        ServerFlavor::LlamaCpp
    } else if lm_studio.is_some() {
        ServerFlavor::LmStudio
    } else if models.iter().any(|(_, ctx)| ctx.is_some()) {
        ServerFlavor::Vllm
    } else {
        ServerFlavor::Other
    };
    // Each model's context: what the server says it runs at.
    for (id, ctx) in &mut models {
        if ctx.is_none() {
            *ctx = lm_studio
                .as_ref()
                .and_then(|v| {
                    per_model(
                        v,
                        "data",
                        "id",
                        id,
                        &["loaded_context_length", "max_context_length"],
                    )
                })
                .or_else(|| {
                    running
                        .as_ref()
                        .and_then(|v| per_model(v, "models", "name", id, &["context_length"]))
                });
        }
    }
    let context = llama_ctx
        .and_then(|n| u32::try_from(n).ok())
        .or_else(|| models.first().and_then(|(_, ctx)| *ctx));
    let models = models
        .into_iter()
        .map(|(id, ctx)| ServerModel {
            label: id.clone(),
            catalog: catalog::recognize(catalog, &id),
            context: ctx.or(context),
            load: None,
            id,
        })
        .collect();
    let error = (!apis.messages).then(|| {
        if first.is_none() {
            "The server lists no models.".to_owned()
        } else {
            "The server doesn't answer the Anthropic Messages API (/v1/messages), which a local agent needs.".to_owned()
        }
    });
    Found {
        probe: ServerProbe {
            at,
            apis,
            context,
            error,
        },
        flavor,
        models,
    }
}

fn failed(at: Timestamp, error: String) -> Found {
    Found {
        probe: ServerProbe {
            at,
            apis: ServerApis::default(),
            context: None,
            error: Some(error),
        },
        flavor: ServerFlavor::Other,
        models: Vec::new(),
    }
}

/// The models an OpenAI-style list names, with a context size where the
/// server gives one (vLLM's `max_model_len`, llama-server's `meta.n_ctx_train`
/// is the training size, so it's left out).
fn openai_models(list: &Value) -> Vec<(String, Option<u32>)> {
    list.get("data")
        .and_then(Value::as_array)
        .map(|data| {
            data.iter()
                .filter_map(|m| {
                    let id = m.get("id").and_then(Value::as_str)?;
                    let ctx = m
                        .get("max_model_len")
                        .and_then(Value::as_u64)
                        .and_then(|n| u32::try_from(n).ok());
                    Some((id.to_owned(), ctx))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A number one model's entry in a list gives, under the first of `keys`
/// it has.
fn per_model(list: &Value, field: &str, name: &str, id: &str, keys: &[&str]) -> Option<u32> {
    let entry = list
        .get(field)?
        .as_array()?
        .iter()
        .find(|m| m.get(name).and_then(Value::as_str) == Some(id))?;
    keys.iter()
        .find_map(|k| entry.get(*k).and_then(Value::as_u64))
        .and_then(|n| u32::try_from(n).ok())
}

/// Why a call got no JSON.
#[derive(Debug)]
enum Fail {
    Unreachable(String),
    Refused(u16),
}

fn request(method: reqwest::Method, url: &str, token: Option<&Secret>) -> reqwest::RequestBuilder {
    let mut request = super::http()
        .request(method, url)
        .header("accept", "application/json")
        .header("anthropic-version", "2023-06-01");
    if let Some(token) = token {
        request = request
            .bearer_auth(token.expose())
            .header("x-api-key", token.expose());
    }
    request
}

async fn get(base: &str, path: &str, token: Option<&Secret>) -> Result<Value, Fail> {
    let answer = request(reqwest::Method::GET, &format!("{base}{path}"), token)
        .timeout(LOOK)
        .send()
        .await
        .map_err(|err| Fail::Unreachable(crate::tools::install::fetch::plain(&err)))?;
    if !answer.status().is_success() {
        return Err(Fail::Refused(answer.status().as_u16()));
    }
    let text = answer.text().await.map_err(|_| Fail::Refused(200))?;
    serde_json::from_str(&text).map_err(|_| Fail::Refused(200))
}

async fn post(base: &str, path: &str, token: Option<&Secret>, body: &Value) -> Option<Value> {
    let answer = request(reqwest::Method::POST, &format!("{base}{path}"), token)
        .timeout(TRY)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .ok()?;
    if !answer.status().is_success() {
        return None;
    }
    serde_json::from_str(&answer.text().await.ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::super::tests::Fake;
    use super::*;

    fn catalog() -> ModelCatalog {
        catalog::builtin()
    }

    #[tokio::test]
    async fn a_llama_server_runs_a_local_agent() {
        let fake = Fake::start().await;
        fake.json(
            "/v1/models",
            200,
            &json!({"data": [{"id": "qwen3.6-35b-a3b@ud-q4_k_xl"}]}),
        );
        fake.json(
            "POST /v1/messages",
            200,
            &json!({"type": "message", "content": []}),
        );
        fake.json("POST /v1/chat/completions", 200, &json!({"choices": []}));
        fake.json("POST /v1/responses", 200, &json!({"output": []}));
        fake.json(
            "/props",
            200,
            &json!({"default_generation_settings": {"n_ctx": 65536}}),
        );
        let token = Secret::new("sk-local".to_owned());
        let found = probe(&fake.base, Some(&token), &catalog(), false).await;
        assert_eq!(found.flavor, ServerFlavor::LlamaCpp);
        assert!(found.probe.apis.messages && found.probe.apis.chat && found.probe.apis.responses);
        assert_eq!(found.probe.context, Some(65_536));
        assert_eq!(found.probe.error, None);
        let model = &found.models[0];
        assert_eq!(
            model.catalog.as_ref().unwrap().file,
            "Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf"
        );
        assert_eq!(model.context, Some(65_536));
        let asked = fake.seen();
        let messages = asked.iter().find(|s| s.path == "/v1/messages").unwrap();
        assert_eq!(messages.header("authorization"), Some("Bearer sk-local"));
        assert_eq!(messages.header("x-api-key"), Some("sk-local"));
        assert!(messages.body.contains("\"max_tokens\":1"));
    }

    #[tokio::test]
    async fn an_openai_only_server_cant_run_a_local_agent_and_says_why() {
        let fake = Fake::start().await;
        fake.json(
            "/v1/models",
            200,
            &json!({"data": [{"id": "gpt-oss-20b", "max_model_len": 131072, "owned_by": "vllm"}]}),
        );
        fake.json("POST /v1/chat/completions", 200, &json!({"choices": []}));
        let found = probe(&fake.base, None, &catalog(), false).await;
        assert_eq!(found.flavor, ServerFlavor::Vllm);
        assert!(!found.probe.apis.messages);
        assert!(found.probe.apis.chat);
        assert!(!found.probe.apis.responses);
        assert_eq!(found.probe.context, Some(131_072));
        assert!(found.probe.error.unwrap().contains("/v1/messages"));
        assert_eq!(
            found.models[0].catalog.as_ref().unwrap().model,
            "gpt-oss-20b"
        );
        // No token, no auth headers.
        assert!(fake
            .seen()
            .iter()
            .all(|s| s.header("authorization").is_none()));
    }

    #[tokio::test]
    async fn ollama_is_known_by_its_tags() {
        let fake = Fake::start().await;
        fake.json(
            "/api/tags",
            200,
            &json!({"models": [{"name": "qwen3.6:35b-a3b"}, {"name": "llama3.2:3b"}]}),
        );
        fake.json(
            "/v1/models",
            200,
            &json!({"data": [{"id": "qwen3.6:35b-a3b"}, {"id": "llama3.2:3b"}]}),
        );
        fake.json(
            "/api/ps",
            200,
            &json!({"models": [{"name": "qwen3.6:35b-a3b", "context_length": 32768}]}),
        );
        fake.json(
            "POST /v1/messages",
            200,
            &json!({"type": "message", "content": [{"type": "text", "text": "H"}]}),
        );
        fake.json("POST /v1/chat/completions", 200, &json!({"choices": []}));
        let found = probe(&fake.base, None, &catalog(), false).await;
        assert_eq!(found.flavor, ServerFlavor::Ollama);
        assert!(found.probe.apis.messages);
        assert_eq!(found.probe.context, Some(32_768));
        assert_eq!(found.models.len(), 2);
        assert_eq!(
            found.models[0].catalog.as_ref().unwrap().model,
            "qwen3.6-35b-a3b"
        );
        assert_eq!(found.models[1].catalog, None);
    }

    #[tokio::test]
    async fn a_server_that_wants_a_token_says_so() {
        let fake = Fake::start().await;
        fake.json("/v1/models", 401, &json!({"error": "unauthorized"}));
        let found = probe(&fake.base, None, &catalog(), false).await;
        assert!(found.probe.error.unwrap().contains("token"));
        assert!(found.models.is_empty());
    }

    #[tokio::test]
    async fn a_server_that_isnt_there_says_so() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let found = probe(&base, None, &catalog(), false).await;
        assert!(found.probe.error.unwrap().starts_with("Couldn't reach"));
    }
}
