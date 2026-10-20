//! What the brain answers: `{"tone": …, "say": …, "actions": […], "remember": …}`, constrained by a JSON
//! schema, parsed leniently, and streamed: [`SayStream`] pulls the `say` text out of the partial JSON as
//! it arrives and [`Sentences`] cuts it into sentences, so chat shows and the voice speaks the first
//! sentence while the rest is still being generated.
//!
//! `tone` comes first (a few tokens) so the voice knows how to deliver the first sentence; `say` comes
//! next, before the actions.

use serde_json::Value;

/// The skills the brain may ask for; the game's skills check and carry them out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Skill {
    Follow,
    Stay,
    Come,
    GoTo,
    Stop,
    Attack,
    Defend,
    HoldFire,
    Fire,
    Jump,
    Crouch,
    StandUp,
    Collect,
    Give,
    Eat,
    Sleep,
    Craft,
}

impl Skill {
    pub const ALL: [Skill; 17] = [
        Skill::Follow,
        Skill::Stay,
        Skill::Come,
        Skill::GoTo,
        Skill::Stop,
        Skill::Attack,
        Skill::Defend,
        Skill::HoldFire,
        Skill::Fire,
        Skill::Jump,
        Skill::Crouch,
        Skill::StandUp,
        Skill::Collect,
        Skill::Give,
        Skill::Eat,
        Skill::Sleep,
        Skill::Craft,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Skill::Follow => "follow",
            Skill::Stay => "stay",
            Skill::Come => "come",
            Skill::GoTo => "go_to",
            Skill::Stop => "stop",
            Skill::Attack => "attack",
            Skill::Defend => "defend",
            Skill::HoldFire => "hold_fire",
            Skill::Fire => "fire",
            Skill::Jump => "jump",
            Skill::Crouch => "crouch",
            Skill::StandUp => "stand_up",
            Skill::Collect => "collect",
            Skill::Give => "give",
            Skill::Eat => "eat",
            Skill::Sleep => "sleep",
            Skill::Craft => "craft",
        }
    }

    pub fn from_name(name: &str) -> Option<Skill> {
        Skill::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// How `say` is delivered by the voice (Qwen3-TTS takes it as an instruction; Pocket TTS maps it to
/// volume and pace) and, in chat, by nothing at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tone {
    #[default]
    Normal,
    Excited,
    Shout,
    Whisper,
    Laugh,
    Sarcastic,
    Worried,
}

