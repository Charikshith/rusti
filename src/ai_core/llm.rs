// Async LLM client: streams chat completions from any OpenAI-compatible
// /v1/chat/completions endpoint over reqwest. Content deltas are emitted as
// they arrive; tool-call arguments arrive as JSON fragments and are
// accumulated per call index (same pattern pi's parseStreamingJson does).

use super::{emit, Event};
use bytes::BytesMut;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const RETRIES: u32 = 3;
/// First backoff step; doubles per attempt (1s, 2s, 4s). Tests shrink it.
pub static RETRY_BASE_MS: AtomicU64 = AtomicU64::new(1000);

/// Transient statuses worth a retry; other 4xx are the caller's fault.
pub fn retryable(status: u16) -> bool {
    matches!(status, 408 | 429) || status >= 500
}

pub struct Client {
    pub url: String,
    pub key: String,
    pub model: String,
    http: reqwest::Client,
    /// Set once this server has rejected `stream_options` with a 400. A Client
    /// is one profile, so it stops sending it for the Client's lifetime: until
    /// /model switches profile or rusti restarts.
    no_stream_options: AtomicBool,
}

/// A 400 that blames `stream_options`: the server does not take the field,
/// so the request is worth one more try without it.
pub fn rejects_stream_options(status: u16, body: &str) -> bool {
    status == 400 && body.contains("stream_options")
}

/// Prompt tokens the provider served from its prefix cache, under whichever
/// name it reports them: OpenAI's details object, DeepSeek's hit count, or
/// Kimi's bare `cached_tokens`. None when the server says nothing about caching.
pub fn cached_tokens(usage: &Value) -> Option<u64> {
    usage
        .pointer("/prompt_tokens_details/cached_tokens")
        .or_else(|| usage.get("prompt_cache_hit_tokens"))
        .or_else(|| usage.get("cached_tokens"))
        .and_then(|x| x.as_u64())
}

impl Client {
    pub fn new(url: String, key: String, model: String) -> Self {
        // ponytail: some LLM proxies close idle connections after ~10s;
        // a pooled connection reused next turn is dead -> "error sending request".
        // No pooling = fresh connection per request (localhost handshake is free).
        let http = reqwest::Client::builder().pool_max_idle_per_host(0).build().unwrap();
        Client { url, key, model, http, no_stream_options: AtomicBool::new(false) }
    }

