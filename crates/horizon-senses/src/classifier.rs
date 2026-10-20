//! The nervous system's second stage (docs/companions.md, "Reflexes"): a small multilingual
//! embedding model compares a line with an example bank and the nearest examples vote.
//!
//! The bank (`data/intents.tsv`, five languages, typos, slang and negations) is embedded once at
//! setup and cached on disk; a line then costs one embedding (a few milliseconds on one thread) and a
//! scan of ~600 vectors. The verdict ranks the intents, gives slot hints (item, count) and says
//! whether the line needs the brain: chat, questions, persona changes, insults and praise always do,
//! and so does anything the vote is unsure about.
use crate::sidecar::{Launch, Sidecar};
use serde_json::{Value, json};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

/// What a line asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Intent {
    Follow,
    Stay,
    Come,
    Stop,
    GoTo,
    Attack,
    Defend,
    HoldFire,
    Fire,
    Jump,
    Crouch,
    Stand,
    Collect,
    Give,
    Eat,
    Sleep,
    Craft,
    ChitChat,
    Question,
    PersonaChange,
    Insult,
    Praise,
    /// Said to someone else (other players, the stream, the room).
    NotAddressed,
}

impl Intent {
    pub const ALL: [Intent; 23] = [
        Intent::Follow,
        Intent::Stay,
        Intent::Come,
        Intent::Stop,
        Intent::GoTo,
        Intent::Attack,
        Intent::Defend,
        Intent::HoldFire,
        Intent::Fire,
        Intent::Jump,
        Intent::Crouch,
        Intent::Stand,
        Intent::Collect,
        Intent::Give,
        Intent::Eat,
        Intent::Sleep,
        Intent::Craft,
        Intent::ChitChat,
        Intent::Question,
        Intent::PersonaChange,
        Intent::Insult,
        Intent::Praise,
        Intent::NotAddressed,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Intent::Follow => "follow",
            Intent::Stay => "stay",
            Intent::Come => "come",
            Intent::Stop => "stop",
            Intent::GoTo => "go_to",
            Intent::Attack => "attack",
            Intent::Defend => "defend",
            Intent::HoldFire => "hold_fire",
            Intent::Fire => "fire",
            Intent::Jump => "jump",
            Intent::Crouch => "crouch",
            Intent::Stand => "stand",
            Intent::Collect => "collect",
            Intent::Give => "give",
            Intent::Eat => "eat",
            Intent::Sleep => "sleep",
            Intent::Craft => "craft",
            Intent::ChitChat => "chit_chat",
            Intent::Question => "question",
            Intent::PersonaChange => "persona_change",
            Intent::Insult => "insult",
            Intent::Praise => "praise",
            Intent::NotAddressed => "not_addressed",
        }
    }

    pub fn parse(name: &str) -> Option<Intent> {
        Intent::ALL.into_iter().find(|i| i.name() == name)
    }

    /// Talk the brain answers: never a reflex.
    pub fn needs_brain(self) -> bool {
        matches!(
            self,
            Intent::ChitChat
                | Intent::Question
                | Intent::PersonaChange
                | Intent::Insult
                | Intent::Praise
        )
    }

    /// Commands that are only complete with an item (`collect oak_log`).
    pub fn needs_item(self) -> bool {
        matches!(self, Intent::Collect | Intent::Give | Intent::Craft)
    }
}

/// One example of the bank.
#[derive(Clone, Debug, PartialEq)]
pub struct Example {
    pub intent: Intent,
    pub language: String,
    pub text: String,
}

/// The built-in example bank.
pub const BANK: &str = include_str!("../data/intents.tsv");

/// Parse `intent<TAB>language<TAB>line` rows (`#` comments).
pub fn parse_examples(tsv: &str) -> Result<Vec<Example>, String> {
    tsv.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|(n, line)| {
            let mut cols = line.splitn(3, '\t');
            let (Some(intent), Some(language), Some(text)) =
                (cols.next(), cols.next(), cols.next())
            else {
                return Err(format!("line {}: expected three columns", n + 1));
            };
            let intent =
                Intent::parse(intent).ok_or_else(|| format!("line {}: intent {intent}", n + 1))?;
            Ok(Example {
                intent,
                language: language.to_owned(),
                text: text.trim().to_owned(),
            })
        })
        .collect()
}