impl Tone {
    pub const ALL: [Tone; 7] = [
        Tone::Normal,
        Tone::Excited,
        Tone::Shout,
        Tone::Whisper,
        Tone::Laugh,
        Tone::Sarcastic,
        Tone::Worried,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tone::Normal => "normal",
            Tone::Excited => "excited",
            Tone::Shout => "shout",
            Tone::Whisper => "whisper",
            Tone::Laugh => "laugh",
            Tone::Sarcastic => "sarcastic",
            Tone::Worried => "worried",
        }
    }

    pub fn from_name(name: &str) -> Option<Tone> {
        Tone::ALL.into_iter().find(|t| t.name() == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub skill: Skill,
    /// An item or block id (`oak_log`) for collect, give and craft.
    pub item: Option<String>,
    pub count: Option<u32>,
    /// go_to: `crosshair`, `owner` or a block id; attack: a mob id or `nearest`.
    pub target: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    pub tone: Tone,
    pub say: String,
    /// At most [`MAX_ACTIONS`].
    pub actions: Vec<Action>,
    /// Actions that name no known skill, for the brain to hear about.
    pub rejected: Vec<String>,
    /// A note for the memory.
    pub remember: Option<String>,
    /// Whether the text was the JSON object asked for (the benchmark's validity rate).
    pub valid_json: bool,
}

pub const MAX_ACTIONS: usize = 3;
pub const MAX_COUNT: u32 = 256;
pub const MAX_SAY_CHARS: usize = 240;

/// The schema for runtimes that turn it into a grammar (llama.cpp, LM Studio, Ollama). Properties are
/// written in the order the model must produce them; it is sent as written, never re-serialised.
pub const SCHEMA: &str = r#"{"type":"object","properties":{"tone":{"type":"string","enum":["normal","excited","shout","whisper","laugh","sarcastic","worried"]},"say":{"type":"string","maxLength":240},"actions":{"type":"array","maxItems":3,"items":{"type":"object","properties":{"skill":{"type":"string","enum":["follow","stay","come","go_to","stop","attack","defend","hold_fire","fire","jump","crouch","stand_up","collect","give","eat","sleep","craft"]},"item":{"type":"string"},"count":{"type":"integer","minimum":1,"maximum":256},"target":{"type":"string"}},"required":["skill"],"additionalProperties":false}},"remember":{"type":"string"}},"required":["tone","say","actions"],"additionalProperties":false}"#;

/// OpenAI's strict mode: every property required (optional ones nullable), no length or range limits.
pub const SCHEMA_STRICT: &str = r#"{"type":"object","properties":{"tone":{"type":"string","enum":["normal","excited","shout","whisper","laugh","sarcastic","worried"]},"say":{"type":"string"},"actions":{"type":"array","items":{"type":"object","properties":{"skill":{"type":"string","enum":["follow","stay","come","go_to","stop","attack","defend","hold_fire","fire","jump","crouch","stand_up","collect","give","eat","sleep","craft"]},"item":{"type":["string","null"]},"count":{"type":["integer","null"]},"target":{"type":["string","null"]}},"required":["skill","item","count","target"],"additionalProperties":false}},"remember":{"type":["string","null"]}},"required":["tone","say","actions","remember"],"additionalProperties":false}"#;

/// Parses a whole reply. Models without a schema sometimes wrap the JSON in a code fence, think aloud
/// first, or answer in plain text; plain text becomes `say` with no actions.
pub fn parse_reply(text: &str) -> Reply {
    let text = strip_thinking(text).trim();
    let json = match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if a < b => serde_json::from_str::<Value>(&text[a..=b]).ok(),
        _ => None,
    };
    let Some(Value::Object(map)) = json else {
        let say: String = text.trim_matches('`').trim().chars().take(MAX_SAY_CHARS).collect();
        return Reply {
            say,
            ..Reply::default()
        };
    };
    let mut reply = Reply {
        tone: map.get("tone").and_then(Value::as_str).and_then(Tone::from_name).unwrap_or_default(),
        say: map
            .get("say")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .chars()
            .take(MAX_SAY_CHARS)
            .collect(),
        remember: map
            .get("remember")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from),
        valid_json: true,
        ..Reply::default()
    };
    for action in map.get("actions").and_then(Value::as_array).into_iter().flatten() {
        let name = action.get("skill").and_then(Value::as_str).unwrap_or_default();
        let Some(skill) = Skill::from_name(name) else {
            reply.rejected.push(if name.is_empty() { action.to_string() } else { name.to_owned() });
            continue;
        };
        if reply.actions.len() == MAX_ACTIONS {
            reply.rejected.push(format!("{name} (more than {MAX_ACTIONS} actions)"));
            continue;
        }
        let text = |key| {
            action
                .get(key)
                .and_then(Value::as_str)
                .map(|s| s.trim().trim_start_matches("minecraft:").to_owned())
                .filter(|s| !s.is_empty())
        };
        reply.actions.push(Action {
            skill,
            item: text("item"),
            count: action
                .get("count")
                .and_then(Value::as_u64)
                .map(|n| n.clamp(1, u64::from(MAX_COUNT)) as u32),
            target: text("target"),
        });
    }
    reply
}

/// Drops a leading `<think>…</think>` block (a model that thinks despite being asked not to).
fn strip_thinking(text: &str) -> &str {
    let trimmed = text.trim_start();
    if let Some(rest) = trimmed.strip_prefix("<think>") {
        return rest.split_once("</think>").map_or("", |(_, after)| after);
    }
    text
}

/// Pulls the top-level `say` string out of a JSON object while it streams in, unescaping as it goes.
#[derive(Debug, Default)]
pub struct SayStream {
    started: bool,
    pending: String,
    depth: u32,
    in_string: bool,
    escape: Option<String>,
    high_surrogate: Option<u32>,
    /// At depth 1, the next string is a key.
    key_next: bool,
    capturing_key: bool,
    key: String,
    last_key: String,
    in_say: bool,
    done: bool,
}

impl SayStream {
    pub fn new() -> SayStream {
        SayStream::default()
    }

    /// Feeds more of the reply; returns the `say` text it completes.
    pub fn push(&mut self, chunk: &str) -> String {
        let mut out = String::new();
        let mut text = chunk;
        let joined;
        if !self.started {
            self.pending.push_str(chunk);
            let trimmed = self.pending.trim_start();
            if trimmed.is_empty() || "<think>".starts_with(trimmed) {
                return out;
            }
            if let Some(rest) = trimmed.strip_prefix("<think>") {
                match rest.split_once("</think>") {
                    Some((_, after)) => joined = after.to_owned(),
                    None => return out,
                }
            } else {
                joined = trimmed.to_owned();
            }
            self.started = true;
            self.pending.clear();
            text = &joined;
        }
        for c in text.chars() {
            self.feed(c, &mut out);
        }
        out
    }

