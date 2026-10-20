//! The nervous system: lines the companion hears go through three stages and the first one that is
//! sure wins (docs/companions.md, "Reflexes").
//!
//! 1. [`parse`]: rules in English, French, Spanish, German and Portuguese, well under a millisecond.
//!    Accents, case, politeness ("can you", "s'il te plaît", "bitte") and small typos don't matter;
//!    negations do ("don't follow me" is not follow).
//! 2. An [`IntentClassifier`] (the senses' embedding model), trusted above a confidence threshold.
//! 3. The brain, for everything else.
//!
//! Item and mob names come from the game through [`Names`] (its language files), so "bûches de chêne",
//! "Eichenstämme" and "oak logs" all reach `oak_log`.

use std::cell::RefCell;
use std::collections::HashMap;

/// What the companion should do. Mirrors the game's companion commands one to one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    Follow,
    Stay,
    Come,
    GoTo {
        target: Target,
    },
    Stop,
    /// A mob id, or any hostile when `None`.
    Attack {
        target: Option<String>,
    },
    Defend,
    /// `true`: hold fire; `false`: fire at will again.
    HoldFire(bool),
    Jump,
    /// `true`: crouch; `false`: stand up.
    Crouch(bool),
    Eat,
    Sleep,
    Collect {
        item: String,
        count: Option<u32>,
    },
    Give {
        item: String,
        count: Option<u32>,
    },
    Craft {
        item: String,
        count: Option<u32>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Where the owner looks; with a kind ("that tree"), the block of that kind nearest the ray.
    Crosshair { kind: Option<String> },
}

/// What the rules made of a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parse {
    Intent(Intent),
    /// Not a command the rules are sure of: the next stage decides.
    Unclear,
}

/// Item and mob names as the game knows them. Words arrive normalised: lowercase, without accents or
/// punctuation, single spaces (see [`normalize`]).
pub trait Names {
    /// An item or block id (`oak_log`), or an item tag (`#logs`) for generic words like "wood".
    fn item(&self, words: &str) -> Option<String>;
    /// An entity id (`zombie`).
    fn mob(&self, words: &str) -> Option<String>;
}

/// A [`Names`] built from (id, display name) pairs: every item and entity of the game's current
/// language file, plus en_us. Plurals of the five languages and small typos are accepted; generic
/// material words ("wood", "bois", "Holz") fall back to tags and common ids.
#[derive(Clone, Debug, Default)]
pub struct NameIndex {
    items: Table,
    mobs: Table,
}

#[derive(Clone, Debug, Default)]
struct Table {
    exact: HashMap<String, String>,
    /// Names by length in characters, for typo matching.
    by_len: HashMap<usize, Vec<(String, String)>>,
}

impl Table {
    fn add(&mut self, id: &str, name: &str) {
        let id = id.trim_start_matches("minecraft:");
        for key in [normalize(name), id.replace('_', " ")] {
            if !key.is_empty() && !self.exact.contains_key(&key) {
                self.exact.insert(key.clone(), id.to_owned());
                self.by_len
                    .entry(key.chars().count())
                    .or_default()
                    .push((key, id.to_owned()));
            }
        }
    }

    fn find(&self, words: &str) -> Option<String> {
        let variants = plural_variants(words);
        if let Some(id) = variants.iter().find_map(|v| self.exact.get(v)) {
            return Some(id.clone());
        }
        let mut best: Option<(usize, &str, &str)> = None;
        for v in &variants[..variants.len().min(2)] {
            let len = v.chars().count();
            let max = typo_allowance(len);
            for l in len.saturating_sub(max)..=len + max {
                for (name, id) in self.by_len.get(&l).into_iter().flatten() {
                    let d = distance(v, name);
                    if d <= max
                        && best.is_none_or(|(bd, bn, _)| d < bd || d == bd && name.as_str() < bn)
                    {
                        best = Some((d, name, id));
                    }
                }
            }
        }
        best.map(|(_, _, id)| id.to_owned())
    }
}

impl NameIndex {
    pub fn new() -> NameIndex {
        NameIndex::default()
    }

    pub fn add_item(&mut self, id: &str, name: &str) {
        self.items.add(id, name);
    }

    pub fn add_mob(&mut self, id: &str, name: &str) {
        self.mobs.add(id, name);
    }
}

impl Names for NameIndex {
    fn item(&self, words: &str) -> Option<String> {
        self.items
            .find(words)
            .or_else(|| material(words).map(String::from))
    }

    fn mob(&self, words: &str) -> Option<String> {
        self.mobs.find(words)
    }
}

/// Remembers lookups during one parse: the same words are tried by many patterns and languages.
struct Memo<'a> {
    names: &'a dyn Names,
    items: RefCell<HashMap<String, Option<String>>>,
    mobs: RefCell<HashMap<String, Option<String>>>,
}

impl Names for Memo<'_> {
    fn item(&self, words: &str) -> Option<String> {
        if let Some(found) = self.items.borrow().get(words) {
            return found.clone();
        }
        let found = self.names.item(words);
        self.items
            .borrow_mut()
            .insert(words.to_owned(), found.clone());
        found
    }

    fn mob(&self, words: &str) -> Option<String> {
        if let Some(found) = self.mobs.borrow().get(words) {
            return found.clone();
        }
        let found = self.names.mob(words);
        self.mobs
            .borrow_mut()
            .insert(words.to_owned(), found.clone());
        found
    }
}

/// The words as written, then with each language's plural endings taken off every word: `-s`, `-es`
/// (English, French, Spanish, Portuguese), `-ões`/`-ães` (Portuguese), `-e`, `-en`, `-er`, `-n` (German).
fn plural_variants(words: &str) -> Vec<String> {
    type Rule = fn(&str) -> Option<String>;
    let rules: [Rule; 7] = [
        |w| Some(w.to_owned()),
        |w| {
            (w.len() > 3 && w.ends_with('s') && !w.ends_with("ss"))
                .then(|| w[..w.len() - 1].to_owned())
        },
        |w| (w.len() > 4 && w.ends_with("es")).then(|| w[..w.len() - 2].to_owned()),
        |w| {
            ["aes", "oes", "aos"]
                .iter()
                .find(|e| w.ends_with(*e))
                .map(|e| format!("{}ao", &w[..w.len() - e.len()]))
        },
        |w| (w.len() > 4 && w.ends_with('e')).then(|| w[..w.len() - 1].to_owned()),
        |w| {
            (w.len() > 5 && (w.ends_with("en") || w.ends_with("er")))
                .then(|| w[..w.len() - 2].to_owned())
        },
        |w| (w.len() > 4 && w.ends_with('n')).then(|| w[..w.len() - 1].to_owned()),
    ];
    let mut out: Vec<String> = Vec::new();
    for rule in rules {
        let v: Vec<String> = words
            .split(' ')
            .map(|w| rule(w).unwrap_or_else(|| w.to_owned()))
            .collect();
        let v = v.join(" ");
        if !out.contains(&v) {
            out.push(v);
        }
    }
    out
}

/// Generic words players use for common materials, in the five languages.
fn material(words: &str) -> Option<&'static str> {
    plural_variants(words).iter().find_map(|w| {
        Some(match w.as_str() {
            "wood" | "log" | "tree" | "bois" | "buche" | "arbre" | "madera" | "tronco"
            | "arbol" | "holz" | "stamm" | "baum" | "madeira" | "arvore" => "#logs",
            "plank" | "wooden plank" | "planche" | "tabla" | "tablon" | "brett" | "planke"
            | "tabua" => "#planks",
            "stone" | "cobble" | "rock" | "pierre" | "roche" | "piedra" | "stein"
            | "bruchstein" | "pedra" => "cobblestone",
            "iron" | "fer" | "hierro" | "eisen" | "ferro" => "iron_ingot",
            "gold" | "or" | "oro" | "ouro" => "gold_ingot",
            "diamond" | "diamant" | "diamante" => "diamond",
            "coal" | "charbon" | "carbon" | "kohle" | "carvao" => "coal",
            "dirt" | "terre" | "tierra" | "erde" | "terra" => "dirt",
            "sand" | "sable" | "arena" | "areia" => "sand",
            "wool" | "laine" | "lana" | "wolle" => "#wool",
            "string" | "ficelle" | "hilo" | "faden" | "linha" => "string",
            "torch" | "torche" | "antorcha" | "fackel" | "tocha" => "torch",
            _ => return None,
        })
    })
}

