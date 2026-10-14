//! One OpenAI-compatible chat client, local first: LM Studio, Ollama,
//! llama.cpp's server, or any service speaking the same API
//! (`projects/shard/design/agent-player.md`). Plain HTTP over `ureq`, on the
//! caller's thread, which is never Tauri's main thread.
//!
//! A turn streams: text and reasoning reach the caller as they arrive, and
//! tool calls are put together from their deltas by index. The wall-clock
//! budget is checked on every line, and a global timeout on the request
//! catches a stream that stalls without sending one.

use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};

/// Where the model is, as Settings holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub base_url: String,
    pub model: String,
    pub sampling: Sampling,
}

/// One mode's sampling. `top_k` and `min_p` are extensions to the OpenAI
/// API that LM Studio, llama.cpp and Ollama accept.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Preset {
    pub temperature: f64,
    pub top_p: f64,
    pub top_k: u32,
    pub min_p: f64,
    pub presence_penalty: f64,
}

/// Sampling per mode. The defaults are Qwen3's recommended values
/// (huggingface.co/Qwen/Qwen3.6-35B-A3B, "Best Practices"): thinking
/// 0.6 / 0.95 / 20 / 0, non-thinking 0.7 / 0.8 / 20 / 0. Greedy or unset
/// sampling in thinking mode is what Qwen warns causes endless repetition;
/// considered adds a presence penalty of 1.0 against it (the card allows 0
/// to 2), quick none, since a note number repeats legitimately.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Sampling {
    pub quick: Preset,
    pub considered: Preset,
}

impl Default for Sampling {
    fn default() -> Self {
        Self {
            quick: Preset {
                temperature: 0.7,
                top_p: 0.8,
                top_k: 20,
                min_p: 0.0,
                presence_penalty: 0.0,
            },
            considered: Preset {
                temperature: 0.6,
                top_p: 0.95,
                top_k: 20,
                min_p: 0.0,
                presence_penalty: 1.0,
            },
        }
    }
}

impl Default for Preset {
    fn default() -> Self {
        Sampling::default().quick
    }
}

impl Sampling {
    pub fn for_mode(&self, quick: bool) -> &Preset {
        if quick {
            &self.quick
        } else {
            &self.considered
        }
    }
}

impl Default for Settings {
    /// LM Studio, and whatever model it has loaded.
    fn default() -> Self {
        Self {
            base_url: "http://localhost:1234/v1".into(),
            model: String::new(),
            sampling: Sampling::default(),
        }
    }
}

/// Whether a model setting means "whatever is loaded".
pub fn is_auto(model: &str) -> bool {
    let m = model.trim();
    m.is_empty() || m.eq_ignore_ascii_case("auto")
}

/// Where LM Studio lists its models with their state: its own REST API,
/// beside the OpenAI-compatible one.
pub fn models_url(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    let root = base.strip_suffix("/v1").unwrap_or(base);
    format!("{root}/api/v0/models")
}

/// The model a reply asks, from LM Studio's listing (`/api/v0/models`), or
/// none when the server has no such listing (Ollama, llama.cpp).
///
/// LM Studio loads a model a request names if it isn't loaded, 20 GB or
/// more, so a model that isn't loaded is never named: only one listed as
/// `loaded`, of type `llm` or `vlm`. "Auto" (or empty) takes the first such;
/// a named model is taken only while it is loaded. With no listing, the
/// named model is taken on trust.
pub fn pick_model(listing: Option<&Json>, configured: &str) -> Result<String, String> {
    let configured = configured.trim();
    let Some(listing) = listing else {
        if is_auto(configured) {
            return Err(
                "No model is set, and the server doesn't say which one is loaded. Name one in Settings, then ask again."
                    .into(),
            );
        }
        return Ok(configured.to_string());
    };
    let loaded: Vec<&str> = listing
        .get("data")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter(|m| matches!(m.get("type").and_then(Json::as_str), Some("llm" | "vlm")))
        .filter(|m| m.get("state").and_then(Json::as_str) == Some("loaded"))
        .filter_map(|m| m.get("id").and_then(Json::as_str))
        .collect();
    if is_auto(configured) {
        return loaded.first().map(|m| m.to_string()).ok_or_else(|| {
            "No model loaded in LM Studio. Load one there, then ask again.".to_string()
        });
    }
    if loaded.contains(&configured) {
        Ok(configured.to_string())
    } else {
        Err(format!(
            "{configured} isn't loaded in LM Studio. Load it there, then ask again."
        ))
    }
}