    /// Whether the whole `say` string has been read.
    pub fn finished(&self) -> bool {
        self.done
    }

    fn feed(&mut self, c: char, out: &mut String) {
        if self.in_string {
            if let Some(esc) = &mut self.escape {
                esc.push(c);
                let esc_now = esc.clone();
                if let Some(decoded) = unescape(&esc_now, &mut self.high_surrogate) {
                    self.escape = None;
                    if let Some(ch) = decoded {
                        self.string_char(ch, out);
                    }
                }
                return;
            }
            match c {
                '\\' => self.escape = Some(String::new()),
                '"' => self.end_string(),
                _ => self.string_char(c, out),
            }
            return;
        }
        match c {
            '{' | '[' => {
                self.depth += 1;
                self.key_next = self.depth == 1 && c == '{';
            }
            '}' | ']' => self.depth = self.depth.saturating_sub(1),
            ',' if self.depth == 1 => self.key_next = true,
            '"' => {
                self.in_string = true;
                if self.depth == 1 && self.key_next {
                    self.capturing_key = true;
                    self.key.clear();
                } else if self.depth == 1 && self.last_key == "say" && !self.done {
                    self.in_say = true;
                }
            }
            _ => {}
        }
    }

    fn string_char(&mut self, c: char, out: &mut String) {
        if self.capturing_key {
            self.key.push(c);
        } else if self.in_say {
            out.push(c);
        }
    }

    fn end_string(&mut self) {
        self.in_string = false;
        if self.capturing_key {
            self.capturing_key = false;
            self.key_next = false;
            self.last_key = std::mem::take(&mut self.key);
        } else if self.in_say {
            self.in_say = false;
            self.done = true;
        }
    }
}

/// One escape sequence after the backslash: `None` while incomplete, `Some(None)` for half a surrogate pair.
fn unescape(esc: &str, high: &mut Option<u32>) -> Option<Option<char>> {
    let first = esc.chars().next()?;
    if first != 'u' {
        return Some(Some(match first {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            'b' => '\u{8}',
            'f' => '\u{c}',
            other => other,
        }));
    }
    if esc.len() < 5 {
        return None;
    }
    let code = u32::from_str_radix(&esc[1..5], 16).unwrap_or(0xfffd);
    if (0xd800..0xdc00).contains(&code) {
        *high = Some(code);
        return Some(None);
    }
    if (0xdc00..0xe000).contains(&code) {
        let pair = high.take().map(|h| 0x10000 + ((h - 0xd800) << 10) + (code - 0xdc00));
        return Some(Some(pair.and_then(char::from_u32).unwrap_or('\u{fffd}')));
    }
    Some(Some(char::from_u32(code).unwrap_or('\u{fffd}')))
}

/// Cuts streamed text into sentences for chat lines and the voice: after `.`, `!`, `?`, `…` and their
/// CJK, Arabic and Devanagari forms (with closing quotes or brackets) followed by a space, and at line
/// breaks. Decimals (`3.5`), single initials and common abbreviations (`Mr.`, `z.B.`, `etc.`) are not
/// ends. A run longer than [`Sentences::MAX`] is cut at its last comma or space.
#[derive(Debug, Default)]
pub struct Sentences {
    buf: String,
}

const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "st", "vs", "etc", "e.g", "i.e", "approx", "m", "mme", "mlle", "env", "p.ex",
    "sr", "sra", "srta", "dra", "ud", "uds", "z.b", "bzw", "ca", "nr", "usw", "dh", "d.h", "u.a", "sto",
    "no", "n.º",
];

impl Sentences {
    /// Longest piece without an end, in characters.
    pub const MAX: usize = 160;

    pub fn new() -> Sentences {
        Sentences::default()
    }

    /// Adds text; returns the sentences it completes, trimmed.
    pub fn push(&mut self, text: &str) -> Vec<String> {
        self.buf.push_str(text);
        let mut out = Vec::new();
        while let Some(end) = self.boundary() {
            let rest = self.buf.split_off(end);
            let sentence = std::mem::replace(&mut self.buf, rest);
            let sentence = sentence.trim();
            if !sentence.is_empty() {
                out.push(sentence.to_owned());
            }
        }
        out
    }