/// Lowercase, accents folded (`é` → `e`, `ß` → `ss`, `œ` → `oe`), apostrophes, hyphens and punctuation
/// turned into spaces, single spaces. Digits are kept.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        let folded: &str = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => "a",
            'ç' | 'ć' | 'č' => "c",
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' => "e",
            'ì' | 'í' | 'î' | 'ï' => "i",
            'ñ' | 'ń' => "n",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => "o",
            'ù' | 'ú' | 'û' | 'ü' | 'ū' => "u",
            'ý' | 'ÿ' => "y",
            'ß' => "ss",
            'œ' => "oe",
            'æ' => "ae",
            'ł' => "l",
            c if c.is_alphanumeric() => {
                out.push(c);
                continue;
            }
            _ => " ",
        };
        out.push_str(folded);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Damerau-Levenshtein distance (adjacent swaps count once), on characters.
fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len().abs_diff(b.len()) > 2 {
        return 3;
    }
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=b.len() {
        d[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

/// Typos allowed in a word or name of `len` characters: none below five, one from five, two from ten.
fn typo_allowance(len: usize) -> usize {
    if len >= 10 {
        2
    } else if len >= 5 {
        1
    } else {
        0
    }
}

fn word_matches(pattern: &str, word: &str) -> bool {
    pattern == word || {
        let max = typo_allowance(pattern.chars().count()).min(1);
        max > 0 && distance(pattern, word) <= max
    }
}

/// The command in a chat line if it is addressed to the companion: `@Kev follow me`, `Kev, follow me`,
/// `Kev follow me`, `hey Kev follow me`, `follow me, Kev`, or anything in the companion's chat mode.
pub fn addressed<'a>(line: &'a str, name: &str, companion_chat: bool) -> Option<&'a str> {
    let line = line.trim();
    if companion_chat {
        return Some(line.trim_start_matches('@').trim());
    }
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    // Lowercasing keeps byte offsets for the names players use; others are not addressed this way.
    let lower = line.to_lowercase();
    let lname = name.to_lowercase();
    if lower.len() != line.len() {
        return None;
    }
    let is_sep = |c: char| c.is_whitespace() || matches!(c, ',' | ':' | '!' | '.' | ';' | '?');
    let mut start = usize::from(lower.starts_with('@'));
    if start == 0
        && let Some(first) = lower.split(is_sep).next()
        && GREETINGS.contains(&first)
    {
        let rest = lower[first.len()..].trim_start_matches(is_sep);
        if rest.starts_with(&lname) {
            start = lower.len() - rest.len();
        }
    }
    if lower[start..].starts_with(&lname) {
        let after = start + lname.len();
        if lower[after..].chars().next().is_none_or(is_sep) {
            return Some(line[after..].trim_start_matches(is_sep).trim());
        }
    }
    if let Some(before) = lower.trim_end_matches(is_sep).strip_suffix(&lname)
        && before.trim_end().ends_with(',')
    {
        return Some(line[..before.len()].trim_end_matches(is_sep).trim());
    }
    None
}

const GREETINGS: &[&str] = &[
    "hey", "hi", "yo", "ok", "okay", "oi", "eh", "hello", "salut", "he", "hola", "oye", "hallo",
    "ei", "ola",
];

/// The languages the rules know, by game-code prefix.
pub const LANGUAGES: [&str; 5] = ["en", "fr", "es", "de", "pt"];

/// The rules stage. The game language's patterns are tried first, then the four others (a French
/// player may type in English): it matters where languages overlap, "come" being "eat" in Spanish and
/// Portuguese.
pub fn parse(text: &str, language: &str, names: &dyn Names) -> Parse {
    let normalized = normalize(text);
    let words: Vec<&str> = normalized.split(' ').filter(|w| !w.is_empty()).collect();
    if words.is_empty() || words.len() > 14 {
        return Parse::Unclear;
    }
    let first = language
        .split(['_', '-'])
        .next()
        .unwrap_or("en")
        .to_ascii_lowercase();
    let mut order: Vec<&str> = LANGUAGES.iter().copied().filter(|l| *l == first).collect();
    order.extend(LANGUAGES.iter().copied().filter(|l| *l != first));
    let memo = Memo {
        names,
        items: RefCell::default(),
        mobs: RefCell::default(),
    };
    order
        .into_iter()
        .find_map(|lang| parse_in(&words, lang, &memo))
        .unwrap_or(Parse::Unclear)
}

fn parse_in(words: &[&str], lang: &str, names: &dyn Names) -> Option<Parse> {
    let grammar = grammar(lang);
    let mut words = words.to_vec();
    strip_fillers(&mut words, grammar);
    if words.is_empty() {
        return None;
    }
    // As written first ("never mind" is not a negation), then without the negation words.
    if let Some(parse) = match_intents(&words, false, grammar, names) {
        return Some(parse);
    }
    strip_negation(&mut words, grammar)
        .then(|| match_intents(&words, true, grammar, names))
        .flatten()
}

fn match_intents(
    words: &[&str],
    negated: bool,
    grammar: &Grammar,
    names: &dyn Names,
) -> Option<Parse> {
    for (kind, patterns) in grammar.intents {
        for pattern in patterns.split(';') {
            let elements = parse_pattern(pattern.trim());
            let mut slots = Slots::default();
            if match_elements(&elements, words, &mut slots, names, grammar) {
                return Some(match (kind, negated) {
                    (Kind::Move, true) => Parse::Intent(Intent::Stay),
                    (Kind::Move, false) => Parse::Unclear,
                    (_, false) => Parse::Intent(build(*kind, slots)?),
                    (_, true) => negate(build(*kind, slots)?),
                });
            }
        }
    }
    None
}