/// Turns lines into unit vectors.
pub trait Embed {
    /// A stable name: the model's id (the cache is per model).
    fn id(&self) -> &str;
    fn embed(&self, texts: &[&str]) -> io::Result<Vec<Vec<f32>>>;
}

/// The vote's knobs, tuned per model on the bank alone (leave-one-out), never on the test set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tuning {
    /// Neighbours that vote.
    pub k: usize,
    /// Softness of the vote: a neighbour weighs `exp((sim - best) / temperature)`.
    pub temperature: f32,
    /// Below this best similarity the line is unlike every example: the brain takes it.
    pub min_similarity: f32,
    /// Below this share of the vote the winner is unsure: the brain takes it.
    pub min_share: f32,
}

impl Tuning {
    /// The tuning measured for a catalogue model (see `examples/senses_bench.rs`).
    pub fn for_model(id: &str) -> Tuning {
        if id.starts_with("paraphrase-multilingual-minilm") {
            Tuning {
                k: 7,
                temperature: 0.03,
                min_similarity: 0.0,
                min_share: 0.8,
            }
        } else {
            Tuning {
                k: 7,
                temperature: 0.03,
                min_similarity: 0.9,
                min_share: 0.6,
            }
        }
    }
}

/// What the prefix of each line is for a model: e5 was trained with "query: ".
pub fn prefix_for(model_id: &str) -> &'static str {
    if model_id.contains("e5") {
        "query: "
    } else {
        ""
    }
}

/// Slot hints for the skills: they still check the item exists and is reachable.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Slots {
    /// An item id (`oak_log`), or a family the skill resolves (`pickaxe`, `food`).
    pub item: Option<String>,
    pub count: Option<u32>,
}

/// The classifier's answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    /// Every intent that got a vote, with its share of it, best first.
    pub ranked: Vec<(Intent, f32)>,
    /// The similarity of the nearest example.
    pub best_similarity: f32,
    pub slots: Slots,
    /// The brain should take the line (talk, or the vote is unsure, or an item is missing).
    pub needs_brain: bool,
}

impl Verdict {
    pub fn intent(&self) -> Intent {
        self.ranked[0].0
    }

    /// A command to run at once (the reflex), if this is one.
    pub fn command(&self) -> Option<Intent> {
        let intent = self.intent();
        (!self.needs_brain && !intent.needs_brain() && intent != Intent::NotAddressed)
            .then_some(intent)
    }

    /// Said to someone else, surely: no reflex, no brain.
    pub fn ignore(&self) -> bool {
        !self.needs_brain && self.intent() == Intent::NotAddressed
    }
}

/// The example bank, embedded.
pub struct Classifier {
    model: String,
    examples: Vec<(Intent, Vec<f32>)>,
    pub tuning: Tuning,
    vocabulary: Vocabulary,
}

impl Classifier {
    /// Embed the bank (or read it from `cache_dir`, written the first time).
    pub fn build(
        embedder: &dyn Embed,
        examples: &[Example],
        cache_dir: Option<&Path>,
    ) -> io::Result<Classifier> {
        let key = cache_key(embedder.id(), examples);
        let cache =
            cache_dir.map(|d| d.join(format!("classifier-{}-{key:016x}.bin", embedder.id())));
        let cached = cache.as_deref().and_then(|p| read_cache(p, examples.len()));
        let vectors = match cached {
            Some(v) => v,
            None => {
                let prefix = prefix_for(embedder.id());
                let texts: Vec<String> = examples
                    .iter()
                    .map(|e| format!("{prefix}{}", e.text))
                    .collect();
                let mut vectors = Vec::with_capacity(texts.len());
                for batch in texts.chunks(64) {
                    let batch: Vec<&str> = batch.iter().map(String::as_str).collect();
                    vectors.extend(embedder.embed(&batch)?.into_iter().map(normalized));
                }
                if let Some(path) = &cache {
                    write_cache(path, &vectors)?;
                }
                vectors
            }
        };
        Ok(Classifier {
            model: embedder.id().to_owned(),
            examples: examples.iter().map(|e| e.intent).zip(vectors).collect(),
            tuning: Tuning::for_model(embedder.id()),
            vocabulary: Vocabulary::builtin(),
        })
    }