    /// The text left at the end of the stream.
    pub fn finish(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.buf);
        let rest = rest.trim();
        (!rest.is_empty()).then(|| rest.to_owned())
    }

    /// Byte index just past the first complete sentence.
    fn boundary(&self) -> Option<usize> {
        let chars: Vec<(usize, char)> = self.buf.char_indices().collect();
        let mut i = 0;
        while i < chars.len() {
            let (at, c) = chars[i];
            if c == '\n' {
                return Some(at + 1);
            }
            if is_terminal(c) {
                let mut j = i + 1;
                while j < chars.len() && (is_terminal(chars[j].1) || is_closer(chars[j].1)) {
                    j += 1;
                }
                // Needs the next character to know the sentence ended (CJK ends need none).
                let ends_here = match chars.get(j) {
                    None => false,
                    Some((_, next)) => next.is_whitespace() || is_cjk_terminal(c),
                };
                if ends_here && !(c == '.' && j == i + 1 && self.not_an_end(&chars, i)) {
                    return Some(chars.get(j).map_or(self.buf.len(), |(at, _)| *at));
                }
                i = j;
                continue;
            }
            i += 1;
        }
        if chars.len() > Self::MAX {
            let limit = chars[Self::MAX].0;
            let cut = self.buf[..limit]
                .rfind([',', ';', '，', '、'])
                .map(|p| p + self.buf[p..].chars().next().map_or(1, char::len_utf8))
                .or_else(|| self.buf[..limit].rfind(' '))
                .unwrap_or(limit);
            return Some(cut.max(1));
        }
        None
    }

    /// A dot after an abbreviation, an initial or inside a number.
    fn not_an_end(&self, chars: &[(usize, char)], dot: usize) -> bool {
        let next_is_digit = chars.get(dot + 1).is_some_and(|(_, c)| c.is_ascii_digit());
        let prev_is_digit = dot > 0 && chars[dot - 1].1.is_ascii_digit();
        if next_is_digit && prev_is_digit {
            return true;
        }
        let start = chars[..dot]
            .iter()
            .rposition(|(_, c)| c.is_whitespace() || *c == '(' || *c == '"')
            .map_or(0, |p| p + 1);
        let word: String = chars[start..dot].iter().map(|(_, c)| c.to_lowercase().to_string()).collect();
        let single_initial = word.chars().count() == 1 && word.chars().all(char::is_alphabetic);
        single_initial && chars[start].1.is_uppercase() || ABBREVIATIONS.contains(&word.as_str())
    }
}

fn is_terminal(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '…' | '‼' | '⁉' | '؟' | '۔' | '।') || is_cjk_terminal(c)
}

fn is_cjk_terminal(c: char) -> bool {
    matches!(c, '。' | '！' | '？')
}