/// Something streamed before the turn ends.
#[derive(Debug, Clone, PartialEq)]
pub enum Delta {
    Text(String),
    Reasoning(String),
}

/// A tool call, put together from its deltas.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// One turn's answer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Turn {
    pub text: String,
    pub calls: Vec<Call>,
    /// `stop`, `tool_calls`, `length`, as the server said.
    pub finish: Option<String>,
}

/// Why a turn ended before its answer did.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    Cancelled,
    /// The reply's wall-clock budget ran out.
    Time,
    /// The model's own text was going round in circles (`Watch`).
    Looping,
    /// The server couldn't be reached or answered with an error.
    Http(String),
}

/// What answers a turn: the server, or a script in a test.
pub trait Model {
    /// The model id to ask, from the setting: by default the setting itself.
    fn resolve(&mut self, configured: &str) -> Result<String, String> {
        Ok(configured.trim().to_string())
    }

    fn turn(
        &mut self,
        body: &Json,
        deadline: Instant,
        cancel: &AtomicBool,
        on: &mut dyn FnMut(Delta),
    ) -> Result<Turn, Failure>;
}

/// A request's body. `quick` turns reasoning off; considered leaves the
/// server's default. `sampling` is that mode's preset. Never a `response_format`: a JSON schema forces the
/// reasoning block into JSON on LM Studio and the answer comes back empty.
pub fn body(
    model: &str,
    messages: &[Json],
    tools: &Json,
    quick: bool,
    max_tokens: u32,
    sampling: &Preset,
) -> Json {
    let mut b = json!({
        "model": model,
        "messages": messages,
        "tools": tools,
        "stream": true,
        "stream_options": { "include_usage": true },
        "max_tokens": max_tokens,
        "temperature": sampling.temperature,
        "top_p": sampling.top_p,
        "top_k": sampling.top_k,
        "min_p": sampling.min_p,
        "presence_penalty": sampling.presence_penalty,
    });
    if quick {
        b["reasoning_effort"] = json!("none");
    }
    b
}

/// Watches one stream of text for going in circles. The text is lower-cased
/// with its whitespace collapsed, so a cycle that differs in line breaks
/// still matches. It loops when the last `TAIL` characters have already
/// occurred five times before in it: six passes over the same 400 characters.
/// A model wrapping up may restate a whole plan, JSON and all, two or
/// three times (seen live, and wrongly stopped at three passes); a real
/// degenerate loop cycles dozens of times, so six passes is the line.
/// Reasoning does not say 400 characters six times; a cycle of the real failure ("One check: ... I will output
/// the calls. One thought: ...", a dozen lines) is caught on its third
/// pass, not at the token cap.
#[derive(Debug, Default)]
pub struct Watch {
    seen: String,
    /// Whether the last character pushed was whitespace.
    space: bool,
    checked: usize,
}

impl Watch {
    const TAIL: usize = 400;
    /// How much new text to gather between checks.
    const EVERY: usize = 100;

    /// Adds text; true when the whole is now looping.
    pub fn push(&mut self, text: &str) -> bool {
        for c in text.chars() {
            if c.is_whitespace() {
                if !self.space && !self.seen.is_empty() {
                    self.seen.push(' ');
                }
                self.space = true;
            } else {
                self.seen.extend(c.to_lowercase());
                self.space = false;
            }
        }
        if self.seen.len() < Self::TAIL * 6 || self.seen.len() < self.checked + Self::EVERY {
            return false;
        }
        self.checked = self.seen.len();
        looping(&self.seen)
    }
}