    pub async fn chat_stream(
        &self,
        messages: &[Value],
        tools: Option<&[Value]>,
        cancel: &AtomicBool,
    ) -> Result<ChatResult, String> {
        let mut body = json!({ "model": self.model, "messages": messages, "stream": true });
        if let Some(t) = tools {
            body["tools"] = Value::Array(t.to_vec());
        }
        // without it OpenAI streams no usage at all, so every count is a guess
        if !self.no_stream_options.load(Ordering::Relaxed) {
            body["stream_options"] = json!({"include_usage": true});
        }
        // retries cover only the connect + status phase: once content has
        // streamed, replaying would duplicate output in the transcript
        let mut attempt = 0u32;
        let resp = loop {
            let mut req = self.http.post(&self.url).json(&body);
            if !self.key.is_empty() {
                req = req.bearer_auth(&self.key);
            }
            let err = match req.send().await {
                Ok(r) if r.status().is_success() => break r,
                Ok(r) => {
                    let status = r.status();
                    let text = r.text().await.unwrap_or_default();
                    let msg = format!("HTTP {status}: {}", truncate(&text, 500));
                    // once: the field is gone after this, so a second 400 is a real error
                    if body.get("stream_options").is_some() && rejects_stream_options(status.as_u16(), &text) {
                        self.no_stream_options.store(true, Ordering::Relaxed);
                        body.as_object_mut().unwrap().remove("stream_options");
                        continue;
                    }
                    if !retryable(status.as_u16()) {
                        return Err(msg);
                    }
                    msg
                }
                Err(e) => format!("request failed: {e}"),
            };
            if attempt >= RETRIES {
                return Err(err);
            }
            attempt += 1;
            let wait = RETRY_BASE_MS.load(Ordering::Relaxed) << (attempt - 1);
            emit(Event::Retry { attempt, of: RETRIES, wait_ms: wait, err });
            let t0 = Instant::now();
            while (t0.elapsed().as_millis() as u64) < wait {
                if cancel.load(Ordering::Relaxed) {
                    return Err("interrupted".into());
                }
                tokio::time::sleep(Duration::from_millis(wait.min(50))).await;
            }
        };

        let mut stream = resp.bytes_stream();
        let mut buf = BytesMut::new();
        let mut content = String::new();
        let mut calls: Vec<ToolCallAcc> = Vec::new();
        let mut finish = None;
        // generation accounting: clock starts at the first token, so prompt
        // processing (TTFT) doesn't dilute the tokens/sec figure
        let mut first: Option<Instant> = None;
        let mut usage: Option<u64> = None;
        let mut prompt: Option<u64> = None;
        let mut cached: Option<u64> = None;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("interrupted".into());
            }
            match stream.next().await {
                Some(Ok(chunk)) => {
                    buf.extend_from_slice(&chunk);
                    for ev in take_sse_events(&mut buf) {
                        handle_event(&ev, &mut content, &mut calls, &mut finish, &mut first, &mut usage, &mut prompt, &mut cached)?;
                    }
                }
                Some(Err(e)) => return Err(format!("stream error: {e}")),
                None => break,
            }
        }
        if !buf.is_empty() {
            // server closed without a trailing blank line
            let ev = String::from_utf8_lossy(&buf).into_owned();
            handle_event(&ev, &mut content, &mut calls, &mut finish, &mut first, &mut usage, &mut prompt, &mut cached)?;
        }

        // usage when the server volunteers it (many OpenAI-compatible ones
        // don't, streaming); otherwise the 4-chars-a-token rule of thumb,
        // flagged as an estimate in the UI
        let prompt_tokens = prompt.unwrap_or((body.to_string().len() as u64 + 3) / 4);
        emit(Event::Usage {
            tokens: usage.unwrap_or((content.chars().count() as u64 + 3) / 4),
            prompt: prompt_tokens,
            est: usage.is_none() || prompt.is_none(),
            gen_ms: first.map(|t| t.elapsed().as_millis()).unwrap_or(0),
            cached,
        });

        let tool_calls: Vec<ToolCall> = calls
            .into_iter()
            .map(|c| ToolCall {
                id: c.id,
                name: c.name,
                raw_arguments: c.raw_arguments.clone(),
                arguments: serde_json::from_str(&c.raw_arguments).unwrap_or(Value::Null),
            })
            .collect();
        let finish_reason = finish
            .unwrap_or_else(|| if tool_calls.is_empty() { "stop".to_string() } else { "tool_calls".to_string() });
        Ok(ChatResult { content, tool_calls, finish_reason, prompt_tokens })
    }
}

pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub raw_arguments: String,
    pub arguments: Value,
}

pub struct ChatResult {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
    pub prompt_tokens: u64, // context size this request sent (estimated if the server omits usage)
}

#[derive(Default, Clone)]
struct ToolCallAcc {
    id: String,
    name: String,
    raw_arguments: String,
}

