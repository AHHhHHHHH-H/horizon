//! The OpenAI-compatible streaming chat client: our llama-server, Ollama (`localhost:11434/v1`),
//! LM Studio (`localhost:1234/v1`) or a cloud endpoint with a key, all through `/chat/completions` with
//! server-sent events.
//!
//! [`Brain`] runs requests on its own thread: [`Brain::ask`] queues one and returns its id, and
//! [`Brain::try_recv`] hands back what has arrived, never waiting. Each reply comes as the sentences of
//! `say` the moment they are complete, then the parsed reply with its timings.

use std::io::{BufRead, BufReader};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use serde_json::value::RawValue;

use crate::reply::{self, Reply, SayStream, Sentences, Tone};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// The runtime this crate supervises.
    Local,
    Ollama,
    LmStudio,
    /// Any OpenAI-compatible service; conversations leave the PC.
    Cloud,
}

impl Provider {
    /// Where the program listens when it runs on this PC.
    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            Provider::Ollama => Some("http://127.0.0.1:11434/v1"),
            Provider::LmStudio => Some("http://127.0.0.1:1234/v1"),
            Provider::Local | Provider::Cloud => None,
        }
    }
}

/// Where requests go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub provider: Provider,
    /// Up to and including `/v1`.
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

impl Message {
    pub fn new(role: Role, content: impl Into<String>) -> Message {
        Message {
            role,
            content: content.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatRequest {
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    pub temperature: f32,
    /// Constrain the answer to the reply schema (else free text, e.g. for memory summaries).
    pub structured: bool,
}

impl ChatRequest {
    /// A companion turn: the reply schema, short, a little creative.
    pub fn turn(messages: Vec<Message>) -> ChatRequest {
        ChatRequest {
            messages,
            max_tokens: 200,
            temperature: 0.7,
            structured: true,
        }
    }

    /// Processes the system prompt (and memory) ahead of the first turn, so the runtime has it cached:
    /// send it once the runtime is ready and ignore the answer. On a 2-core laptop the prefix takes
    /// about 20 s to process; after this the first real turn starts as fast as the later ones.
    pub fn warm_up(system: Message) -> ChatRequest {
        ChatRequest {
            messages: vec![system, Message::new(Role::User, "[event] Loading.")],
            max_tokens: 1,
            temperature: 0.0,
            structured: false,
        }
    }
}

/// How long a reply took, from the request being sent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub first_token: Option<Duration>,
    pub first_sentence: Option<Duration>,
    pub total: Duration,
    /// Prompt tokens processed for this request, and those reused from the cache (llama.cpp only).
    pub prompt_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    /// Generation speed as the server measured it, else from the stream.
    pub tokens_per_second: Option<f64>,
    pub prompt_ms: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A complete sentence of `say`, for chat and the voice, and the reply's tone so far.
    Sentence {
        id: u64,
        text: String,
        tone: Tone,
    },
    /// The whole reply, parsed.
    Done {
        id: u64,
        reply: Reply,
        raw: String,
        stats: Stats,
    },
    Failed {
        id: u64,
        error: String,
    },
}

impl Event {
    pub fn id(&self) -> u64 {
        match self {
            Event::Sentence { id, .. } | Event::Done { id, .. } | Event::Failed { id, .. } => *id,
        }
    }
}

struct Job {
    id: u64,
    request: ChatRequest,
}

/// The brain's connection: a worker thread and two channels.
pub struct Brain {
    jobs: Sender<Job>,
    events: Receiver<Event>,
    /// Requests with an id up to this are dropped, mid-stream included.
    cancelled: Arc<AtomicU64>,
    next_id: u64,
    endpoint: Endpoint,
}

impl Brain {
    pub fn new(endpoint: Endpoint) -> Brain {
        let (jobs, job_rx) = mpsc::channel::<Job>();
        let (event_tx, events) = mpsc::channel();
        let cancelled = Arc::new(AtomicU64::new(0));
        let (worker_endpoint, worker_cancelled) = (endpoint.clone(), cancelled.clone());
        std::thread::Builder::new()
            .name("companion-brain".into())
            .spawn(move || {
                let agent = agent_for(&worker_endpoint);
                for job in job_rx {
                    if job.id <= worker_cancelled.load(Ordering::Relaxed) {
                        continue;
                    }
                    let id = job.id;
                    let result = stream_chat(
                        &agent,
                        &worker_endpoint,
                        &job.request,
                        &mut |text, tone| {
                            let _ = event_tx.send(Event::Sentence { id, text, tone });
                        },
                        &|| id <= worker_cancelled.load(Ordering::Relaxed),
                    );
                    let event = match result {
                        Ok((reply, raw, stats)) => Event::Done {
                            id,
                            reply,
                            raw,
                            stats,
                        },
                        Err(error) => Event::Failed { id, error },
                    };
                    if event_tx.send(event).is_err() {
                        break;
                    }
                }
            })
            .expect("the brain thread starts");
        Brain {
            jobs,
            events,
            cancelled,
            next_id: 1,
            endpoint,
        }
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Queues a request; its events carry the returned id.
    pub fn ask(&mut self, request: ChatRequest) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let _ = self.jobs.send(Job { id, request });
        id
    }

    /// The next event, if one has arrived.
    pub fn try_recv(&self) -> Option<Event> {
        match self.events.try_recv() {
            Ok(event) if event.id() <= self.cancelled.load(Ordering::Relaxed) => self.try_recv(),
            Ok(event) => Some(event),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    /// Drops `id` and every request before it (the player said something new).
    pub fn cancel_up_to(&self, id: u64) {
        self.cancelled.fetch_max(id, Ordering::Relaxed);
    }
}

fn agent_for(endpoint: &Endpoint) -> ureq::Agent {
    let local = endpoint.provider != Provider::Cloud;
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(if local { 3 } else { 15 }))
        // Between two pieces of the stream: a CPU may take a while on a long prompt.
        .timeout_read(Duration::from_secs(120))
        .user_agent(concat!("Horizon/", env!("CARGO_PKG_VERSION")))
        .build()
}

#[derive(Serialize)]
struct Body<'a> {
    model: &'a str,
    messages: &'a [Message],
    stream: bool,
    max_tokens: u32,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<&'a RawValue>,
    /// Qwen3.5's template: no thinking (llama.cpp; LM Studio and Ollama ignore it).
    #[serde(skip_serializing_if = "Option::is_none")]
    chat_template_kwargs: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<Value>,
    /// llama.cpp: reuse the cached prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_prompt: Option<bool>,
}

/// The JSON body of a request; public so tools can log exactly what was sent.
pub fn request_body(endpoint: &Endpoint, request: &ChatRequest) -> String {
    let strict = endpoint.provider == Provider::Cloud;
    let format = request.structured.then(|| {
        let schema = if strict { reply::SCHEMA_STRICT } else { reply::SCHEMA };
        format!(r#"{{"type":"json_schema","json_schema":{{"name":"companion_reply","strict":{strict},"schema":{schema}}}}}"#)
    });
    let format = format.map(|f| RawValue::from_string(f).expect("the schema is valid JSON"));
    let local_llama = endpoint.provider == Provider::Local;
    let body = Body {
        model: &endpoint.model,
        messages: &request.messages,
        stream: true,
        max_tokens: request.max_tokens,
        temperature: request.temperature,
        response_format: format.as_deref(),
        chat_template_kwargs: (endpoint.provider != Provider::Cloud)
            .then(|| serde_json::json!({ "enable_thinking": false })),
        stream_options: (!local_llama).then(|| serde_json::json!({ "include_usage": true })),
        cache_prompt: local_llama.then_some(true),
    };
    serde_json::to_string(&body).expect("the request serialises")
}

/// Sends one request and reads its stream to the end, handing each finished sentence to `sentence`.
pub fn stream_chat(
    agent: &ureq::Agent,
    endpoint: &Endpoint,
    request: &ChatRequest,
    sentence: &mut dyn FnMut(String, Tone),
    cancelled: &dyn Fn() -> bool,
) -> Result<(Reply, String, Stats), String> {
    let started = Instant::now();
    let mut call = agent
        .post(&format!(
            "{}/chat/completions",
            endpoint.base_url.trim_end_matches('/')
        ))
        .set("Content-Type", "application/json")
        .set("Accept", "text/event-stream");
    if let Some(key) = &endpoint.api_key {
        call = call.set("Authorization", &format!("Bearer {key}"));
    }
    let response = match call.send_string(&request_body(endpoint, request)) {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let text = r.into_string().unwrap_or_default();
            return Err(format!("{code}: {}", error_message(&text).unwrap_or(text)));
        }
        Err(e) => return Err(e.to_string()),
    };
    let mut stats = Stats::default();
    let (mut raw, mut say, mut sentences) = (String::new(), SayStream::new(), Sentences::new());
    let mut chunks = 0u64;
    let mut tone = None;
    let mut emit = |text: String, tone: Option<Tone>, stats: &mut Stats| {
        stats.first_sentence.get_or_insert(started.elapsed());
        sentence(
            reply::restore_apostrophes(&text).into_owned(),
            tone.unwrap_or_default(),
        );
    };
    for line in BufReader::new(response.into_reader()).lines() {
        if cancelled() {
            return Err("cancelled".into());
        }
        let line = line.map_err(|e| format!("the stream broke: {e}"))?;
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }
        let Ok(chunk) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if let Some(message) = chunk
            .get("error")
            .and_then(|e| e.get("message").and_then(Value::as_str))
        {
            return Err(message.to_owned());
        }
        read_stats(&chunk, &mut stats);
        let Some(delta) = chunk
            .pointer("/choices/0/delta/content")
            .and_then(Value::as_str)
        else {
            continue;
        };
        if delta.is_empty() {
            continue;
        }
        chunks += 1;
        stats.first_token.get_or_insert(started.elapsed());
        raw.push_str(delta);
        if tone.is_none() {
            tone = reply::tone_so_far(&raw);
        }
        let text = say.push(delta);
        for s in sentences.push(&text) {
            emit(s, tone, &mut stats);
        }
        if say.finished()
            && let Some(rest) = sentences.finish()
        {
            emit(rest, tone, &mut stats);
        }
    }
    stats.total = started.elapsed();
    let reply = reply::parse_reply(&raw);
    // A plain-text answer (no schema support) never went through `say`: send it whole.
    if !say.finished() {
        let rest = sentences.finish();
        let pieces = if reply.valid_json {
            rest.into_iter().collect()
        } else {
            cut_all(&reply.say)
        };
        for s in pieces {
            emit(s, Some(reply.tone), &mut stats);
        }
    }
    stats.completion_tokens.get_or_insert(chunks);
    if stats.tokens_per_second.is_none()
        && let Some(first) = stats.first_token
    {
        let secs = (stats.total - first).as_secs_f64();
        stats.tokens_per_second = (secs > 0.0).then(|| chunks.saturating_sub(1) as f64 / secs);
    }
    Ok((reply, raw, stats))
}

fn cut_all(text: &str) -> Vec<String> {
    let mut s = Sentences::new();
    let mut out = s.push(text);
    out.extend(s.finish());
    out
}

/// llama.cpp's `timings` (last chunk) and the OpenAI `usage` (Ollama, LM Studio, cloud).
fn read_stats(chunk: &Value, stats: &mut Stats) {
    if let Some(t) = chunk.get("timings") {
        stats.prompt_tokens = t
            .get("prompt_n")
            .and_then(Value::as_u64)
            .or(stats.prompt_tokens);
        stats.cached_tokens = t
            .get("cache_n")
            .and_then(Value::as_u64)
            .or(stats.cached_tokens);
        stats.completion_tokens = t
            .get("predicted_n")
            .and_then(Value::as_u64)
            .or(stats.completion_tokens);
        stats.tokens_per_second = t
            .get("predicted_per_second")
            .and_then(Value::as_f64)
            .or(stats.tokens_per_second);
        stats.prompt_ms = t
            .get("prompt_ms")
            .and_then(Value::as_f64)
            .or(stats.prompt_ms);
    }
    if let Some(u) = chunk.get("usage").filter(|u| u.is_object()) {
        stats.prompt_tokens = stats
            .prompt_tokens
            .or(u.get("prompt_tokens").and_then(Value::as_u64));
        stats.completion_tokens = stats
            .completion_tokens
            .or(u.get("completion_tokens").and_then(Value::as_u64));
    }
}

fn error_message(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    v.pointer("/error/message")
        .and_then(Value::as_str)
        .map(String::from)
}

/// Another AI program already running on this PC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detected {
    pub provider: Provider,
    pub base_url: String,
    /// The models it offers, by the id requests take.
    pub models: Vec<String>,
}