/// A negated command: "don't shoot" holds fire, "don't attack the villager" stops the fight (only that
/// one: other mobs may still be fought) and "don't follow me" stays. Any other negated command ("don't
/// eat", "don't get wood") is for the brain. The benchmark showed the small brain attacking a villager
/// right after being told not to, so a rule that cannot get this wrong takes it.
fn negate(intent: Intent) -> Parse {
    match intent {
        Intent::Attack { target: None } | Intent::HoldFire(_) => {
            Parse::Intent(Intent::HoldFire(true))
        }
        Intent::Attack { target: Some(_) } => Parse::Intent(Intent::Stop),
        Intent::Follow => Parse::Intent(Intent::Stay),
        _ => Parse::Unclear,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Follow,
    Stay,
    /// "Move": only a command when negated ("don't move" stays).
    Move,
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
    Eat,
    Sleep,
    Collect,
    Give,
    Craft,
}

#[derive(Clone, Default, Debug)]
struct Slots {
    count: Option<u32>,
    item: Option<String>,
    mob: Option<String>,
}

fn build(kind: Kind, s: Slots) -> Option<Intent> {
    Some(match kind {
        Kind::Follow => Intent::Follow,
        Kind::Stay | Kind::Move => Intent::Stay,
        Kind::Come => Intent::Come,
        Kind::GoTo => Intent::GoTo {
            target: Target::Crosshair { kind: s.item },
        },
        Kind::Stop => Intent::Stop,
        Kind::Attack => Intent::Attack { target: s.mob },
        Kind::Defend => Intent::Defend,
        Kind::HoldFire => Intent::HoldFire(true),
        Kind::Fire => Intent::HoldFire(false),
        Kind::Jump => Intent::Jump,
        Kind::Crouch => Intent::Crouch(true),
        Kind::StandUp => Intent::Crouch(false),
        Kind::Eat => Intent::Eat,
        Kind::Sleep => Intent::Sleep,
        Kind::Collect => Intent::Collect {
            item: s.item?,
            count: s.count,
        },
        Kind::Give => Intent::Give {
            item: s.item?,
            count: s.count,
        },
        Kind::Craft => Intent::Craft {
            item: s.item?,
            count: s.count,
        },
    })
}

#[derive(Debug)]
enum Element {
    /// Alternatives of one or more words (written joined by `_`).
    Words(Vec<Vec<String>>),
    Optional(Box<Element>),
    Count,
    Item,
    Mob,
}

/// `get|fetch [me] [{n}] {item}`: `a|b` alternatives, `x_y` two words, `[…]` optional, `{n}` a count,
/// `{item}` an item name, `{mob}` a mob name.
fn parse_pattern(pattern: &str) -> Vec<Element> {
    pattern
        .split_whitespace()
        .map(|token| {
            let (optional, inner) = match token.strip_prefix('[').and_then(|t| t.strip_suffix(']'))
            {
                Some(inner) => (true, inner),
                None => (false, token),
            };
            let element = match inner {
                "{n}" => Element::Count,
                "{item}" => Element::Item,
                "{mob}" => Element::Mob,
                words => Element::Words(
                    words
                        .split('|')
                        .map(|alt| alt.split('_').map(String::from).collect())
                        .collect(),
                ),
            };
            if optional {
                Element::Optional(Box::new(element))
            } else {
                element
            }
        })
        .collect()
}

fn match_elements(
    elements: &[Element],
    words: &[&str],
    slots: &mut Slots,
    names: &dyn Names,
    g: &Grammar,
) -> bool {
    let Some((first, rest)) = elements.split_first() else {
        return words.is_empty();
    };
    if let Element::Optional(inner) = first {
        let mut tried = slots.clone();
        if match_one(inner, words, rest, &mut tried, names, g) {
            *slots = tried;
            return true;
        }
        return match_elements(rest, words, slots, names, g);
    }
    match_one(first, words, rest, slots, names, g)
}

fn match_one(
    element: &Element,
    words: &[&str],
    rest: &[Element],
    slots: &mut Slots,
    names: &dyn Names,
    g: &Grammar,
) -> bool {
    match element {
        Element::Words(alternatives) => alternatives.iter().any(|alt| {
            words.len() >= alt.len()
                && alt.iter().zip(words).all(|(p, w)| word_matches(p, w))
                && match_elements(rest, &words[alt.len()..], slots, names, g)
        }),
        Element::Count => [2, 1].into_iter().any(|take| {
            if words.len() < take {
                return false;
            }
            let Some(n) = count(&words[..take], g) else {
                return false;
            };
            let mut tried = slots.clone();
            tried.count = n;
            let ok = match_elements(rest, &words[take..], &mut tried, names, g);
            if ok {
                *slots = tried;
            }
            ok
        }),
        Element::Item | Element::Mob => {
            // Determiners before the name ("the", "des", "deine") are skipped.
            let skip = words
                .iter()
                .take_while(|w| g.determiners.contains(w))
                .count();
            let words = &words[skip..];
            (1..=words.len().min(5)).rev().any(|end| {
                let phrase = words[..end].join(" ");
                let found = match element {
                    Element::Item => names.item(&phrase),
                    _ => names.mob(&phrase),
                };
                let Some(id) = found else {
                    return false;
                };
                let mut tried = slots.clone();
                match element {
                    Element::Item => tried.item = Some(id),
                    _ => tried.mob = Some(id),
                }
                let ok = match_elements(rest, &words[end..], &mut tried, names, g);
                if ok {
                    *slots = tried;
                }
                ok
            })
        }
        Element::Optional(_) => unreachable!("handled by match_elements"),
    }
}

/// A count of one or two words: `10`, `ten`, `dix`, `dix sept`, `a stack`, `half a stack`, `two stacks`,
/// `un`; `some` is a count without a number. `None` when the words are no count.
fn count(words: &[&str], g: &Grammar) -> Option<Option<u32>> {
    let number = |w: &str| {
        g.numbers
            .iter()
            .position(|alts| alts.split('|').any(|a| a == w))
            .map(|i| i as u32 + 1)
    };
    match words {
        [one] => {
            if let Ok(n) = one.trim_end_matches('x').parse::<u32>() {
                return (1..=crate::reply::MAX_COUNT)
                    .contains(&n)
                    .then_some(Some(n));
            }
            if g.stack.contains(one) {
                return Some(Some(64));
            }
            if g.some.contains(one) {
                return Some(None);
            }
            number(one).map(Some)
        }
        [a, b] => {
            if let Some(n) = number(&format!("{a} {b}")) {
                return Some(Some(n));
            }
            if g.half.contains(a) && g.stack.contains(b) {
                return Some(Some(32));
            }
            if g.some_two.contains(&format!("{a} {b}").as_str()) {
                return Some(None);
            }
            let n = a.parse::<u32>().ok().or_else(|| number(a))?;
            g.stack
                .contains(b)
                .then(|| Some((n * 64).min(crate::reply::MAX_COUNT)))
        }
        _ => None,
    }
}

fn strip_fillers(words: &mut Vec<&str>, g: &Grammar) {
    // Request frames at the start ("can you", "est ce que tu peux"), as often as they repeat.
    loop {
        let before = words.len();
        for frame in g.frames {
            let frame: Vec<&str> = frame.split(' ').collect();
            if words.len() > frame.len() && words[..frame.len()] == frame[..] {
                words.drain(..frame.len());
            }
        }
        if words.len() == before {
            break;
        }
    }
    for filler in g.fillers.iter().filter(|f| f.contains(' ')) {
        let filler: Vec<&str> = filler.split(' ').collect();
        while let Some(i) = words
            .windows(filler.len())
            .position(|w| w == filler.as_slice())
        {
            words.drain(i..i + filler.len());
        }
    }
    words.retain(|w| !g.fillers.contains(w));
}

/// Removes negation words; whether there were any.
fn strip_negation(words: &mut Vec<&str>, g: &Grammar) -> bool {
    let before = words.len();
    for pair in g.negations.iter().filter(|n| n.contains(' ')) {
        let pair: Vec<&str> = pair.split(' ').collect();
        while let Some(i) = words.windows(pair.len()).position(|w| w == pair.as_slice()) {
            words.drain(i..i + pair.len());
        }
    }
    words.retain(|w| !g.negations.contains(w));
    words.len() != before
}

struct Grammar {
    intents: &'static [(Kind, &'static str)],
    /// Request frames at the start of a line.
    frames: &'static [&'static str],
    /// Words (or phrases) dropped anywhere.
    fillers: &'static [&'static str],
    negations: &'static [&'static str],
    determiners: &'static [&'static str],
    /// One to twenty, alternatives joined by `|`.
    numbers: [&'static str; 20],
    stack: &'static [&'static str],
    half: &'static [&'static str],
    some: &'static [&'static str],
    some_two: &'static [&'static str],
}

fn grammar(lang: &str) -> &'static Grammar {
    match lang {
        "fr" => &FR,
        "es" => &ES,
        "de" => &DE,
        "pt" => &PT,
        _ => &EN,
    }
}

// Patterns are tried in order, the specific ones of a verb first ("stop following me" stays before
// "stop" stops). Words are written normalised: lowercase, no accents, no apostrophes. Negations are
// taken out before matching, so "don't move" matches `move` (Kind::Move).