    /// Item names the game knows (from its language files), added to the built-in hints.
    pub fn add_items(&mut self, names: impl IntoIterator<Item = (String, String)>) {
        self.vocabulary.add(names);
    }

    pub fn len(&self) -> usize {
        self.examples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.examples.is_empty()
    }

    /// Classify a line: one embedding, then [`Classifier::vote`].
    pub fn classify(&self, embedder: &dyn Embed, text: &str) -> io::Result<Verdict> {
        let line = format!("{}{text}", prefix_for(&self.model));
        let vector = embedder
            .embed(&[&line])?
            .pop()
            .ok_or_else(|| io::Error::other("no embedding"))?;
        Ok(self.vote(&normalized(vector), text, None))
    }

    /// The kNN vote for an embedded line. `skip` leaves one example out (tuning on the bank).
    pub fn vote(&self, vector: &[f32], text: &str, skip: Option<usize>) -> Verdict {
        let similarities: Vec<f32> = self.examples.iter().map(|(_, v)| dot(vector, v)).collect();
        self.decide(&similarities, text, skip)
    }

    /// The vote from a line's similarity to every example.
    pub fn decide(&self, similarities: &[f32], text: &str, skip: Option<usize>) -> Verdict {
        let t = self.tuning;
        let mut nearest: Vec<(f32, Intent)> = self
            .examples
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != skip)
            .map(|(i, (intent, _))| (similarities[i], *intent))
            .collect();
        let k = t.k.min(nearest.len()).max(1);
        nearest.select_nth_unstable_by(k - 1, |a, b| b.0.total_cmp(&a.0));
        nearest.truncate(k);
        let best = nearest.iter().fold(f32::MIN, |m, n| m.max(n.0));
        let mut votes: Vec<(Intent, f32)> = Vec::new();
        for (sim, intent) in &nearest {
            let w = ((sim - best) / t.temperature.max(1e-4)).exp();
            match votes.iter_mut().find(|(i, _)| i == intent) {
                Some(v) => v.1 += w,
                None => votes.push((*intent, w)),
            }
        }
        let total: f32 = votes.iter().map(|v| v.1).sum();
        votes.iter_mut().for_each(|v| v.1 /= total);
        votes.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let intent = votes[0].0;
        let slots = self.vocabulary.slots(text);
        let needs_brain = intent.needs_brain()
            || best < t.min_similarity
            || votes[0].1 < t.min_share
            || (intent.needs_item() && slots.item.is_none());
        Verdict {
            ranked: votes,
            best_similarity: best,
            slots,
            needs_brain,
        }
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn normalized(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
    v
}

/// FNV-1a over the model and every example: a changed bank re-embeds.
fn cache_key(model: &str, examples: &[Example]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    eat(model.as_bytes());
    for e in examples {
        eat(e.intent.name().as_bytes());
        eat(&[0]);
        eat(e.text.as_bytes());
        eat(&[1]);
    }
    h
}

const CACHE_MAGIC: &[u8; 8] = b"HZEMB\x001\x00";

fn write_cache(path: &Path, vectors: &[Vec<f32>]) -> io::Result<()> {
    let dim = vectors.first().map_or(0, Vec::len);
    let mut out = Vec::with_capacity(16 + vectors.len() * dim * 4);
    out.extend_from_slice(CACHE_MAGIC);
    out.extend_from_slice(&(dim as u32).to_le_bytes());
    out.extend_from_slice(&(vectors.len() as u32).to_le_bytes());
    for v in vectors {
        for x in v {
            out.extend_from_slice(&x.to_le_bytes());
        }
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let partial = path.with_extension("part");
    fs::write(&partial, out)?;
    fs::rename(partial, path)
}

fn read_cache(path: &Path, count: usize) -> Option<Vec<Vec<f32>>> {
    let bytes = fs::read(path).ok()?;
    if bytes.get(..8)? != CACHE_MAGIC {
        return None;
    }
    let dim = u32::from_le_bytes(bytes.get(8..12)?.try_into().ok()?) as usize;
    let n = u32::from_le_bytes(bytes.get(12..16)?.try_into().ok()?) as usize;
    if n != count || dim == 0 || bytes.len() != 16 + n * dim * 4 {
        return None;
    }
    Some(
        bytes[16..]
            .chunks_exact(dim * 4)
            .map(|v| {
                v.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b))
                    .collect()
            })
            .collect(),
    )
}