/// Looks for Ollama and LM Studio on their default ports (a fraction of a second when neither runs).
/// Blocking: call it from a worker, or use [`detect_in_background`].
pub fn detect_local_servers() -> Vec<Detected> {
    [Provider::Ollama, Provider::LmStudio]
        .into_iter()
        .filter_map(|provider| {
            let base_url = provider.default_base_url()?.to_owned();
            let models = list_models(&base_url, None, Duration::from_millis(400)).ok()?;
            Some(Detected {
                provider,
                base_url,
                models,
            })
        })
        .collect()
}

pub fn detect_in_background() -> Receiver<Vec<Detected>> {
    let (send, recv) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(detect_local_servers());
    });
    recv
}

/// `GET /models`: the ids an OpenAI-compatible server offers.
pub fn list_models(
    base_url: &str,
    api_key: Option<&str>,
    timeout: Duration,
) -> Result<Vec<String>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(timeout)
        .timeout(timeout * 3)
        .build();
    let mut call = agent.get(&format!("{}/models", base_url.trim_end_matches('/')));
    if let Some(key) = api_key {
        call = call.set("Authorization", &format!("Bearer {key}"));
    }
    let text = call
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())?;
    let body: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(body
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| m.get("id").and_then(Value::as_str).map(String::from))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    /// Serves one canned SSE response per connection and records the request bodies.
    fn sse_server(events: Vec<String>) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let bodies = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = bodies.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0;
                let mut path = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if path.is_empty() {
                        path = line.clone();
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0; length];
                std::io::Read::read_exact(&mut reader, &mut body).unwrap();
                seen.lock().unwrap().push(String::from_utf8(body).unwrap());
                if path.starts_with("GET /v1/models") {
                    let json =
                        r#"{"object":"list","data":[{"id":"qwen3.5:4b"},{"id":"gemma4:12b"}]}"#;
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{json}",
                        json.len()
                    );
                    continue;
                }
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
                );
                for e in &events {
                    let _ = write!(stream, "data: {e}\n\n");
                    let _ = stream.flush();
                }
                let _ = write!(stream, "data: [DONE]\n\n");
            }
        });
        (url, bodies)
    }

    fn deltas(reply: &str, size: usize) -> Vec<String> {
        let chars: Vec<char> = reply.chars().collect();
        let mut events: Vec<String> = chars
            .chunks(size)
            .map(|c| {
                let piece: String = c.iter().collect();
                serde_json::json!({"choices":[{"index":0,"delta":{"content":piece}}]}).to_string()
            })
            .collect();
        events.push(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"timings":{"prompt_n":42,"cache_n":900,"predicted_n":31,"predicted_per_second":88.5,"prompt_ms":35.2}}"#.into());
        events
    }

    fn local(url: String) -> Endpoint {
        Endpoint {
            provider: Provider::Local,
            base_url: url,
            api_key: Some("secret".into()),
            model: "horizon-companion".into(),
        }
    }

    #[test]
    fn streams_sentences_then_the_parsed_reply() {
        let json = r#"{"tone":"sarcastic","say":"Oh great, a creeper. Lovely. I'll handle it.","actions":[{"skill":"attack","target":"creeper"}]}"#;
        let (url, bodies) = sse_server(deltas(json, 3));
        let mut brain = Brain::new(local(url));
        let id = brain.ask(ChatRequest::turn(vec![Message::new(Role::User, "help")]));
        let mut sentences = Vec::new();
        let done = loop {
            match brain.try_recv() {
                Some(Event::Sentence {
                    id: got,
                    text,
                    tone,
                }) => {
                    assert_eq!(got, id);
                    // Known from the first sentence on: the voice starts in the right delivery.
                    assert_eq!(tone, Tone::Sarcastic);
                    sentences.push(text);
                }
                Some(Event::Done {
                    reply, stats, raw, ..
                }) => break (reply, stats, raw),
                Some(Event::Failed { error, .. }) => panic!("{error}"),
                None => std::thread::sleep(Duration::from_millis(2)),
            }
        };
        assert_eq!(
            sentences,
            ["Oh great, a creeper.", "Lovely.", "I'll handle it."]
        );
        let (reply, stats, raw) = done;
        assert_eq!(raw, json);
        assert_eq!(reply.actions[0].skill, reply::Skill::Attack);
        assert_eq!(
            (
                stats.prompt_tokens,
                stats.cached_tokens,
                stats.completion_tokens
            ),
            (Some(42), Some(900), Some(31))
        );
        assert!(stats.first_token.unwrap() <= stats.first_sentence.unwrap());
        let body: Value = serde_json::from_str(&bodies.lock().unwrap()[0]).unwrap();
        assert_eq!(body["stream"], true);
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(body["response_format"]["type"], "json_schema");
    }

    #[test]
    fn the_schema_reaches_the_server_in_its_written_order() {
        let body = request_body(&local("http://x/v1".into()), &ChatRequest::turn(vec![]));
        let (tone, say, actions) = (
            body.find("\"tone\":{").unwrap(),
            body.find("\"say\":{").unwrap(),
            body.find("\"actions\":{").unwrap(),
        );
        assert!(tone < say && say < actions, "{body}");
        let cloud = Endpoint {
            provider: Provider::Cloud,
            ..local("https://api.example.com/v1".into())
        };
        let body: Value =
            serde_json::from_str(&request_body(&cloud, &ChatRequest::turn(vec![]))).unwrap();
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);
        assert!(
            body.get("chat_template_kwargs").is_none(),
            "cloud APIs reject unknown fields"
        );
    }

    #[test]
    fn plain_text_answers_still_reach_chat() {
        let (url, _) = sse_server(deltas("Sure thing! Following you.", 4));
        let endpoint = Endpoint {
            provider: Provider::Ollama,
            ..local(url)
        };
        let agent = agent_for(&endpoint);
        let mut said = Vec::new();
        let (reply, _, _) = stream_chat(
            &agent,
            &endpoint,
            &ChatRequest::turn(vec![]),
            &mut |s, _| said.push(s),
            &|| false,
        )
        .unwrap();
        assert!(!reply.valid_json);
        assert_eq!(said, ["Sure thing!", "Following you."]);
    }

    #[test]
    fn errors_and_cancels_are_reported() {
        let (url, _) = sse_server(vec![r#"{"error":{"message":"context full"}}"#.into()]);
        let mut brain = Brain::new(local(url.clone()));
        brain.ask(ChatRequest::turn(vec![]));
        let error = loop {
            match brain.try_recv() {
                Some(Event::Failed { error, .. }) => break error,
                Some(other) => panic!("{other:?}"),
                None => std::thread::sleep(Duration::from_millis(2)),
            }
        };
        assert_eq!(error, "context full");

        let (url, _) = sse_server(deltas(
            r#"{"tone":"normal","say":"one. two.","actions":[]}"#,
            1,
        ));
        let mut brain = Brain::new(local(url));
        let first = brain.ask(ChatRequest::turn(vec![]));
        brain.cancel_up_to(first);
        let second = brain.ask(ChatRequest::turn(vec![]));
        let got = loop {
            if let Some(e) = brain.try_recv() {
                break e;
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(got.id(), second, "the cancelled request sends nothing");
        let _ = brain.endpoint();
    }

    #[test]
    fn lists_models_like_ollama_and_lm_studio() {
        let (url, _) = sse_server(vec![]);
        assert_eq!(
            list_models(&url, None, Duration::from_secs(1)).unwrap(),
            ["qwen3.5:4b", "gemma4:12b"]
        );
        assert!(list_models("http://127.0.0.1:9/v1", None, Duration::from_millis(200)).is_err());
    }
}