static EN: Grammar = Grammar {
    intents: &[
        (
            Kind::Stay,
            "stop following [me|us]; stay [here|there|put|back]; wait [here|there|for_me|up]; hold [this|that|your] position|spot|ground; hold here; guard [this|that|the] spot|place|area|position; guard here; stand still",
        ),
        (Kind::Move, "move; move [a] muscle"),
        (
            Kind::HoldFire,
            "hold [your] fire; cease fire|firing; ceasefire; stop attacking|shooting|fighting|hitting; stand down",
        ),
        (
            Kind::Follow,
            "follow [me|us]; come with|along [me|us]; stick with|close [me|us|to_me]; keep up; with me; on me",
        ),
        (
            Kind::Come,
            "come [over] [here|back|to_me]; get [over] here; over here; here; to me; regroup",
        ),
        (
            Kind::Stop,
            "stop [it|that|everything]; cancel [that|it]; halt; freeze; never mind; nevermind; forget [it|that]; abort",
        ),
        (
            Kind::GoTo,
            "go|move|walk|run [over] there; go that way; go|move|walk|run to|towards|toward [that|this|the|those] [{item}]; over there; there",
        ),
        (
            Kind::Fire,
            "fire [at_will]; open fire; weapons free; shoot; you can attack|fight [again|now]",
        ),
        (
            Kind::Attack,
            "attack|kill|fight|hit|slay [it|that|them|him|her|this] [{mob}]; attack|kill|fight|hit|shoot|slay [the|that|this|those] {mob}; get|shoot him|her|it|them; go get [it|them|him|her]",
        ),
        (
            Kind::Defend,
            "defend|protect|cover|guard me|us; watch my back; keep me safe; defend|protect",
        ),
        (Kind::Jump, "jump [up]"),
        (
            Kind::StandUp,
            "stand [up]; get up; stop crouching|sneaking; uncrouch; unsneak",
        ),
        (Kind::Crouch, "crouch [down]; sneak; duck; get down"),
        (
            Kind::Eat,
            "eat [something|food|up]; go eat; have [a] snack|bite",
        ),
        (
            Kind::Sleep,
            "sleep; go [to] sleep|bed; bedtime; bed time; get [in|into|to] bed",
        ),
        (Kind::Craft, "craft|make|build [me|us] [{n}] {item}"),
        (
            Kind::Give,
            "give|throw|toss|pass|hand|drop me|us [{n}] [of] {item}; give|throw|toss|pass|hand [{n}] [of] {item} [to_me]; i want [{n}] [of] {item}",
        ),
        (
            Kind::Collect,
            "get|collect|gather|mine|chop|cut|bring|fetch|grab|farm|find|harvest [me|us] [{n}] [of] {item}; go get|collect|gather|mine|chop|fetch|grab [me|us] [{n}] [of] {item}; i|we need [{n}] [more] [of] {item}",
        ),
    ],
    frames: &[
        "can you",
        "could you",
        "would you",
        "will you",
        "can u",
        "i want you to",
        "i need you to",
        "go ahead and",
        "you should",
        "lets",
        "let s",
        "please",
        "pls",
        "plz",
        "ok",
        "okay",
        // "no, follow me" corrects the last order; "no, don't shoot" still has its own negation.
        "no",
        "nope",
        "alright",
        "hey",
        "yo",
    ],
    fillers: &[
        "please",
        "pls",
        "plz",
        "now",
        "quick",
        "quickly",
        "asap",
        "right now",
        "thanks",
        "thx",
        "bro",
        "dude",
        "buddy",
        "mate",
        "again",
    ],
    negations: &["don t", "do not", "dont", "never", "doesn t"],
    determiners: &[
        "the", "a", "an", "some", "of", "your", "my", "that", "this", "those", "these", "more",
    ],
    numbers: [
        "one|a|an",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
    ],
    stack: &["stack", "stacks"],
    half: &["half"],
    some: &["some", "more", "few", "several"],
    some_two: &["a few", "a couple", "a bunch"],
};

static FR: Grammar = Grammar {
    intents: &[
        (
            Kind::Stay,
            "arrete|arretes|stop [de] [me|nous] suivre; reste|restes|restez [ici|la|la_bas|sur_place]; attends|attend|attendez [moi|nous] [ici|la|la_bas]; bouge plus; garde [ce|cette|la] coin|position|zone|place; garde ici; tiens [la|ta] position",
        ),
        (Kind::Move, "bouge|bouges|bougez"),
        (
            Kind::HoldFire,
            "cessez|cesse [le] feu; arrete|arretes|stop [d|de] attaquer|tirer|taper|frapper; calme toi",
        ),
        (
            Kind::Follow,
            "suis|suit|sui|suivez|suives moi|nous; viens|venez avec moi|nous; avec moi; me suivre; me suis; suis; suit",
        ),
        (
            Kind::Come,
            "viens|venez|vient [ici|la|par_ici|vers_moi|me_voir]; ramene toi; amene toi; approche [toi]; reviens|revenez; rapplique; ici",
        ),
        (
            Kind::Stop,
            "stop; stoppe; arrete|arretes|arretez [tout|ca]; arret; halte; annule [ca]; laisse tomber; oublie [ca]",
        ),
        (
            Kind::GoTo,
            "va|vas|allez [par] la|la_bas; vas y; va|vas|allez vers|a|au|jusqu_a [ce|cet|cette|le|la|l|les] [{item}]; la bas",
        ),
        (
            Kind::Fire,
            "feu [a_volonte]; ouvre|ouvrez le feu; tire|tirez",
        ),
        (
            Kind::Attack,
            "attaque|attaques|attaquez|tue|tues|tuez|tape|tapes|frappe|bute|defonce|degomme [le|la|les|l|ce|cet|cette|ces|ca] [{mob}]",
        ),
        (
            Kind::Defend,
            "defends|defend|defendez|protege|proteges|protegez|couvre|couvrez moi|nous; surveille mes arrieres; defends|protege",
        ),
        (Kind::Jump, "saute|sautes|sautez"),
        (Kind::StandUp, "leve|releve|levez toi|vous; debout"),
        (
            Kind::Crouch,
            "accroupis|accroupi|baisse|baisses|accroupissez toi|vous; accroupis|accroupi",
        ),
        (
            Kind::Eat,
            "mange|manges|mangez [quelque_chose|un_truc|un_peu]; va manger",
        ),
        (
            Kind::Sleep,
            "dors|dort|dormez; va|vas|allez dormir; au lit; dodo; va|vas te coucher; couche toi",
        ),
        (
            Kind::Craft,
            "fabrique|fabriques|fabriquez|craft|crafte|craftes|fais|fait|faites|construis [moi|nous] [{n}] {item}",
        ),
        (
            Kind::Give,
            "donne|donnes|donnez|lance|lances|passe|passes|file|files|jette|jettes|envoie moi|nous [{n}] [de|d|du|des] {item}; donne|donnes|donnez|file [{n}] [de|d|du|des] {item}; je veux [{n}] [de|d|du|des] {item}",
        ),
        (
            Kind::Collect,
            "va|vas chercher|recuperer|miner|couper|ramasser [moi|nous] [{n}] [de|d|du|des] {item}; ramasse|ramasses|recolte|recoltes|recupere|recuperes|mine|mines|coupe|coupes|trouve|trouves|prends|prend|ramene|ramenes|rapporte|cherche [moi|nous] [{n}] [de|d|du|des] {item}; j|on ai|a besoin [de|d] [{n}] [de|d|du|des] {item}; il [me|nous] faut [{n}] [de|d|du|des] {item}",
        ),
    ],
    frames: &[
        "est ce que tu peux",
        "est ce que tu pourrais",
        "tu peux",
        "peux tu",
        "tu pourrais",
        "pourrais tu",
        "tu veux bien",
        "je veux que tu",
        "j aimerais que tu",
        "il faut que tu",
        "s il te plait",
        "s il vous plait",
        "stp",
        "svp",
        "bon",
        "non",
        "ok",
        "hey",
        "he",
        "eh",
        "oh",
    ],
    fillers: &[
        "stp",
        "svp",
        "vite",
        "maintenant",
        "merci",
        "mec",
        "frero",
        "frerot",
        "gros",
        "steuplait",
        "s il te plait",
        "s il vous plait",
        "encore",
        "tout de suite",
    ],
    negations: &["ne", "n", "pas", "jamais"],
    determiners: &[
        "le", "la", "les", "l", "un", "une", "des", "du", "de", "d", "ce", "cet", "cette", "ces",
        "tes", "ton", "ta", "mes", "mon", "ma",
    ],
    numbers: [
        "un|une", "deux", "trois", "quatre", "cinq", "six", "sept", "huit", "neuf", "dix", "onze",
        "douze", "treize", "quatorze", "quinze", "seize", "dix sept", "dix huit", "dix neuf",
        "vingt",
    ],
    stack: &["stack", "stacks", "pile", "piles"],
    half: &["demi"],
    some: &["quelques", "plein", "beaucoup"],
    some_two: &["un peu", "un tas"],
};