/// Lowercase without accents, letters and digits only (`Récupère` → `recupere`).
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ß' => 's',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        };
        out.push(c);
    }
    out
}

/// Number words of the five languages, folded (`a stack` is 64).
const NUMBERS: &[(&str, u32)] = &[
    ("one", 1),
    ("two", 2),
    ("three", 3),
    ("four", 4),
    ("five", 5),
    ("six", 6),
    ("seven", 7),
    ("eight", 8),
    ("nine", 9),
    ("ten", 10),
    ("twelve", 12),
    ("fifteen", 15),
    ("twenty", 20),
    ("thirty", 30),
    ("dozen", 12),
    ("stack", 64),
    ("un", 1),
    ("une", 1),
    ("deux", 2),
    ("trois", 3),
    ("quatre", 4),
    ("cinq", 5),
    ("sept", 7),
    ("huit", 8),
    ("neuf", 9),
    ("dix", 10),
    ("douze", 12),
    ("quinze", 15),
    ("vingt", 20),
    ("trente", 30),
    ("douzaine", 12),
    ("pile", 64),
    ("uno", 1),
    ("una", 1),
    ("dos", 2),
    ("tres", 3),
    ("cuatro", 4),
    ("cinco", 5),
    ("seis", 6),
    ("siete", 7),
    ("ocho", 8),
    ("nueve", 9),
    ("diez", 10),
    ("doce", 12),
    ("quince", 15),
    ("veinte", 20),
    ("treinta", 30),
    ("docena", 12),
    ("eins", 1),
    ("zwei", 2),
    ("drei", 3),
    ("vier", 4),
    ("funf", 5),
    ("sechs", 6),
    ("sieben", 7),
    ("acht", 8),
    ("neun", 9),
    ("zehn", 10),
    ("zwolf", 12),
    ("funfzehn", 15),
    ("zwanzig", 20),
    ("dreissig", 30),
    ("dutzend", 12),
    ("um", 1),
    ("uma", 1),
    ("dois", 2),
    ("duas", 2),
    ("tres", 3),
    ("quatro", 4),
    ("cinco", 5),
    ("sete", 7),
    ("oito", 8),
    ("nove", 9),
    ("dez", 10),
    ("doze", 12),
    ("quinze", 15),
    ("vinte", 20),
    ("trinta", 30),
    ("duzia", 12),
];