/// Whether `text` (already normalised) ends with a 400-character stretch it
/// has already held five times before.
fn looping(text: &str) -> bool {
    let mut start = text.len() - Watch::TAIL;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let (before, tail) = text.split_at(start);
    before.matches(tail).take(5).count() >= 5
}

/// Puts a streamed answer together, one `data:` line at a time.
#[derive(Debug, Default)]
pub struct Stream {
    text: String,
    /// By the index the server gives each call.
    calls: Vec<(u64, Call)>,
    finish: Option<String>,
    done: bool,
}

impl Stream {
    /// Reads one line of the event stream. Anything that isn't a `data:`
    /// line is skipped, and so is a chunk that doesn't parse.
    pub fn line(&mut self, line: &str, on: &mut dyn FnMut(Delta)) {
        let Some(data) = line.trim().strip_prefix("data:") else {
            return;
        };
        let data = data.trim();
        if data == "[DONE]" {
            self.done = true;
            return;
        }
        let Ok(chunk) = serde_json::from_str::<Json>(data) else {
            return;
        };
        for choice in chunk
            .get("choices")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(f) = choice.get("finish_reason").and_then(Json::as_str) {
                self.finish = Some(f.to_string());
            }
            let Some(delta) = choice.get("delta") else {
                continue;
            };
            let reasoning = delta
                .get("reasoning_content")
                .or_else(|| delta.get("reasoning"))
                .and_then(Json::as_str);
            if let Some(r) = reasoning.filter(|r| !r.is_empty()) {
                on(Delta::Reasoning(r.to_string()));
            }
            if let Some(c) = delta
                .get("content")
                .and_then(Json::as_str)
                .filter(|c| !c.is_empty())
            {
                self.text.push_str(c);
                on(Delta::Text(c.to_string()));
            }
            for tc in delta
                .get("tool_calls")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
            {
                let index = tc.get("index").and_then(Json::as_u64).unwrap_or(0);
                let at = match self.calls.iter().position(|(i, _)| *i == index) {
                    Some(at) => at,
                    None => {
                        self.calls.push((index, Call::default()));
                        self.calls.len() - 1
                    }
                };
                let call = &mut self.calls[at].1;
                if let Some(id) = tc.get("id").and_then(Json::as_str) {
                    if call.id.is_empty() {
                        call.id = id.to_string();
                    }
                }
                if let Some(f) = tc.get("function") {
                    if let Some(n) = f.get("name").and_then(Json::as_str) {
                        call.name.push_str(n);
                    }
                    if let Some(a) = f.get("arguments").and_then(Json::as_str) {
                        call.arguments.push_str(a);
                    }
                }
            }
        }
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    pub fn finish(mut self) -> Turn {
        self.calls.sort_by_key(|(i, _)| *i);
        Turn {
            text: self.text,
            calls: self.calls.into_iter().map(|(_, c)| c).collect(),
            finish: self.finish,
        }
    }
}

/// The server, over plain HTTP.
pub struct Http {
    agent: ureq::Agent,
    url: String,
    models: String,
}

impl Http {
    pub fn new(settings: &Settings) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(5)))
            // The body says what went wrong; read it rather than an error.
            .http_status_as_error(false)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            url: format!(
                "{}/chat/completions",
                settings.base_url.trim().trim_end_matches('/')
            ),
            models: models_url(&settings.base_url),
        }
    }

    /// LM Studio's listing of its models with their state, or none when the
    /// server has no such listing or can't be reached.
    fn listing(&self) -> Option<Json> {
        let mut response = self
            .agent
            .get(&self.models)
            .config()
            .timeout_global(Some(Duration::from_secs(5)))
            .build()
            .call()
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let text = response.body_mut().read_to_string().ok()?;
        serde_json::from_str::<Json>(&text)
            .ok()
            .filter(|j| j.get("data").is_some_and(Json::is_array))
    }
}