static ES: Grammar = Grammar {
    intents: &[
        (
            Kind::Stay,
            "deja|para de seguirme|seguirnos; quedate|quedense|quedese [aqui|ahi|alli|quieto]; espera|esperen [aqui|ahi|alli]; esperame; quieto; manten [la] posicion; vigila|guarda aqui",
        ),
        (Kind::Move, "te muevas; muevas"),
        (
            Kind::HoldFire,
            "alto el fuego; deja|para de atacar|disparar|pelear; calmate",
        ),
        (
            Kind::Follow,
            "sigueme|siganme|siguenos|sigame; sigue|sigan [me|nos]; me|nos sigas|sigues|sigue|siga; ven|vente|venga conmigo; conmigo",
        ),
        (
            Kind::Come,
            "ven|venga|vente|vengan [aqui|aca|para_aca]; acercate; vuelve|regresa; aqui",
        ),
        (
            Kind::Stop,
            "para|parale; detente; alto; basta; cancela [eso]; dejalo; olvidalo; stop",
        ),
        (
            Kind::GoTo,
            "ve|vete|anda [para] alli|ahi|alla; ve|vete|anda a|hacia|hasta [ese|esa|el|la|los|las|aquel|aquella] [{item}]; alla",
        ),
        (
            Kind::Fire,
            "fuego [a_discrecion]; abre fuego; dispara|disparen|dispares",
        ),
        (
            Kind::Attack,
            "ataca|atacalo|atacala|atacalos|ataques|mata|matalo|matala|matalos|pega|golpea|dale [a|al|a_la|a_los|a_las|el|la|ese|esa|aquel] [{mob}]",
        ),
        (
            Kind::Defend,
            "defiendeme|protegeme|cubreme|cuidame|defiendenos|protegenos; defiende|protege [me|nos]; cuida mi espalda",
        ),
        (Kind::Jump, "salta"),
        (Kind::StandUp, "levantate; de pie; parate"),
        (Kind::Crouch, "agachate|agachese"),
        (Kind::Eat, "come|comete [algo|un_poco]; a comer; ve a comer"),
        (
            Kind::Sleep,
            "duerme; a dormir; vete|ve a dormir|la_cama; a la cama",
        ),
        (
            Kind::Craft,
            "fabrica|fabricame|craftea|crafteame|haz|hazme|crea|creame|construye|construyeme [{n}] {item}",
        ),
        (
            Kind::Give,
            "dame|danos|pasame|tirame|lanzame|dejame [{n}] [de] {item}; quiero [{n}] [de] {item}",
        ),
        (
            Kind::Collect,
            "consigue|consigueme|recoge|recogeme|junta|juntame|busca|buscame|trae|traeme|mina|corta|cortame|tala|agarra [{n}] [de] {item}; ve|anda por|a_buscar|a_por [{n}] [de] {item}; necesito|necesitamos [{n}] [mas] [de] {item}",
        ),
    ],
    frames: &[
        "puedes por favor",
        "puedes",
        "podrias",
        "quiero que",
        "necesito que",
        "por favor",
        "porfa",
        "porfis",
        "vale",
        "oye",
        "ok",
        "eh",
    ],
    fillers: &[
        "porfa",
        "porfis",
        "ya",
        "ahora",
        "rapido",
        "gracias",
        "tio",
        "bro",
        "wey",
        "guey",
        "otra vez",
        "por favor",
    ],
    negations: &["no", "nunca", "jamas"],
    determiners: &[
        "el", "la", "los", "las", "un", "una", "unos", "unas", "de", "del", "tu", "tus", "mi",
        "mis", "ese", "esa", "esos", "esas", "este", "esta", "mas", "al",
    ],
    numbers: [
        "uno|una|un",
        "dos",
        "tres",
        "cuatro",
        "cinco",
        "seis",
        "siete",
        "ocho",
        "nueve",
        "diez",
        "once",
        "doce",
        "trece",
        "catorce",
        "quince",
        "dieciseis",
        "diecisiete",
        "dieciocho",
        "diecinueve",
        "veinte",
    ],
    stack: &["stack", "stacks", "pila", "pilas"],
    half: &["medio", "media"],
    some: &["algunos", "algunas", "unos", "unas", "varios"],
    some_two: &["un poco", "un monton"],
};

static DE: Grammar = Grammar {
    intents: &[
        (
            Kind::Stay,
            "hor|hoer|horen auf mir|uns zu folgen; bleib|bleibe|bleibt [hier|da|dort|stehen]; warte|wart|wartet [hier|da|dort|auf_mich]; halt|halte die stellung|position; bewache|bewach [den|diesen] platz|ort|bereich",
        ),
        (Kind::Move, "beweg|bewege dich; bewegen"),
        (
            Kind::HoldFire,
            "feuer einstellen; hor|hoer auf anzugreifen|zu_schiessen|zu_kampfen; waffen runter",
        ),
        (
            Kind::Follow,
            "folge|folg|folgt mir|uns; komm|kommt mit [mir|uns]; mir|uns nach; hinterher",
        ),
        (
            Kind::Come,
            "komm|kommt|komme [her|hierher|zu_mir|zuruck|zurueck|mal_her]; her zu mir; hierher",
        ),
        (
            Kind::Stop,
            "stopp; stop; halt; hor|hoer auf [damit]; abbrechen; brich ab; lass das|es [sein]; vergiss es",
        ),
        (
            Kind::GoTo,
            "geh|gehe|lauf|laufe dahin|dorthin|hin|da|dort [hin]; geh|gehe|lauf|laufe zu|zum|zur [dem|der|diesem|dieser|den] [{item}]; dahin; dorthin",
        ),
        (
            Kind::Fire,
            "feuer [frei]; schiess|schiesse [los]; du darfst [wieder] angreifen",
        ),
        (
            Kind::Attack,
            "greif|greife [ihn|sie|es|den|die|das|diesen|diese] [{mob}] an; attackier|attackiere|tote|toete|hau|haue|erledige|kill [ihn|sie|es|den|die|das|diesen|diese] [{mob}]; angreifen; greif an",
        ),
        (
            Kind::Defend,
            "verteidige|verteidig|beschutze|beschuetze|schutze|schuetze|deck|decke mich|uns; pass auf mich auf; gib mir deckung; verteidige|beschutze",
        ),
        (Kind::Jump, "spring|springe|hupf|huepf|hupfe"),
        (Kind::StandUp, "steh|steht auf; aufstehen"),
        (
            Kind::Crouch,
            "duck|ducke dich; ducken; schleich|schleiche [dich]; geh in die hocke",
        ),
        (Kind::Eat, "iss|esse [was|etwas]; essen; geh essen"),
        (
            Kind::Sleep,
            "schlaf|schlafe; geh|gehe schlafen; ab ins bett; schlafen; ins bett",
        ),
        (
            Kind::Craft,
            "craft|crafte|bau|baue|stell|mach|mache [mir|uns] [{n}] {item} [her]",
        ),
        (
            Kind::Give,
            "gib|gebt|wirf|reich|reiche mir|uns [{n}] [von] {item}; ich will|mochte|moechte [{n}] {item}",
        ),
        (
            Kind::Collect,
            "hol|hole|holt|sammle|sammel|besorg|besorge|bring|bringe|fall|falle|finde|such|suche [mir|uns] [{n}] [von] {item}; bau|baue [mir|uns] [{n}] {item} ab; ich|wir brauche|brauchen|brauch [{n}] [mehr] {item}",
        ),
    ],
    frames: &[
        "kannst du bitte",
        "kannst du",
        "konntest du",
        "koenntest du",
        "ich will dass du",
        "bitte",
        "mal",
        "los",
        "okay",
        "ok",
        "hey",
        "he",
        "ey",
    ],
    fillers: &[
        "bitte", "mal", "jetzt", "schnell", "sofort", "danke", "doch", "bro", "alter", "digga",
        "nochmal", "wieder", "endlich",
    ],
    negations: &["nicht", "kein", "keine", "nie", "niemals"],
    determiners: &[
        "der", "die", "das", "den", "dem", "des", "ein", "eine", "einen", "einem", "deine", "dein",
        "deinen", "meine", "mein", "diese", "dieser", "diesen", "mehr", "von",
    ],
    numbers: [
        "ein|eine|einen|eins",
        "zwei",
        "drei",
        "vier",
        "funf|fuenf",
        "sechs",
        "sieben",
        "acht",
        "neun",
        "zehn",
        "elf",
        "zwolf|zwoelf",
        "dreizehn",
        "vierzehn",
        "funfzehn|fuenfzehn",
        "sechzehn",
        "siebzehn",
        "achtzehn",
        "neunzehn",
        "zwanzig",
    ],
    stack: &["stack", "stacks", "stapel"],
    half: &["halben", "halber", "halbe"],
    some: &["paar", "etwas", "einige", "mehrere"],
    some_two: &["ein paar", "ein bisschen", "ein haufen"],
};