fn is_closer(c: char) -> bool {
    matches!(c, '"' | '\'' | '»' | '”' | '’' | ')' | ']' | '」' | '』' | '）')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_are_json_with_tone_then_say_first() {
        for schema in [SCHEMA, SCHEMA_STRICT] {
            let v: Value = serde_json::from_str(schema).unwrap();
            let names: Vec<&str> = v["properties"]["actions"]["items"]["properties"]["skill"]["enum"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap())
                .collect();
            let all: Vec<&str> = Skill::ALL.iter().map(|s| s.name()).collect();
            assert_eq!(names, all);
            let tones: Vec<&str> = v["properties"]["tone"]["enum"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect();
            assert_eq!(tones, Tone::ALL.map(Tone::name));
            let tone = schema.find("\"tone\"").unwrap();
            let say = schema.find("\"say\"").unwrap();
            let actions = schema.find("\"actions\"").unwrap();
            assert!(tone < say && say < actions);
        }
    }

    #[test]
    fn parses_a_full_reply() {
        let reply = parse_reply(
            r#"{"tone":"sarcastic","say":"Wood. Again. Fine.","actions":[{"skill":"collect","item":"minecraft:oak_log","count":16},{"skill":"give","item":"oak_log","count":999}],"remember":" Steve hoards logs "}"#,
        );
        assert!(reply.valid_json);
        assert_eq!(reply.tone, Tone::Sarcastic);
        assert_eq!(reply.say, "Wood. Again. Fine.");
        assert_eq!(reply.actions[0].item.as_deref(), Some("oak_log"));
        assert_eq!(reply.actions[1].count, Some(MAX_COUNT));
        assert_eq!(reply.remember.as_deref(), Some("Steve hoards logs"));
    }

    #[test]
    fn unknown_skills_and_extra_actions_are_rejected() {
        let reply = parse_reply(
            r#"{"tone":"normal","say":"ok","actions":[{"skill":"fly"},{"skill":"jump"},{"skill":"jump"},{"skill":"eat"},{"skill":"sleep"}]}"#,
        );
        assert_eq!(reply.actions.len(), 3);
        assert_eq!(reply.rejected, ["fly", "sleep (more than 3 actions)"]);
    }

    #[test]
    fn tolerates_fences_thinking_and_plain_text() {
        let fenced = parse_reply("```json\n{\"say\":\"hi\",\"actions\":[]}\n```");
        assert_eq!((fenced.say.as_str(), fenced.valid_json, fenced.tone), ("hi", true, Tone::Normal));
        let thought = parse_reply("<think>they want {wood}</think>\n{\"say\":\"On it\",\"actions\":[{\"skill\":\"collect\",\"item\":\"oak_log\"}]}");
        assert_eq!(thought.say, "On it");
        assert_eq!(thought.actions[0].skill, Skill::Collect);
        let plain = parse_reply("Sure, I'll follow you!");
        assert!(!plain.valid_json);
        assert_eq!(plain.say, "Sure, I'll follow you!");
        assert!(plain.actions.is_empty());
    }

    fn stream_in_pieces(json: &str, size: usize) -> String {
        let mut stream = SayStream::new();
        let chars: Vec<char> = json.chars().collect();
        let mut out = String::new();
        for piece in chars.chunks(size) {
            out += &stream.push(&piece.iter().collect::<String>());
        }
        assert!(stream.finished(), "{json}");
        out
    }

    #[test]
    fn say_streams_out_of_partial_json() {
        let json = r#"{"tone":"shout","say":"Creeper! Run, \"now\"\n\u00e9\ud83d\ude00 \\o/","actions":[{"skill":"say","item":"say"}],"remember":"x"}"#;
        for size in [1, 2, 3, 7, 1000] {
            assert_eq!(stream_in_pieces(json, size), "Creeper! Run, \"now\"\né😀 \\o/", "pieces of {size}");
        }
    }

    #[test]
    fn say_is_found_wherever_it_is_and_never_inside_actions() {
        let json = r#"{"actions":[{"skill":"collect","say":"nope"}],"say":"Got it."}"#;
        assert_eq!(stream_in_pieces(json, 4), "Got it.");
        let thinking = "<think>\n{\"say\":\"no\"}\n</think>\n\n{\"say\":\"Yes.\"}";
        assert_eq!(stream_in_pieces(thinking, 3), "Yes.");
    }

    fn cut(text: &str) -> Vec<String> {
        let mut s = Sentences::new();
        let mut out = Vec::new();
        for c in text.chars() {
            out.extend(s.push(&c.to_string()));
        }
        out.extend(s.finish());
        out
    }

    #[test]
    fn sentences_in_five_languages() {
        assert_eq!(cut("Creeper, 2 o'clock! Back off. Now?"), ["Creeper, 2 o'clock!", "Back off.", "Now?"]);
        assert_eq!(cut("Attention ! Un creeper à 2 heures. On recule ?"), ["Attention !", "Un creeper à 2 heures.", "On recule ?"]);
        assert_eq!(cut("¡Cuidado! ¿Lo ves? Vamos."), ["¡Cuidado!", "¿Lo ves?", "Vamos."]);
        assert_eq!(cut("Achtung, z.B. ein Creeper! Lauf."), ["Achtung, z.B. ein Creeper!", "Lauf."]);
        assert_eq!(cut("Olha só... um creeper. Corre!"), ["Olha só...", "um creeper.", "Corre!"]);
    }

    #[test]
    fn numbers_initials_quotes_and_cjk() {
        assert_eq!(cut("Version 3.5 is out. Mr. Steve agrees."), ["Version 3.5 is out.", "Mr. Steve agrees."]);
        assert_eq!(cut("He said \"run!\" and ran."), ["He said \"run!\"", "and ran."]);
        assert_eq!(cut("J. R. is here. Go."), ["J. R. is here.", "Go."]);
        assert_eq!(cut("小心！苦力怕。"), ["小心！", "苦力怕。"]);
        assert_eq!(cut("Line one\nLine two"), ["Line one", "Line two"]);
    }

    #[test]
    fn a_sentence_is_emitted_as_soon_as_the_next_word_starts() {
        let mut s = Sentences::new();
        assert!(s.push("Creeper!").is_empty(), "could still be \"Creeper!!\"");
        assert_eq!(s.push(" R"), ["Creeper!"]);
        assert_eq!(s.finish().as_deref(), Some("R"));
    }

    #[test]
    fn long_runs_are_cut_at_a_comma_or_space() {
        let long = "word, ".repeat(40);
        let parts = cut(&long);
        assert!(parts.len() >= 2);
        assert!(parts.iter().all(|p| p.chars().count() <= Sentences::MAX));
        assert_eq!(parts.concat().replace(' ', ""), long.replace(' ', ""));
    }
}