/// The built-in item hints, folded stems → item id or family. The game adds its own names.
const ITEMS: &[(&str, &str)] = &[
    ("wood", "oak_log"),
    ("log", "oak_log"),
    ("bois", "oak_log"),
    ("buche", "oak_log"),
    ("madera", "oak_log"),
    ("tronco", "oak_log"),
    ("holz", "oak_log"),
    ("madeira", "oak_log"),
    ("tree", "oak_log"),
    ("arbre", "oak_log"),
    ("arbol", "oak_log"),
    ("baum", "oak_log"),
    ("arvore", "oak_log"),
    ("plank", "oak_planks"),
    ("planche", "oak_planks"),
    ("tablon", "oak_planks"),
    ("brett", "oak_planks"),
    ("tabua", "oak_planks"),
    ("cobblestone", "cobblestone"),
    ("cobble", "cobblestone"),
    ("stone", "cobblestone"),
    ("pierre", "cobblestone"),
    ("piedra", "cobblestone"),
    ("bruchstein", "cobblestone"),
    ("stein", "cobblestone"),
    ("pedra", "cobblestone"),
    ("coal", "coal"),
    ("charbon", "coal"),
    ("carbon", "coal"),
    ("kohle", "coal"),
    ("carvao", "coal"),
    ("iron", "iron_ingot"),
    ("fer", "iron_ingot"),
    ("hierro", "iron_ingot"),
    ("eisen", "iron_ingot"),
    ("ferro", "iron_ingot"),
    ("diamond", "diamond"),
    ("diamant", "diamond"),
    ("diamante", "diamond"),
    ("bread", "bread"),
    ("pain", "bread"),
    ("pan", "bread"),
    ("brot", "bread"),
    ("pao", "bread"),
    ("paes", "bread"),
    ("food", "food"),
    ("bouffe", "food"),
    ("manger", "food"),
    ("comida", "food"),
    ("essen", "food"),
    ("torch", "torch"),
    ("torche", "torch"),
    ("antorcha", "torch"),
    ("fackel", "torch"),
    ("tocha", "torch"),
    ("pickaxe", "pickaxe"),
    ("pioche", "pickaxe"),
    ("pico", "pickaxe"),
    ("spitzhacke", "pickaxe"),
    ("picareta", "pickaxe"),
    ("sword", "sword"),
    ("epee", "sword"),
    ("espada", "sword"),
    ("schwert", "sword"),
    ("arrow", "arrow"),
    ("fleche", "arrow"),
    ("flecha", "arrow"),
    ("pfeil", "arrow"),
    ("crafting table", "crafting_table"),
    ("etabli", "crafting_table"),
    ("mesa de trabajo", "crafting_table"),
    ("werkbank", "crafting_table"),
    ("bancada", "crafting_table"),
    ("furnace", "furnace"),
    ("four", "furnace"),
    ("horno", "furnace"),
    ("ofen", "furnace"),
    ("fornalha", "furnace"),
    ("chest", "chest"),
    ("coffre", "chest"),
    ("cofre", "chest"),
    ("truhe", "chest"),
    ("bau", "chest"),
];

/// Item and number words, matched on folded whole words with plural endings.
#[derive(Clone, Debug, Default)]
pub struct Vocabulary {
    /// Folded name → item, longest names first.
    items: Vec<(String, String)>,
}

impl Vocabulary {
    pub fn builtin() -> Vocabulary {
        let mut v = Vocabulary::default();
        v.add(
            ITEMS
                .iter()
                .map(|(n, i)| ((*n).to_owned(), (*i).to_owned())),
        );
        v
    }

    pub fn add(&mut self, names: impl IntoIterator<Item = (String, String)>) {
        for (name, item) in names {
            let name = fold(&name).split_whitespace().collect::<Vec<_>>().join(" ");
            if !name.is_empty() && !self.items.iter().any(|(n, _)| *n == name) {
                self.items.push((name, item));
            }
        }
        self.items.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
    }