static PT: Grammar = Grammar {
    intents: &[
        (
            Kind::Stay,
            "para|pare de me seguir; fica|fique [aqui|ai|ali|parado|parada]; espera|espere [aqui|ai|ali|por_mim]; parado; parada; segura|segure [a] posicao; vigia|vigie aqui",
        ),
        (Kind::Move, "se mexa|mexe; se mova|move; mexe; mexa"),
        (
            Kind::HoldFire,
            "cessar fogo; para|pare de atacar|atirar|bater; calma",
        ),
        (
            Kind::Follow,
            "me siga|segue|sigam; siga|segue|sigam me; vem|venha comigo; comigo",
        ),
        (
            Kind::Come,
            "vem|venha|chega [aqui|ca|pra_ca|para_ca|ate_mim]; volta|volte; aqui",
        ),
        (
            Kind::Stop,
            "para; pare; chega; cancela|cancele [isso]; deixa pra la; esquece|esqueca [isso]; stop",
        ),
        (
            Kind::GoTo,
            "vai|va|ande [para] la|ali|pra_la; vai|va|ande para|pra|ate|ao [aquele|aquela|esse|essa|o|a|os|as] [{item}]; la",
        ),
        (Kind::Fire, "fogo [a_vontade]; abre|abra fogo; atira|atire"),
        (
            Kind::Attack,
            "ataca|ataque|mata|mate|bate|bata|acaba_com|acabe_com [o|a|os|as|esse|essa|aquele|aquela|ele|ela] [{mob}]",
        ),
        (
            Kind::Defend,
            "me defende|defenda|protege|proteja|cobre|cubra; defende|defenda|protege|proteja|cobre me; cuida das minhas costas",
        ),
        (Kind::Jump, "pula|pule|salta|salte"),
        (
            Kind::StandUp,
            "levanta|levante [se]; fica|fique de pe; de pe",
        ),
        (Kind::Crouch, "agacha|agache|abaixa|abaixe [se]"),
        (
            Kind::Eat,
            "come|coma [alguma_coisa|algo|um_pouco]; vai comer",
        ),
        (
            Kind::Sleep,
            "dorme|durma; vai|va dormir; pra|para cama; hora de dormir",
        ),
        (
            Kind::Craft,
            "fabrica|fabrique|crafta|crafte|faz|faca|cria|crie|constroi|construa [pra_mim|para_mim|pra_gente] [{n}] {item}; me faz|faca [{n}] {item}",
        ),
        (
            Kind::Give,
            "me da|de|passa|passe|joga|jogue|manda|mande [{n}] [de] {item}; da|de|passa|joga me [{n}] [de] {item}; quero [{n}] [de] {item}",
        ),
        (
            Kind::Collect,
            "pega|pegue|coleta|colete|junta|junte|busca|busque|minera|minere|corta|corte|traz|traga|arruma|arrume|consegue|consiga [pra_mim|para_mim|pra_gente] [{n}] [de] {item}; vai buscar|pegar [{n}] [de] {item}; preciso|precisamos [de] [{n}] [mais] [de] {item}",
        ),
    ],
    frames: &[
        "voce pode",
        "vc pode",
        "pode",
        "podes",
        "poderia",
        "quero que voce",
        "quero que",
        "preciso que voce",
        "por favor",
        "pfv",
        "pf",
        "ok",
        "ei",
        "oi",
        "eh",
    ],
    fillers: &[
        "pfv",
        "pf",
        "agora",
        "rapido",
        "ja",
        "obrigado",
        "obrigada",
        "mano",
        "cara",
        "brother",
        "de novo",
        "por favor",
    ],
    negations: &["nao", "nunca", "jamais"],
    determiners: &[
        "o", "a", "os", "as", "um", "uma", "uns", "umas", "de", "do", "da", "dos", "das", "teu",
        "tua", "seu", "sua", "seus", "suas", "esse", "essa", "esses", "essas", "mais",
    ],
    numbers: [
        "um|uma",
        "dois|duas",
        "tres",
        "quatro",
        "cinco",
        "seis",
        "sete",
        "oito",
        "nove",
        "dez",
        "onze",
        "doze",
        "treze",
        "catorze|quatorze",
        "quinze",
        "dezesseis|dezasseis",
        "dezessete|dezassete",
        "dezoito",
        "dezenove|dezanove",
        "vinte",
    ],
    stack: &["stack", "stacks", "pilha", "pilhas", "pack"],
    half: &["meio", "meia"],
    some: &["alguns", "algumas", "uns", "umas", "varios", "varias"],
    some_two: &["um pouco", "um monte"],
};

/// What a classifier made of a line: intents with their confidence (0–1).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Classification {
    pub ranked: Vec<(Intent, f32)>,
    /// Chit-chat, a question, a plan: the brain answers even if an intent ranks high.
    pub needs_brain: bool,
}

/// The second stage: a model that recognises commands the rules don't ("we're out of wood", "watch
/// out behind me"). It may take tens of milliseconds, so with one set, call [`Pipeline::route`] off the
/// game thread.
pub trait IntentClassifier {
    /// `None` when it can't tell or isn't running.
    fn classify(&self, utterance: &str, language: &str) -> Option<Classification>;
}

/// No classifier: rules, then the brain.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoClassifier;

impl IntentClassifier for NoClassifier {
    fn classify(&self, _: &str, _: &str) -> Option<Classification> {
        None
    }
}

/// Which stage took a line.
#[derive(Clone, Debug, PartialEq)]
pub enum Route {
    /// Run it now and acknowledge it from the phrase bank.
    Reflex {
        intent: Intent,
        by: Stage,
    },
    Brain,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stage {
    Rules,
    Classifier { confidence: f32 },
}

/// Rules, then the classifier when its best intent reaches `threshold` and leads the runner-up by
/// `margin`, then the brain.
pub struct Pipeline<C: IntentClassifier = NoClassifier> {
    pub classifier: C,
    pub threshold: f32,
    pub margin: f32,
}

impl Default for Pipeline<NoClassifier> {
    fn default() -> Self {
        Pipeline::new(NoClassifier)
    }
}

impl<C: IntentClassifier> Pipeline<C> {
    pub fn new(classifier: C) -> Pipeline<C> {
        Pipeline {
            classifier,
            threshold: 0.75,
            margin: 0.1,
        }
    }