impl Model for Http {
    fn resolve(&mut self, configured: &str) -> Result<String, String> {
        pick_model(self.listing().as_ref(), configured)
    }

    fn turn(
        &mut self,
        body: &Json,
        deadline: Instant,
        cancel: &AtomicBool,
        on: &mut dyn FnMut(Delta),
    ) -> Result<Turn, Failure> {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Failure::Time);
        }
        let late = || Instant::now() >= deadline;
        let response = self
            .agent
            .post(&self.url)
            .config()
            .timeout_global(Some(left))
            .build()
            .header("Content-Type", "application/json")
            .send(body.to_string())
            .map_err(|e| {
                if late() {
                    Failure::Time
                } else {
                    Failure::Http(format!("couldn't reach the model at {}: {e}", self.url))
                }
            })?;
        let status = response.status();
        let mut reader = BufReader::new(response.into_body().into_reader());
        if !status.is_success() {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut reader, &mut text);
            let message = serde_json::from_str::<Json>(&text)
                .ok()
                .and_then(|j| {
                    j.pointer("/error/message")
                        .or_else(|| j.get("error"))
                        .and_then(Json::as_str)
                        .map(str::to_string)
                })
                .unwrap_or(text);
            return Err(Failure::Http(format!(
                "the model answered {}: {}",
                status.as_u16(),
                message.trim()
            )));
        }
        let mut stream = Stream::default();
        let (mut thinking, mut saying) = (Watch::default(), Watch::default());
        let mut line = String::new();
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(Failure::Cancelled);
            }
            if late() {
                return Err(Failure::Time);
            }
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    let mut looped = false;
                    stream.line(&line, &mut |d| {
                        looped |= match &d {
                            Delta::Reasoning(r) => thinking.push(r),
                            Delta::Text(t) => saying.push(t),
                        };
                        on(d)
                    });
                    if looped {
                        return Err(Failure::Looping);
                    }
                    if stream.is_done() {
                        break;
                    }
                }
                Err(_) if late() => return Err(Failure::Time),
                Err(e) => return Err(Failure::Http(format!("the stream broke: {e}"))),
            }
        }
        Ok(stream.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(lines: &[&str]) -> (Turn, Vec<Delta>) {
        let mut s = Stream::default();
        let mut seen = Vec::new();
        for l in lines {
            s.line(l, &mut |d| seen.push(d));
        }
        (s.finish(), seen)
    }

    #[test]
    fn tool_call_arguments_split_across_chunks_are_put_back_together() {
        let (turn, _) = read(&[
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"set_drums","arguments":""}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"hits\": [{\"st"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ep\": 0, \"midi_note\": 36}]}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            "data: [DONE]",
        ]);
        assert_eq!(turn.calls.len(), 1);
        assert_eq!(turn.calls[0].id, "call_a");
        assert_eq!(turn.calls[0].name, "set_drums");
        let args: Json = serde_json::from_str(&turn.calls[0].arguments).unwrap();
        assert_eq!(args["hits"][0]["midi_note"], 36);
        assert_eq!(turn.finish.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn two_calls_interleaved_keep_their_own_arguments_in_index_order() {
        let (turn, _) = read(&[
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"b","function":{"name":"set_melody","arguments":"{\"steps\""}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"set_tempo","arguments":"{\"bpm\":"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"arguments":": []}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":" 90}"}}]}}]}"#,
        ]);
        let names: Vec<&str> = turn.calls.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["set_tempo", "set_melody"]);
        assert_eq!(turn.calls[0].arguments, r#"{"bpm": 90}"#);
        assert_eq!(turn.calls[1].arguments, r#"{"steps": []}"#);
    }

    #[test]
    fn text_and_reasoning_stream_as_they_arrive_and_noise_is_skipped() {
        let (turn, seen) = read(&[
            ": keep-alive",
            "",
            r#"data: {"choices":[{"delta":{"reasoning_content":"Four on "}}]}"#,
            r#"data: {"choices":[{"delta":{"reasoning":"the floor."}}]}"#,
            r#"data: {"choices":[{"delta":{"content":"Done"}}]}"#,
            "data: not json",
            r#"data: {"choices":[{"delta":{"content":"."},"finish_reason":"stop"}],"usage":null}"#,
        ]);
        assert_eq!(
            seen,
            vec![
                Delta::Reasoning("Four on ".into()),
                Delta::Reasoning("the floor.".into()),
                Delta::Text("Done".into()),
                Delta::Text(".".into()),
            ]
        );
        assert_eq!(turn.text, "Done.");
        assert!(turn.calls.is_empty());
        assert_eq!(turn.finish.as_deref(), Some("stop"));
    }

    fn listing() -> Json {
        json!({ "object": "list", "data": [
            { "id": "text-embedding-qwen3-embedding-4b", "type": "embeddings", "state": "loaded" },
            { "id": "prism-ml/bonsai-27b", "type": "llm", "state": "not-loaded" },
            { "id": "qwen/qwen3.6-35b-a3b", "type": "llm", "state": "loaded" },
            { "id": "some/vision", "type": "vlm", "state": "loaded" },
        ] })
    }

    #[test]
    fn auto_takes_the_first_loaded_chat_model_and_never_one_that_isnt_loaded() {
        let l = listing();
        assert_eq!(pick_model(Some(&l), "").unwrap(), "qwen/qwen3.6-35b-a3b");
        assert_eq!(
            pick_model(Some(&l), "Auto").unwrap(),
            "qwen/qwen3.6-35b-a3b"
        );
        assert_eq!(pick_model(Some(&l), "some/vision").unwrap(), "some/vision");
        let e = pick_model(Some(&l), "prism-ml/bonsai-27b").unwrap_err();
        assert_eq!(
            e,
            "prism-ml/bonsai-27b isn't loaded in LM Studio. Load it there, then ask again."
        );
        // An embeddings model is loaded, but it can't chat.
        let none = json!({ "data": [
            { "id": "text-embedding-qwen3-embedding-4b", "type": "embeddings", "state": "loaded" },
            { "id": "prism-ml/bonsai-27b", "type": "llm", "state": "not-loaded" },
        ] });
        assert_eq!(
            pick_model(Some(&none), "").unwrap_err(),
            "No model loaded in LM Studio. Load one there, then ask again."
        );
    }

    #[test]
    fn without_a_listing_the_named_model_is_taken_on_trust() {
        assert_eq!(pick_model(None, " llama3 ").unwrap(), "llama3");
        assert!(pick_model(None, "")
            .unwrap_err()
            .contains("Name one in Settings"));
        assert_eq!(
            models_url("http://localhost:1234/v1/"),
            "http://localhost:1234/api/v0/models"
        );
        assert_eq!(
            models_url("http://localhost:11434"),
            "http://localhost:11434/api/v0/models"
        );
    }

    #[test]
    fn quick_turns_reasoning_off_and_considered_leaves_it() {
        let tools = json!([]);
        let p = Sampling::default();
        let quick = body("m", &[], &tools, true, 8000, &p.quick);
        assert_eq!(quick["reasoning_effort"], "none");
        let considered = body("m", &[], &tools, false, 8000, &p.considered);
        assert!(considered.get("reasoning_effort").is_none());
        assert!(considered.get("response_format").is_none());
        assert_eq!(considered["max_tokens"], 8000);
    }

    #[test]
    fn each_mode_sends_its_own_sampling() {
        let tools = json!([]);
        let p = Sampling::default();
        let q = body("m", &[], &tools, true, 8000, &p.quick);
        let c = body("m", &[], &tools, false, 8000, &p.considered);
        assert_eq!(q["temperature"], 0.7);
        assert_eq!(q["top_p"], 0.8);
        assert_eq!(q["presence_penalty"], 0.0);
        assert_eq!(c["temperature"], 0.6);
        assert_eq!(c["top_p"], 0.95);
        assert_eq!(c["top_k"], 20);
        assert_eq!(c["min_p"], 0.0);
        assert_eq!(c["presence_penalty"], 1.0);
        // The same bytes every turn of a reply.
        assert_eq!(
            c.to_string(),
            body("m", &[], &tools, false, 8000, &p.considered).to_string()
        );
        // A settings file from before sampling still loads.
        let s: Settings = serde_json::from_str(r#"{"baseUrl":"http://x/v1","model":"m"}"#).unwrap();
        assert_eq!(s.sampling, p);
    }

    /// The failure seen live: the plan settled, then a dozen lines said
    /// over and over until the token cap.
    const CYCLE: &str = "One check: \"track 1\" -> track: \"Track 1\". This is correct.\nI will output the calls.\nOne thought: the user said baseline, so a bass line.\nOne more thought: set_tempo 175 first.\nThen set_melody with Track 1 and ten notes.\nWait, is G1 31? Yes, G1 = 31.\nNotes: 31, 31, 34, 31, 38, 31, 34, 31, 43, 31.\nHold values short for speedy.\nVelocity high for death metal.\nAll good.\nLet me double check the track name once more.\nOne check: ";

    #[test]
    fn a_cycle_of_the_same_lines_is_caught_after_many_passes() {
        let mut w = Watch::default();
        let mut caught = None;
        for pass in 1..=12 {
            // Streamed in small pieces, as tokens arrive.
            for piece in CYCLE.as_bytes().chunks(7) {
                if w.push(std::str::from_utf8(piece).unwrap()) {
                    caught = Some(pass);
                    break;
                }
            }
            if caught.is_some() {
                break;
            }
        }
        assert!(matches!(caught, Some(6..=9)), "caught on pass {caught:?}");
    }

    #[test]
    fn a_plan_restated_three_times_is_not_a_loop() {
        let plan = "[Tool Calls] set_tempo {\"bpm\": 175} set_melody {\"track\": \"Track 1\", \"length\": 16, \"steps\": [{\"step\": 1, \"note\": 43, \"velocity\": 127, \"hold\": 1}, {\"step\": 2, \"note\": 43, \"velocity\": 110, \"hold\": 1}, {\"step\": 4, \"note\": 41, \"velocity\": 120, \"hold\": 2}, {\"step\": 6, \"note\": 44, \"velocity\": 100, \"hold\": 1}, {\"step\": 9, \"note\": 43, \"velocity\": 127, \"hold\": 1}, {\"step\": 12, \"note\": 41, \"velocity\": 95, \"hold\": 2}, {\"step\": 14, \"note\": 44, \"velocity\": 105, \"hold\": 1}, {\"step\": 15, \"note\": 43, \"velocity\": 127, \"hold\": 1}]} ";
        assert!(plan.len() > 450);
        let mut w = Watch::default();
        let mut text = String::from("Planning the bass in G with chromatic neighbours. ");
        for stage in ["Output Generation", "Proceeds", "Done"] {
            text.push_str(stage);
            text.push_str(". ");
            text.push_str(plan);
        }
        for piece in text.as_bytes().chunks(9) {
            assert!(!w.push(std::str::from_utf8(piece).unwrap()));
        }
    }

    #[test]
    fn long_reasoning_that_reuses_its_phrases_is_not_a_loop() {
        let mut w = Watch::default();
        let mut text = String::new();
        for i in 0..200 {
            text.push_str(&format!(
                "Step {i}: the bass needs G1 = 31 and the hold is {} steps, so the note at step {} stays short. Check the tempo of 175 again. ",
                i % 4 + 1,
                i * 3 % 16 + 1
            ));
        }
        assert!(text.len() > 20_000);
        for piece in text.as_bytes().chunks(11) {
            assert!(!w.push(std::str::from_utf8(piece).unwrap()));
        }
    }
}