    pub fn slots(&self, text: &str) -> Slots {
        let folded = fold(text);
        let words: Vec<&str> = folded.split_whitespace().collect();
        let number = |w: &str| {
            w.parse::<u32>()
                .ok()
                .or_else(|| NUMBERS.iter().find(|(n, _)| *n == w).map(|(_, v)| *v))
        };
        let singular = |w: &str| -> Vec<String> {
            let mut forms = vec![w.to_owned()];
            for end in ["es", "s", "en", "n", "e"] {
                if let Some(stem) = w.strip_suffix(end).filter(|s| s.len() >= 3) {
                    forms.push(stem.to_owned());
                }
            }
            forms
        };
        // Every item name in the line, longest names first, with the words it covers.
        let mut found: Vec<(&str, std::ops::Range<usize>)> = Vec::new();
        for (name, item) in &self.items {
            let parts: Vec<&str> = name.split(' ').collect();
            if let Some(at) = words.windows(parts.len()).position(|window| {
                window
                    .iter()
                    .zip(&parts)
                    .all(|(w, p)| singular(w).iter().any(|f| f == p))
            }) {
                found.push((item, at..at + parts.len()));
            }
        }
        // "four" is a number in English and a furnace in French: a number word is the item only
        // when nothing else is.
        let is_number =
            |r: &std::ops::Range<usize>| r.len() == 1 && number(words[r.start]).is_some();
        let item = found
            .iter()
            .find(|(_, r)| !is_number(r))
            .or_else(|| found.first())
            .cloned();
        let count = words
            .iter()
            .enumerate()
            .filter(|(i, _)| item.as_ref().is_none_or(|(_, r)| !r.contains(i)))
            .find_map(|(_, w)| number(w));
        Slots {
            item: item.map(|(i, _)| i.to_owned()),
            count,
        }
    }
}

/// The classifier's embedding server: the brain's llama.cpp build, as a second `llama-server` on one
/// low-priority thread with no GPU layers (the game keeps the GPU), a random API key and a small
/// context.
pub struct EmbedServer {
    sidecar: Sidecar,
    client: EmbedClient,
}

impl EmbedServer {
    pub fn start(
        llama_server: &Path,
        model: &Path,
        model_id: &str,
        threads: usize,
        work: &Path,
    ) -> io::Result<EmbedServer> {
        let mut key = [0u8; 16];
        getrandom::fill(&mut key).map_err(|e| io::Error::other(e.to_string()))?;
        let key: String = key.iter().map(|b| format!("{b:02x}")).collect();
        let threads = threads.max(1).to_string();
        let args: Vec<std::ffi::OsString> = vec![
            "-m".into(),
            model.into(),
            "--embeddings".into(),
            "--pooling".into(),
            "mean".into(),
            "--threads".into(),
            threads.clone().into(),
            "--threads-batch".into(),
            threads.into(),
            "--n-gpu-layers".into(),
            "0".into(),
            "--device".into(),
            "none".into(),
            "--ctx-size".into(),
            "512".into(),
            "--batch-size".into(),
            "512".into(),
            "--ubatch-size".into(),
            "512".into(),
            "--parallel".into(),
            "1".into(),
            "--no-webui".into(),
            "--host".into(),
            "127.0.0.1".into(),
            "--port".into(),
            "{port}".into(),
            "--api-key".into(),
            key.clone().into(),
        ];
        let launch = Launch {
            exe: llama_server.to_owned(),
            args,
            dir: llama_server.parent().unwrap_or(Path::new(".")).to_owned(),
            log: work.join("classifier-server.log"),
            files: Vec::new(),
        };
        let sidecar = Sidecar::start(&launch, "/health", Duration::from_secs(30))?;
        let client = EmbedClient::new(&sidecar.base_url(), Some(key), model_id);
        Ok(EmbedServer { sidecar, client })
    }

    pub fn client(&self) -> &EmbedClient {
        &self.client
    }

    pub fn sidecar(&mut self) -> &mut Sidecar {
        &mut self.sidecar
    }
}

/// An OpenAI-style `/v1/embeddings` endpoint.
#[derive(Clone)]
pub struct EmbedClient {
    agent: ureq::Agent,
    url: String,
    key: Option<String>,
    model: String,
}

impl EmbedClient {
    pub fn new(base: &str, key: Option<String>, model: &str) -> EmbedClient {
        EmbedClient {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(2))
                .timeout_read(Duration::from_secs(30))
                .build(),
            url: format!("{}/v1/embeddings", base.trim_end_matches('/')),
            key,
            model: model.to_owned(),
        }
    }
}

impl Embed for EmbedClient {
    fn id(&self) -> &str {
        &self.model
    }