    pub fn route(&self, utterance: &str, language: &str, names: &dyn Names) -> Route {
        if let Parse::Intent(intent) = parse(utterance, language, names) {
            return Route::Reflex {
                intent,
                by: Stage::Rules,
            };
        }
        let Some(c) = self.classifier.classify(utterance, language) else {
            return Route::Brain;
        };
        if c.needs_brain {
            return Route::Brain;
        }
        let mut ranked = c.ranked;
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        match ranked.as_slice() {
            [(intent, top), rest @ ..]
                if *top >= self.threshold
                    && rest
                        .first()
                        .is_none_or(|(_, second)| top - second >= self.margin) =>
            {
                Route::Reflex {
                    intent: intent.clone(),
                    by: Stage::Classifier { confidence: *top },
                }
            }
            _ => Route::Brain,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slice of the game's names in the five languages (the game passes its language files).
    pub(crate) fn names() -> NameIndex {
        let mut n = NameIndex::new();
        for (id, names) in [
            (
                "oak_log",
                &[
                    "Oak Log",
                    "Bûche de chêne",
                    "Tronco de roble",
                    "Eichenstamm",
                    "Tronco de carvalho",
                ][..],
            ),
            (
                "cobblestone",
                &[
                    "Cobblestone",
                    "Pierre taillée",
                    "Roca",
                    "Bruchstein",
                    "Pedregulho",
                ],
            ),
            (
                "iron_ingot",
                &[
                    "Iron Ingot",
                    "Lingot de fer",
                    "Lingote de hierro",
                    "Eisenbarren",
                    "Barra de ferro",
                ],
            ),
            (
                "crafting_table",
                &[
                    "Crafting Table",
                    "Table de craft",
                    "Mesa de trabajo",
                    "Werkbank",
                    "Bancada de trabalho",
                ],
            ),
            (
                "wooden_pickaxe",
                &[
                    "Wooden Pickaxe",
                    "Pioche en bois",
                    "Pico de madera",
                    "Holzspitzhacke",
                    "Picareta de madeira",
                ],
            ),
            ("bread", &["Bread", "Pain", "Pan", "Brot", "Pão"]),
            ("torch", &["Torch", "Torche", "Antorcha", "Fackel", "Tocha"]),
        ] {
            for name in names {
                n.add_item(id, name);
            }
        }
        for (id, names) in [
            ("zombie", &["Zombie", "Zombi", "Zumbi"][..]),
            ("creeper", &["Creeper"]),
            (
                "skeleton",
                &["Skeleton", "Squelette", "Esqueleto", "Skelett"],
            ),
            (
                "spider",
                &["Spider", "Araignée", "Araña", "Spinne", "Aranha"],
            ),
            (
                "villager",
                &[
                    "Villager",
                    "Villageois",
                    "Aldeano",
                    "Dorfbewohner",
                    "Aldeão",
                ],
            ),
        ] {
            for name in names {
                n.add_mob(id, name);
            }
        }
        n
    }

    fn intent(text: &str, lang: &str) -> Intent {
        match parse(text, lang, &names()) {
            Parse::Intent(i) => i,
            Parse::Unclear => panic!("{text:?} ({lang}) was unclear"),
        }
    }

    fn unclear(text: &str, lang: &str) {
        assert_eq!(
            parse(text, lang, &names()),
            Parse::Unclear,
            "{text:?} ({lang})"
        );
    }

    fn collect(item: &str, count: Option<u32>) -> Intent {
        Intent::Collect {
            item: item.into(),
            count,
        }
    }

    fn there(kind: Option<&str>) -> Intent {
        Intent::GoTo {
            target: Target::Crosshair {
                kind: kind.map(String::from),
            },
        }
    }

    #[test]
    fn english_commands() {
        let cases: &[(&str, Intent)] = &[
            ("follow me", Intent::Follow),
            ("Follow me!", Intent::Follow),
            ("can you follow me please", Intent::Follow),
            ("folow me", Intent::Follow),
            ("come with me", Intent::Follow),
            ("stay here", Intent::Stay),
            ("wait here", Intent::Stay),
            ("stop following me", Intent::Stay),
            ("come here", Intent::Come),
            ("get over here", Intent::Come),
            ("stop", Intent::Stop),
            ("never mind", Intent::Stop),
            ("go there", there(None)),
            ("go to that tree", there(Some("#logs"))),
            ("attack", Intent::Attack { target: None }),
            (
                "kill that zombie",
                Intent::Attack {
                    target: Some("zombie".into()),
                },
            ),
            (
                "attack the skeletons",
                Intent::Attack {
                    target: Some("skeleton".into()),
                },
            ),
            ("defend me", Intent::Defend),
            ("watch my back", Intent::Defend),
            ("hold fire", Intent::HoldFire(true)),
            ("hold your fire", Intent::HoldFire(true)),
            ("fire", Intent::HoldFire(false)),
            ("open fire", Intent::HoldFire(false)),
            ("jump", Intent::Jump),
            ("crouch", Intent::Crouch(true)),
            ("stand up", Intent::Crouch(false)),
            ("eat something", Intent::Eat),
            ("go to sleep", Intent::Sleep),
            ("get 10 oak logs", collect("oak_log", Some(10))),
            ("collect ten cobblestone", collect("cobblestone", Some(10))),
            ("get me some wood", collect("#logs", None)),
            ("mine a stack of cobble", collect("cobblestone", Some(64))),
            ("i need 3 iron", collect("iron_ingot", Some(3))),
            (
                "give me 5 bread",
                Intent::Give {
                    item: "bread".into(),
                    count: Some(5),
                },
            ),
            (
                "throw me your torches",
                Intent::Give {
                    item: "torch".into(),
                    count: None,
                },
            ),
            (
                "craft a crafting table",
                Intent::Craft {
                    item: "crafting_table".into(),
                    count: Some(1),
                },
            ),
            (
                "make me 2 wooden pickaxes",
                Intent::Craft {
                    item: "wooden_pickaxe".into(),
                    count: Some(2),
                },
            ),
        ];
        for (text, want) in cases {
            assert_eq!(&intent(text, "en_us"), want, "{text}");
        }
    }

    #[test]
    fn french_with_accents_and_typos() {
        let cases: &[(&str, Intent)] = &[
            ("suis-moi", Intent::Follow),
            ("Suis moi !", Intent::Follow),
            ("suit moi", Intent::Follow),
            ("tu peux me suivre stp", Intent::Follow),
            ("viens avec moi", Intent::Follow),
            ("reste ici", Intent::Stay),
            ("attends-moi là", Intent::Stay),
            ("arrête de me suivre", Intent::Stay),
            ("viens ici", Intent::Come),
            ("ramène-toi", Intent::Come),
            ("arrête", Intent::Stop),
            ("laisse tomber", Intent::Stop),
            ("va là-bas", there(None)),
            ("va vers cet arbre", there(Some("#logs"))),
            ("attaque", Intent::Attack { target: None }),
            (
                "tue le zombie",
                Intent::Attack {
                    target: Some("zombie".into()),
                },
            ),
            (
                "attaque l'araignée",
                Intent::Attack {
                    target: Some("spider".into()),
                },
            ),
            ("défends-moi", Intent::Defend),
            ("protège moi", Intent::Defend),
            ("cessez le feu", Intent::HoldFire(true)),
            ("feu à volonté", Intent::HoldFire(false)),
            ("saute", Intent::Jump),
            ("accroupis-toi", Intent::Crouch(true)),
            ("lève-toi", Intent::Crouch(false)),
            ("mange quelque chose", Intent::Eat),
            ("va dormir", Intent::Sleep),
            ("ramasse 10 bûches de chêne", collect("oak_log", Some(10))),
            ("va chercher du bois", collect("#logs", None)),
            ("recupere dix buches de chene", collect("oak_log", Some(10))),
            ("récupère 3 lingots de fer", collect("iron_ingot", Some(3))),
            (
                "j'ai besoin de 20 pierre taillée",
                collect("cobblestone", Some(20)),
            ),
            (
                "donne-moi 5 pains",
                Intent::Give {
                    item: "bread".into(),
                    count: Some(5),
                },
            ),
            (
                "file moi des torches",
                Intent::Give {
                    item: "torch".into(),
                    count: None,
                },
            ),
            (
                "fabrique une table de craft",
                Intent::Craft {
                    item: "crafting_table".into(),
                    count: Some(1),
                },
            ),
            (
                "craft moi une pioche en bois s'il te plaît",
                Intent::Craft {
                    item: "wooden_pickaxe".into(),
                    count: Some(1),
                },
            ),
            (
                "fabrique une pioche en boi",
                Intent::Craft {
                    item: "wooden_pickaxe".into(),
                    count: Some(1),
                },
            ),
            ("ramasse dix sept bûches", collect("#logs", Some(17))),
        ];
        for (text, want) in cases {
            assert_eq!(&intent(text, "fr_fr"), want, "{text}");
        }
    }

    #[test]
    fn spanish_german_portuguese() {
        let cases: &[(&str, &str, Intent)] = &[
            ("sígueme", "es_es", Intent::Follow),
            ("ven aquí", "es_es", Intent::Come),
            ("quédate aquí", "es_es", Intent::Stay),
            ("para", "es_es", Intent::Stop),
            (
                "ataca al zombi",
                "es_es",
                Intent::Attack {
                    target: Some("zombie".into()),
                },
            ),
            ("defiéndeme", "es_es", Intent::Defend),
            ("alto el fuego", "es_es", Intent::HoldFire(true)),
            ("agáchate", "es_es", Intent::Crouch(true)),
            (
                "consígueme 10 troncos de roble",
                "es_es",
                collect("oak_log", Some(10)),
            ),
            (
                "dame 3 lingotes de hierro",
                "es_es",
                Intent::Give {
                    item: "iron_ingot".into(),
                    count: Some(3),
                },
            ),
            ("come algo", "es_es", Intent::Eat),
            ("folge mir", "de_de", Intent::Follow),
            ("bleib hier", "de_de", Intent::Stay),
            ("komm her", "de_de", Intent::Come),
            ("hör auf", "de_de", Intent::Stop),
            (
                "greif den Zombie an",
                "de_de",
                Intent::Attack {
                    target: Some("zombie".into()),
                },
            ),
            ("beschütze mich", "de_de", Intent::Defend),
            ("Feuer einstellen", "de_de", Intent::HoldFire(true)),
            ("steh auf", "de_de", Intent::Crouch(false)),
            (
                "hol mir 10 Eichenstämme",
                "de_de",
                collect("oak_log", Some(10)),
            ),
            (
                "bau zehn Bruchstein ab",
                "de_de",
                collect("cobblestone", Some(10)),
            ),
            (
                "gib mir 5 Brot",
                "de_de",
                Intent::Give {
                    item: "bread".into(),
                    count: Some(5),
                },
            ),
            (
                "bau mir eine Werkbank",
                "de_de",
                Intent::Craft {
                    item: "crafting_table".into(),
                    count: Some(1),
                },
            ),
            ("me siga", "pt_br", Intent::Follow),
            ("vem aqui", "pt_br", Intent::Come),
            ("fica aqui", "pt_br", Intent::Stay),
            ("pare", "pt_br", Intent::Stop),
            (
                "ataca o zumbi",
                "pt_br",
                Intent::Attack {
                    target: Some("zombie".into()),
                },
            ),
            ("me protege", "pt_br", Intent::Defend),
            ("cessar fogo", "pt_br", Intent::HoldFire(true)),
            ("pula", "pt_br", Intent::Jump),
            (
                "pega 10 troncos de carvalho",
                "pt_br",
                collect("oak_log", Some(10)),
            ),
            (
                "me dá 5 pães",
                "pt_br",
                Intent::Give {
                    item: "bread".into(),
                    count: Some(5),
                },
            ),
            (
                "faz uma bancada de trabalho",
                "pt_br",
                Intent::Craft {
                    item: "crafting_table".into(),
                    count: Some(1),
                },
            ),
            ("vai dormir", "pt_br", Intent::Sleep),
        ];
        for (text, lang, want) in cases {
            assert_eq!(&intent(text, lang), want, "{text} ({lang})");
        }
    }

    #[test]
    fn negations_count() {
        assert_eq!(intent("don't follow me", "en_us"), Intent::Stay);
        assert_eq!(intent("ne me suis pas", "fr_fr"), Intent::Stay);
        assert_eq!(intent("me suis pas", "fr_fr"), Intent::Stay);
        assert_eq!(intent("no me sigas", "es_es"), Intent::Stay);
        assert_eq!(intent("folge mir nicht", "de_de"), Intent::Stay);
        assert_eq!(intent("não me siga", "pt_br"), Intent::Stay);
        assert_eq!(intent("don't shoot", "en_us"), Intent::HoldFire(true));
        assert_eq!(intent("n'attaque pas", "fr_fr"), Intent::HoldFire(true));
        assert_eq!(intent("no ataques", "es_es"), Intent::HoldFire(true));
        assert_eq!(intent("nicht angreifen", "de_de"), Intent::HoldFire(true));
        assert_eq!(intent("não atire", "pt_br"), Intent::HoldFire(true));
        assert_eq!(intent("don't move", "en_us"), Intent::Stay);
        assert_eq!(intent("ne bouge pas", "fr_fr"), Intent::Stay);
        assert_eq!(intent("no te muevas", "es_es"), Intent::Stay);
        assert_eq!(intent("beweg dich nicht", "de_de"), Intent::Stay);
        assert_eq!(intent("não se mexa", "pt_br"), Intent::Stay);
        unclear("move", "en_us");
        unclear("don't get wood", "en_us");
        unclear("ne mange pas", "fr_fr");
        assert_eq!(intent("don't attack the zombie", "en_us"), Intent::Stop);
        assert_eq!(
            intent("NO, don't attack the villager!", "en_us"),
            Intent::Stop
        );
        assert_eq!(
            intent("NON, attaque pas le villageois !", "fr_fr"),
            Intent::Stop
        );
        assert_eq!(intent("no ataques al aldeano", "es_es"), Intent::Stop);
    }

    #[test]
    fn the_game_language_wins_where_languages_overlap() {
        assert_eq!(intent("come", "en_us"), Intent::Come);
        assert_eq!(intent("come", "es_es"), Intent::Eat);
        assert_eq!(intent("come", "pt_br"), Intent::Eat);
        assert_eq!(
            intent("follow me", "fr_fr"),
            Intent::Follow,
            "English in a French game"
        );
        assert_eq!(intent("para", "pt_br"), Intent::Stop);
    }

    #[test]
    fn everything_else_is_for_the_next_stage() {
        for text in [
            "what do you think of my house?",
            "tu as vu le coucher de soleil",
            "get 3 blarg",
            "build a castle",
            "follow the river to the village",
            "",
            "lol",
            "how many diamonds do we have",
            "go to the nether and find a fortress then come back",
            "I need you to be nicer",
        ] {
            unclear(text, "en_us");
        }
    }

    #[test]
    fn addressed_to_the_companion() {
        assert_eq!(addressed("@Kev follow me", "Kev", false), Some("follow me"));
        assert_eq!(addressed("kev, suis-moi", "Kev", false), Some("suis-moi"));
        assert_eq!(addressed("Kev: stop", "Kev", false), Some("stop"));
        assert_eq!(
            addressed("hey Kev get wood", "Kev", false),
            Some("get wood")
        );
        assert_eq!(addressed("Kev follow me", "Kev", false), Some("follow me"));
        assert_eq!(addressed("follow me, Kev", "Kev", false), Some("follow me"));
        assert_eq!(addressed("Kev", "Kev", false), Some(""));
        assert_eq!(addressed("Kevin is late", "Kev", false), None);
        assert_eq!(addressed("I told Kev to stop", "Kev", false), None);
        assert_eq!(addressed("follow me", "Kev", false), None);
        assert_eq!(addressed(" follow me ", "Kev", true), Some("follow me"));
    }

    #[test]
    fn names_take_plurals_and_typos() {
        let n = names();
        assert_eq!(n.item("oak logs").as_deref(), Some("oak_log"));
        assert_eq!(n.item("buches de chene").as_deref(), Some("oak_log"));
        assert_eq!(n.item("lingots de fer").as_deref(), Some("iron_ingot"));
        assert_eq!(n.item("eichenstamme").as_deref(), Some("oak_log"));
        assert_eq!(n.item("paes").as_deref(), Some("bread"));
        assert_eq!(n.item("crafting tabel").as_deref(), Some("crafting_table"));
        assert_eq!(n.item("oak log").as_deref(), Some("oak_log"));
        assert_eq!(n.item("wood").as_deref(), Some("#logs"));
        assert_eq!(n.item("holz").as_deref(), Some("#logs"));
        assert_eq!(n.item("unobtainium"), None);
        assert_eq!(n.mob("zombies").as_deref(), Some("zombie"));
        assert_eq!(
            normalize("Bûche de Chêne, l'Araignée — Größe!"),
            "buche de chene l araignee grosse"
        );
    }

    #[test]
    fn rules_are_instant_with_a_full_item_list() {
        let mut n = names();
        // About the size of the game's items and blocks in two languages.
        for i in 0..3000 {
            n.add_item(&format!("item_{i}"), &format!("Generated Thing Number {i}"));
        }
        let started = std::time::Instant::now();
        for _ in 0..50 {
            assert!(matches!(
                parse("can you please get me ten oak logs", "fr_fr", &n),
                Parse::Intent(_)
            ));
            parse("what do you think about the weather today", "en_us", &n);
        }
        let per_line = started.elapsed() / 100;
        assert!(
            per_line < std::time::Duration::from_millis(3),
            "{per_line:?} per line"
        );
    }

    struct Fake(Option<Classification>);

    impl IntentClassifier for Fake {
        fn classify(&self, _: &str, _: &str) -> Option<Classification> {
            self.0.clone()
        }
    }

    #[test]
    fn the_pipeline_trusts_rules_then_a_sure_classifier_then_the_brain() {
        let n = names();
        let sure = Classification {
            ranked: vec![(Intent::Follow, 0.6), (collect("#logs", None), 0.91)],
            needs_brain: false,
        };
        let pipeline = Pipeline::new(Fake(Some(sure.clone())));
        assert_eq!(
            pipeline.route("stop", "en_us", &n),
            Route::Reflex {
                intent: Intent::Stop,
                by: Stage::Rules
            }
        );
        assert_eq!(
            pipeline.route("we're out of wood", "en_us", &n),
            Route::Reflex {
                intent: collect("#logs", None),
                by: Stage::Classifier { confidence: 0.91 }
            }
        );
        let close = Classification {
            ranked: vec![(Intent::Follow, 0.86), (Intent::Come, 0.8)],
            needs_brain: false,
        };
        assert_eq!(
            Pipeline::new(Fake(Some(close))).route("over by me", "en_us", &n),
            Route::Brain,
            "too close to call"
        );
        let low = Classification {
            ranked: vec![(Intent::Follow, 0.5)],
            needs_brain: false,
        };
        assert_eq!(
            Pipeline::new(Fake(Some(low))).route("hmm", "en_us", &n),
            Route::Brain
        );
        let chat = Classification {
            needs_brain: true,
            ..sure
        };
        assert_eq!(
            Pipeline::new(Fake(Some(chat))).route("what a day", "en_us", &n),
            Route::Brain
        );
        assert_eq!(
            Pipeline::default().route("what a day", "en_us", &n),
            Route::Brain
        );
        assert_eq!(
            Pipeline::new(Fake(None)).route("jump", "en_us", &n),
            Route::Reflex {
                intent: Intent::Jump,
                by: Stage::Rules
            }
        );
    }
}
