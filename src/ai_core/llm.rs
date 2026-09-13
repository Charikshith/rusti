// Async LLM client: streams chat completions from any OpenAI-compatible
// /v1/chat/completions endpoint over reqwest. Content deltas are emitted as
// they arrive; tool-call arguments arrive as JSON fragments and are
// accumulated per call index (same pattern pi's parseStreamingJson does).

use super::{emit, Event};
use bytes::BytesMut;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

pub struct Client {
    pub url: String,
    pub key: String,
    pub model: String,
    http: reqwest::Client,
}

impl Client {
    pub fn new(url: String, key: String, model: String) -> Self {
        // ponytail: some LLM proxies close idle connections after ~10s;
        // a pooled connection reused next turn is dead -> "error sending request".
        // No pooling = fresh connection per request (localhost handshake is free).
        let http = reqwest::Client::builder().pool_max_idle_per_host(0).build().unwrap();
        Client { url, key, model, http }
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
        let mut req = self.http.post(&self.url).json(&body);
        if !self.key.is_empty() {
            req = req.bearer_auth(&self.key);
        }
        let resp = req.send().await.map_err(|e| format!("request failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP {status}: {}", truncate(&text, 500)));
        }

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
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("interrupted".into());
            }
            match stream.next().await {
                Some(Ok(chunk)) => {
                    buf.extend_from_slice(&chunk);
                    for ev in take_sse_events(&mut buf) {
                        handle_event(&ev, &mut content, &mut calls, &mut finish, &mut first, &mut usage, &mut prompt)?;
                    }
                }
                Some(Err(e)) => return Err(format!("stream error: {e}")),
                None => break,
            }
        }
        if !buf.is_empty() {
            // server closed without a trailing blank line
            let ev = String::from_utf8_lossy(&buf).into_owned();
            handle_event(&ev, &mut content, &mut calls, &mut finish, &mut first, &mut usage, &mut prompt)?;
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
        if let Some(t) = v.pointer("/usage/completion_tokens").and_then(|x| x.as_u64()) {
            *usage = Some(t);
        }
        if let Some(p) = v.pointer("/usage/prompt_tokens").and_then(|x| x.as_u64()) {
            *prompt = Some(p);
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