fn handle_event(
    ev: &str,
    content: &mut String,
    calls: &mut Vec<ToolCallAcc>,
    finish: &mut Option<String>,
    first: &mut Option<Instant>,
    usage: &mut Option<u64>,
    prompt: &mut Option<u64>,
    cached: &mut Option<u64>,
) -> Result<(), String> {
    for line in ev.lines() {
        let data = match line.trim_start().strip_prefix("data:") {
            Some(d) => d.trim(),
            None => continue,
        };
        if data == "[DONE]" {
            continue;
        }
        let v: Value = serde_json::from_str(data).map_err(|e| format!("bad SSE JSON: {e}"))?;
        if let Some(err) = v.get("error") {
            return Err(format!("API error: {err}"));
        }
        // Kimi streams its usage inside choices[0] rather than at the top level
        if let Some(u) = v.get("usage").filter(|u| !u.is_null()).or_else(|| v.pointer("/choices/0/usage")) {
            if let Some(t) = u.get("completion_tokens").and_then(|x| x.as_u64()) {
                *usage = Some(t);
            }
            if let Some(p) = u.get("prompt_tokens").and_then(|x| x.as_u64()) {
                *prompt = Some(p);
            }
            if let Some(c) = cached_tokens(u) {
                *cached = Some(c);
            }
        }
        let Some(choice) = v.get("choices").and_then(|c| c.get(0)) else { continue };
        if let Some(delta) = choice.get("delta") {
            if let Some(c) = delta.get("content").and_then(|x| x.as_str()) {
                if !c.is_empty() {
                    first.get_or_insert_with(Instant::now);
                    content.push_str(c);
                    emit(Event::TextDelta(c.to_string()));
                }
            }
            // thinking models (QwQ, DeepSeek R1, GLM) stream their scratchpad
            // in reasoning_content; it is shown but never fed back as content
            if let Some(r) = delta
                .get("reasoning_content")
                .or_else(|| delta.get("reasoning"))
                .and_then(|x| x.as_str())
            {
                if !r.is_empty() {
                    first.get_or_insert_with(Instant::now);
                    emit(Event::ReasoningDelta(r.to_string()));
                }
            }
            if let Some(tcs) = delta.get("tool_calls").and_then(|x| x.as_array()) {
                for tc in tcs {
                    let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                    if idx >= calls.len() {
                        calls.resize(idx + 1, ToolCallAcc::default());
                    }
                    let acc = &mut calls[idx];
                    if let Some(id) = tc.get("id").and_then(|x| x.as_str()) {
                        acc.id = id.to_string();
                    }
                    if let Some(n) = tc.get("function").and_then(|f| f.get("name")).and_then(|x| x.as_str()) {
                        acc.name = n.to_string();
                    }
                    if let Some(a) = tc.get("function").and_then(|f| f.get("arguments")).and_then(|x| x.as_str()) {
                        acc.raw_arguments.push_str(a);
                    }
                }
            }
        }
        if let Some(fr) = choice.get("finish_reason").and_then(|x| x.as_str()) {
            if !fr.is_empty() {
                *finish = Some(fr.to_string());
            }
        }
    }
    Ok(())
}

/// Split complete SSE events ("\n\n" or "\r\n\r\n" separated) out of the
/// buffer, leaving partial data for the next chunk. JSON in `data:` lines is
/// compact, so raw blank lines only appear as event delimiters.
fn take_sse_events(buf: &mut BytesMut) -> Vec<String> {
    let mut out = Vec::new();
    loop {
        let sep = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .or_else(|| buf.windows(2).position(|w| w == b"\n\n"));
        match sep {
            Some(i) => out.push(String::from_utf8_lossy(&buf.split_to(i + 2)).into_owned()),
            None => break,
        }
    }
    out
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_hits_read_under_each_providers_name() {
        assert_eq!(cached_tokens(&json!({"prompt_tokens": 100, "prompt_tokens_details": {"cached_tokens": 80}})), Some(80));
        assert_eq!(cached_tokens(&json!({"prompt_cache_hit_tokens": 64, "prompt_cache_miss_tokens": 36})), Some(64)); // DeepSeek
        assert_eq!(cached_tokens(&json!({"cached_tokens": 12})), Some(12)); // Kimi
        assert_eq!(cached_tokens(&json!({"prompt_tokens": 100})), None, "silence is not a 0% hit rate");
        assert_eq!(cached_tokens(&Value::Null), None);

        assert!(rejects_stream_options(400, r#"{"error":"Unrecognized request argument supplied: stream_options"}"#));
        assert!(!rejects_stream_options(400, "model not found"), "only the field's own rejection drops it");
        assert!(!rejects_stream_options(500, "stream_options"));
    }

    #[test]
    fn usage_read_from_choices_when_not_top_level() {
        let (mut content, mut calls, mut finish, mut first) = (String::new(), Vec::new(), None, None);
        let (mut usage, mut prompt, mut cached) = (None, None, None);
        let kimi = r#"data: {"usage":null,"choices":[{"index":0,"delta":{},"finish_reason":"stop","usage":{"prompt_tokens":100,"completion_tokens":7,"total_tokens":107,"cached_tokens":40}}]}"#;
        handle_event(kimi, &mut content, &mut calls, &mut finish, &mut first, &mut usage, &mut prompt, &mut cached).unwrap();
        assert_eq!((usage, prompt, cached), (Some(7), Some(100), Some(40)));
        assert_eq!(finish.as_deref(), Some("stop"));
    }
}