    fn embed(&self, texts: &[&str]) -> io::Result<Vec<Vec<f32>>> {
        let mut request = self.agent.post(&self.url);
        if let Some(key) = &self.key {
            request = request.set("Authorization", &format!("Bearer {key}"));
        }
        let response: Value = request
            .set("Content-Type", "application/json")
            .send_string(&json!({ "input": texts, "model": self.model }).to_string())
            .map(|r| serde_json::from_reader(r.into_reader()))
            .map_err(|e| io::Error::other(e.to_string()))?
            .map_err(io::Error::other)?;
        let data = response
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| io::Error::other(format!("embeddings: {response}")))?;
        let mut out = vec![Vec::new(); texts.len()];
        for item in data {
            let index = item.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let vector = item
                .get("embedding")
                .and_then(Value::as_array)
                .ok_or_else(|| io::Error::other("embeddings: no vector"))?
                .iter()
                .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                .collect();
            if let Some(slot) = out.get_mut(index) {
                *slot = vector;
            }
        }
        if out.iter().any(Vec::is_empty) {
            return Err(io::Error::other("embeddings: missing vectors"));
        }
        Ok(out)
    }
}

/// Where the bank's vectors are cached for a model.
pub fn cache_dir(companion: &Path) -> PathBuf {
    companion.join("cache")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A toy embedder: character trigram hashes. Enough to check the vote, the cache and the
    /// rules without a model.
    struct Trigrams {
        calls: Cell<usize>,
    }

    impl Embed for Trigrams {
        fn id(&self) -> &str {
            "trigrams"
        }

        fn embed(&self, texts: &[&str]) -> io::Result<Vec<Vec<f32>>> {
            self.calls.set(self.calls.get() + 1);
            Ok(texts
                .iter()
                .map(|t| {
                    let chars: Vec<char> = format!("  {}  ", fold(t)).chars().collect();
                    let mut v = vec![0.0f32; 256];
                    for w in chars.windows(3) {
                        let h = w.iter().fold(7u32, |h, c| h.wrapping_mul(31) ^ *c as u32);
                        v[(h % 256) as usize] += 1.0;
                    }
                    v
                })
                .collect())
        }
    }

    fn toy() -> (Trigrams, Classifier) {
        let embedder = Trigrams {
            calls: Cell::new(0),
        };
        let mut classifier =
            Classifier::build(&embedder, &parse_examples(BANK).unwrap(), None).unwrap();
        classifier.tuning = Tuning {
            k: 5,
            temperature: 0.05,
            min_similarity: 0.5,
            min_share: 0.5,
        };
        (embedder, classifier)
    }

    #[test]
    fn the_bank_covers_every_intent_in_five_languages() {
        let examples = parse_examples(BANK).unwrap();
        for intent in Intent::ALL {
            for language in ["en", "fr", "es", "de", "pt"] {
                let n = examples
                    .iter()
                    .filter(|e| e.intent == intent && e.language == language)
                    .count();
                assert!(n >= 5, "{} {language}: {n}", intent.name());
            }
            let n = examples.iter().filter(|e| e.intent == intent).count();
            assert!((50..=60).contains(&n), "{}: {n}", intent.name());
        }
        let test = parse_examples(include_str!("../data/intents-test.tsv")).unwrap();
        assert!(test.len() >= 300, "{}", test.len());
        for t in &test {
            assert!(
                !examples.iter().any(|e| fold(&e.text) == fold(&t.text)),
                "held-out line in the bank: {}",
                t.text
            );
        }
        assert!(parse_examples("follow\ten").is_err());
        assert!(parse_examples("dance\ten\tdance").is_err());
    }

    #[test]
    fn near_copies_of_examples_win_their_intent() {
        let (embedder, classifier) = toy();
        let v = classifier.classify(&embedder, "suis-moi stp").unwrap();
        assert_eq!(v.intent(), Intent::Follow);
        assert_eq!(v.command(), Some(Intent::Follow));
        let v = classifier.classify(&embedder, "ne me suis pas").unwrap();
        assert_eq!(v.intent(), Intent::Stay);
        let v = classifier
            .classify(&embedder, "raconte-moi une blague")
            .unwrap();
        assert_eq!(v.intent(), Intent::ChitChat);
        assert!(v.needs_brain);
        assert_eq!(v.command(), None);
    }

    #[test]
    fn unlike_lines_and_split_votes_go_to_the_brain() {
        let (embedder, mut classifier) = toy();
        let v = classifier
            .classify(&embedder, "xylophone quantum zebra")
            .unwrap();
        assert!(v.needs_brain, "{v:?}");
        classifier.tuning.min_similarity = 0.0;
        classifier.tuning.min_share = 1.01;
        assert!(
            classifier
                .classify(&embedder, "follow me")
                .unwrap()
                .needs_brain
        );
    }

    #[test]
    fn item_commands_without_an_item_go_to_the_brain() {
        let (embedder, classifier) = toy();
        let v = classifier.classify(&embedder, "collect 10 stone").unwrap();
        assert_eq!(v.intent(), Intent::Collect);
        assert_eq!(v.slots.item.as_deref(), Some("cobblestone"));
        assert_eq!(v.slots.count, Some(10));
        assert!(!v.needs_brain);
        let v = classifier.vote(
            &v_of(&embedder, "collect 10 stone"),
            "collect ten of those",
            None,
        );
        assert_eq!(v.slots.count, Some(10));
        assert!(v.needs_brain);
    }

    fn v_of(embedder: &Trigrams, text: &str) -> Vec<f32> {
        normalized(embedder.embed(&[text]).unwrap().pop().unwrap())
    }

    #[test]
    fn slots_read_numbers_and_items_in_five_languages() {
        let v = Vocabulary::builtin();
        let s = |t: &str| {
            let s = v.slots(t);
            (s.item, s.count)
        };
        assert_eq!(s("gather 20 logs"), (Some("oak_log".into()), Some(20)));
        assert_eq!(
            s("Récupère dix pierres"),
            (Some("cobblestone".into()), Some(10))
        );
        assert_eq!(s("dame cinco panes"), (Some("bread".into()), Some(5)));
        assert_eq!(
            s("sammle zwanzig Steine"),
            (Some("cobblestone".into()), Some(20))
        );
        assert_eq!(s("me dá três pães"), (Some("bread".into()), Some(3)));
        assert_eq!(s("fais un four"), (Some("furnace".into()), Some(1)));
        assert_eq!(
            s("crafte quatre planches"),
            (Some("oak_planks".into()), Some(4))
        );
        assert_eq!(
            s("construye una mesa de trabajo"),
            (Some("crafting_table".into()), Some(1))
        );
        assert_eq!(
            s("get me a stack of cobble"),
            (Some("cobblestone".into()), Some(64))
        );
        assert_eq!(s("hello there"), (None, None));
        let mut v = v;
        v.add([("Mangrove Log".to_owned(), "mangrove_log".to_owned())]);
        assert_eq!(
            v.slots("get mangrove logs").item.as_deref(),
            Some("mangrove_log")
        );
    }

    #[test]
    fn the_bank_is_embedded_once_then_read_from_the_cache() {
        let dir = std::env::temp_dir().join(format!("horizon-senses-cache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let examples = parse_examples(BANK).unwrap();
        let embedder = Trigrams {
            calls: Cell::new(0),
        };
        let first = Classifier::build(&embedder, &examples, Some(&dir)).unwrap();
        let calls = embedder.calls.get();
        assert!(calls >= examples.len() / 64);
        let second = Classifier::build(&embedder, &examples, Some(&dir)).unwrap();
        assert_eq!(embedder.calls.get(), calls);
        assert_eq!(first.examples, second.examples);
        // A changed bank misses the cache.
        Classifier::build(&embedder, &examples[1..], Some(&dir)).unwrap();
        assert!(embedder.calls.get() > calls);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn folding_strips_accents_and_punctuation() {
        assert_eq!(fold("Ça va, Jürgen?"), "ca va  jurgen ");
        assert_eq!(fold("ÉPÉE"), "epee");
    }
}
