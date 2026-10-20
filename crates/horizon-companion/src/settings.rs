//! The companion's settings, shared by the launcher and the game (docs/companions.md, "Fitting the
//! player's PC"): `no_ai`, the simple screen's state, Advanced's choices per part and the persona.
//! Stored as `<data>/companion/settings.json`; a cloud key lives in the OS keychain ([`keychain`]),
//! never in the file.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::client::{Endpoint, Provider};
use crate::plan::{Part, PartOption, Plan};
use crate::prompt::Persona;

/// Bumped when a field changes meaning; older files are read with defaults for what they lack.
pub const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompanionSettings {
    pub version: u32,
    /// Hides every AI feature, in the launcher and in the game. Also kept in the game's extras file
    /// ([`sync_extras`]).
    pub no_ai: bool,
    /// The player pressed "Set up companion": the game offers companions once the parts are on disk.
    pub set_up: bool,
    /// Push-to-talk and the spoken voice. Off by default: the microphone is opened only after this.
    pub voice: bool,
    pub persona: Persona,
    pub brain: BrainChoice,
    pub classifier: PartChoice,
    pub speech_to_text: PartChoice,
    pub text_to_speech: PartChoice,
    pub tuning: Tuning,
}

impl Default for CompanionSettings {
    fn default() -> CompanionSettings {
        CompanionSettings {
            version: VERSION,
            no_ai: false,
            set_up: false,
            voice: false,
            persona: Persona::default(),
            brain: BrainChoice::Auto,
            classifier: PartChoice::Auto,
            speech_to_text: PartChoice::Auto,
            text_to_speech: PartChoice::Auto,
            tuning: Tuning::default(),
        }
    }
}

/// Advanced's choice for a part.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "use", rename_all = "snake_case")]
pub enum PartChoice {
    /// Whatever the plan picks.
    #[default]
    Auto,
    Off,
    /// A catalogue option by its id (`qwen3.5-9b-q4_k_m@gpu`).
    Option {
        id: String,
    },
}

/// Advanced's choice for the brain: a catalogue option, the player's own GGUF, or a server.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "use", rename_all = "snake_case")]
pub enum BrainChoice {
    #[default]
    Auto,
    Option {
        id: String,
    },
    /// A GGUF the player picked, run by the bundled runtime.
    Gguf {
        path: PathBuf,
    },
    /// Ollama, LM Studio or a cloud API. A key, if any, is in the keychain under [`Self::key_account`].
    Server {
        provider: Provider,
        base_url: String,
        model: String,
    },
}

impl BrainChoice {
    /// The keychain account of a server's key.
    pub fn key_account(&self) -> Option<String> {
        match self {
            BrainChoice::Server { base_url, .. } => Some(base_url.trim_end_matches('/').to_owned()),
            _ => None,
        }
    }

    /// The endpoint of a server choice, its key read from the keychain.
    pub fn endpoint(&self) -> Option<Endpoint> {
        let BrainChoice::Server {
            provider,
            base_url,
            model,
        } = self
        else {
            return None;
        };
        let api_key = self
            .key_account()
            .and_then(|account| keychain::get(&account).ok().flatten());
        Some(Endpoint {
            provider: *provider,
            base_url: base_url.clone(),
            api_key,
            model: model.clone(),
        })
    }
}

/// Advanced's runtime knobs; `None` is the automatic value.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tuning {
    pub temperature: Option<f32>,
    /// Layers on the GPU: `Some(0)` keeps the brain on the processor.
    pub gpu_layers: Option<u32>,
    pub threads: Option<usize>,
    pub context: Option<u32>,
}

impl CompanionSettings {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("companion").join("settings.json")
    }

    /// The saved settings; defaults when there are none. A file that can't be read is kept beside as
    /// `settings.json.bad` so a broken edit isn't silently lost, and defaults are used.
    pub fn load(data_dir: &Path) -> CompanionSettings {
        let path = CompanionSettings::path(data_dir);
        let Ok(text) = fs::read_to_string(&path) else {
            return CompanionSettings::default();
        };
        match serde_json::from_str::<CompanionSettings>(&text) {
            Ok(mut settings) => {
                settings.version = VERSION;
                settings
            }
            Err(_) => {
                let _ = fs::rename(&path, path.with_extension("json.bad"));
                CompanionSettings::default()
            }
        }
    }

    /// Writes the file atomically (a temporary file renamed over it).
    pub fn save(&self, data_dir: &Path) -> io::Result<()> {
        let path = CompanionSettings::path(data_dir);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        fs::write(&tmp, text + "\n")?;
        fs::rename(&tmp, &path)
    }

    pub fn choice(&self, part: Part) -> PartChoice {
        match part {
            Part::Brain => match &self.brain {
                BrainChoice::Auto => PartChoice::Auto,
                BrainChoice::Option { id } => PartChoice::Option { id: id.clone() },
                // Not a catalogue option: the plan has nothing to set up for it.
                BrainChoice::Gguf { .. } | BrainChoice::Server { .. } => PartChoice::Off,
            },
            Part::Classifier => self.classifier.clone(),
            Part::SpeechToText if !self.voice => PartChoice::Off,
            Part::TextToSpeech if !self.voice => PartChoice::Off,
            Part::SpeechToText => self.speech_to_text.clone(),
            Part::TextToSpeech => self.text_to_speech.clone(),
        }
    }

    /// What gets set up for each part: the plan's pick unless Advanced says otherwise. An option id
    /// that is no longer in the catalogue falls back to the plan.
    pub fn selection<'a>(&self, plan: &'a Plan, options: &'a [PartOption]) -> Vec<&'a PartOption> {
        Part::ALL
            .into_iter()
            .filter_map(|part| match self.choice(part) {
                PartChoice::Off => None,
                PartChoice::Option { id } => options
                    .iter()
                    .find(|o| o.part == part && o.id == id)
                    .or_else(|| plan.pick(part).map(|p| &p.option)),
                PartChoice::Auto => plan.pick(part).map(|p| &p.option),
            })
            .collect()
    }
}

/// The game's extras file in its home, and the key No AI is kept under there.
pub const EXTRAS_FILE: &str = "horizon-extras.txt";
pub const EXTRAS_KEY: &str = "noAi";

/// Writes `noAi:true|false` into the game's `horizon-extras.txt`, keeping every other line, so the
/// game sees No AI even before it reads the companion's settings. The launcher calls it on every
/// change and before each start, since the game rewrites that file with the keys it knows. (A
/// `--no-ai` flag is not passed: today's game refuses options it doesn't know.)
pub fn sync_extras(game_home: &Path, no_ai: bool) -> io::Result<()> {
    let path = game_home.join(EXTRAS_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let line = format!("{EXTRAS_KEY}:{no_ai}");
    let mut lines: Vec<&str> = text
        .lines()
        .filter(|l| {
            l.split_once(':')
                .is_none_or(|(k, _)| k.trim() != EXTRAS_KEY)
        })
        .collect();
    if !no_ai && lines.is_empty() && !path.exists() {
        // Nothing to say yet: the game's default is off.
        return Ok(());
    }
    lines.push(&line);
    fs::create_dir_all(game_home)?;
    fs::write(&path, lines.join("\n") + "\n")
}

/// Secrets in the OS keychain: the macOS login keychain (`security`), the Secret Service on Linux
/// (`secret-tool`, from libsecret) and the Windows Credential Manager. Secrets go to the tools on
/// stdin, never on a command line other processes can read.
pub mod keychain {
    use std::io::{self, Write};
    use std::process::{Command, Stdio};

    pub const SERVICE: &str = "Horizon companion";

    /// Stores (or replaces) the secret for `account`.
    pub fn set(account: &str, secret: &str) -> io::Result<()> {
        imp::set(account, secret)
    }

    /// The secret for `account`, if there is one.
    pub fn get(account: &str) -> io::Result<Option<String>> {
        imp::get(account)
    }

    pub fn delete(account: &str) -> io::Result<()> {
        imp::delete(account)
    }

    #[allow(dead_code)]
    fn run(program: &str, args: &[&str], stdin: Option<&str>) -> io::Result<(bool, String)> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        if let Some(input) = stdin {
            child
                .stdin
                .take()
                .expect("piped")
                .write_all(input.as_bytes())?;
        } else {
            drop(child.stdin.take());
        }
        let out = child.wait_with_output()?;
        Ok((
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        ))
    }

    /// `security -i` reads its commands from stdin; arguments are double-quoted there.
    #[allow(dead_code)]
    pub(crate) fn security_quote(text: &str) -> String {
        format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
    }

    #[cfg(target_os = "macos")]
    mod imp {
        use super::{SERVICE, run, security_quote};
        use std::io;

        pub fn set(account: &str, secret: &str) -> io::Result<()> {
            let command = format!(
                "add-generic-password -U -s {} -a {} -w {}\n",
                security_quote(SERVICE),
                security_quote(account),
                security_quote(secret)
            );
            match run("security", &["-i"], Some(&command))? {
                (true, _) => Ok(()),
                _ => Err(io::Error::other("the keychain refused the key")),
            }
        }

        pub fn get(account: &str) -> io::Result<Option<String>> {
            let (ok, out) = run(
                "security",
                &["find-generic-password", "-s", SERVICE, "-a", account, "-w"],
                None,
            )?;
            Ok(ok.then(|| out.trim_end_matches('\n').to_owned()))
        }

        pub fn delete(account: &str) -> io::Result<()> {
            run(
                "security",
                &["delete-generic-password", "-s", SERVICE, "-a", account],
                None,
            )
            .map(|_| ())
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    mod imp {
        use super::{SERVICE, run};
        use std::io;

        pub fn set(account: &str, secret: &str) -> io::Result<()> {
            match run(
                "secret-tool",
                &[
                    "store", "--label", SERVICE, "service", SERVICE, "account", account,
                ],
                Some(secret),
            ) {
                Ok((true, _)) => Ok(()),
                Ok(_) => Err(io::Error::other("the keyring refused the key")),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Err(io::Error::other(
                    "no keyring: install libsecret (secret-tool) to save keys",
                )),
                Err(e) => Err(e),
            }
        }

        pub fn get(account: &str) -> io::Result<Option<String>> {
            let (ok, out) = run(
                "secret-tool",
                &["lookup", "service", SERVICE, "account", account],
                None,
            )?;
            Ok((ok && !out.is_empty()).then_some(out))
        }

        pub fn delete(account: &str) -> io::Result<()> {
            run(
                "secret-tool",
                &["clear", "service", SERVICE, "account", account],
                None,
            )
            .map(|_| ())
        }
    }

    #[cfg(windows)]
    mod imp {
        use super::SERVICE;
        use std::io;
        use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, GetLastError};
        use windows_sys::Win32::Security::Credentials::{
            CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
            CredReadW, CredWriteW,
        };

        fn target(account: &str) -> Vec<u16> {
            format!("{SERVICE}: {account}")
                .encode_utf16()
                .chain(Some(0))
                .collect()
        }

        pub fn set(account: &str, secret: &str) -> io::Result<()> {
            let mut name = target(account);
            let mut blob = secret.as_bytes().to_vec();
            // SAFETY: every pointer is to a live buffer for the duration of the call.
            let ok = unsafe {
                let mut credential: CREDENTIALW = std::mem::zeroed();
                credential.Type = CRED_TYPE_GENERIC;
                credential.TargetName = name.as_mut_ptr();
                credential.CredentialBlobSize = blob.len() as u32;
                credential.CredentialBlob = blob.as_mut_ptr();
                credential.Persist = CRED_PERSIST_LOCAL_MACHINE;
                CredWriteW(&credential, 0)
            };
            if ok == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        pub fn get(account: &str) -> io::Result<Option<String>> {
            let name = target(account);
            let mut found: *mut CREDENTIALW = std::ptr::null_mut();
            // SAFETY: CredReadW fills `found`, freed below with CredFree.
            unsafe {
                if CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut found) == 0 {
                    return if GetLastError() == ERROR_NOT_FOUND {
                        Ok(None)
                    } else {
                        Err(io::Error::last_os_error())
                    };
                }
                let credential = &*found;
                let bytes = std::slice::from_raw_parts(
                    credential.CredentialBlob,
                    credential.CredentialBlobSize as usize,
                );
                let secret = String::from_utf8_lossy(bytes).into_owned();
                CredFree(found as *const _);
                Ok(Some(secret))
            }
        }

        pub fn delete(account: &str) -> io::Result<()> {
            let name = target(account);
            // SAFETY: a valid, NUL-terminated name.
            unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) };
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::Hardware;
    use crate::plan::{self, brain_options};

    #[test]
    fn settings_round_trip_and_survive_a_broken_file() {
        let dir = crate::download::tests::scratch("settings");
        assert_eq!(CompanionSettings::load(&dir), CompanionSettings::default());
        let mut settings = CompanionSettings {
            no_ai: true,
            voice: true,
            brain: BrainChoice::Server {
                provider: Provider::Ollama,
                base_url: "http://127.0.0.1:11434/v1".into(),
                model: "qwen3.5:4b".into(),
            },
            text_to_speech: PartChoice::Option {
                id: "pocket-tts@cpu".into(),
            },
            ..CompanionSettings::default()
        };
        settings.persona.name = "Rosa".into();
        settings.tuning.temperature = Some(0.4);
        settings.save(&dir).unwrap();
        let text = fs::read_to_string(CompanionSettings::path(&dir)).unwrap();
        assert!(text.contains("\"use\": \"server\""), "{text}");
        assert!(!text.contains("key"), "no key in the file: {text}");
        assert_eq!(CompanionSettings::load(&dir), settings);

        // Fields from a newer or older version: unknown ones are ignored, missing ones default.
        fs::write(
            CompanionSettings::path(&dir),
            r#"{"no_ai": false, "set_up": true, "future": 3}"#,
        )
        .unwrap();
        let old = CompanionSettings::load(&dir);
        assert!(old.set_up && !old.voice);
        assert_eq!(old.brain, BrainChoice::Auto);

        fs::write(CompanionSettings::path(&dir), "{ not json").unwrap();
        assert_eq!(CompanionSettings::load(&dir), CompanionSettings::default());
        assert!(
            CompanionSettings::path(&dir)
                .with_extension("json.bad")
                .exists()
        );
    }

    #[test]
    fn no_ai_reaches_the_games_extras_file_and_keeps_its_lines() {
        let home = crate::download::tests::scratch("extras");
        sync_extras(&home, false).unwrap();
        assert!(
            !home.join(EXTRAS_FILE).exists(),
            "nothing written for the default"
        );
        fs::write(home.join(EXTRAS_FILE), "zoom:true\nfps:false\n").unwrap();
        sync_extras(&home, true).unwrap();
        assert_eq!(
            fs::read_to_string(home.join(EXTRAS_FILE)).unwrap(),
            "zoom:true\nfps:false\nnoAi:true\n"
        );
        sync_extras(&home, false).unwrap();
        assert_eq!(
            fs::read_to_string(home.join(EXTRAS_FILE)).unwrap(),
            "zoom:true\nfps:false\nnoAi:false\n"
        );
    }

    #[test]
    fn the_selection_is_the_plan_unless_advanced_overrides_it() {
        let hw = Hardware {
            ram_mb: 16 * 1024,
            physical_cores: 6,
            logical_cores: 12,
            apple_silicon: false,
            gpus: Vec::new(),
        };
        let options = brain_options();
        let plan = plan::plan(&hw, &options, &[], None);
        let mut settings = CompanionSettings::default();
        let ids = |s: &CompanionSettings| -> Vec<String> {
            s.selection(&plan, &options)
                .iter()
                .map(|o| o.id.clone())
                .collect()
        };
        assert_eq!(ids(&settings), ["qwen3.5-4b-q4_k_m@cpu"]);
        settings.brain = BrainChoice::Option {
            id: "qwen3.5-2b-q4_k_m@cpu".into(),
        };
        assert_eq!(ids(&settings), ["qwen3.5-2b-q4_k_m@cpu"]);
        settings.brain = BrainChoice::Option {
            id: "a-model-that-was-removed".into(),
        };
        assert_eq!(ids(&settings), ["qwen3.5-4b-q4_k_m@cpu"]);
        settings.brain = BrainChoice::Gguf {
            path: "/models/mine.gguf".into(),
        };
        assert!(ids(&settings).is_empty(), "nothing to download");
        assert_eq!(settings.choice(Part::TextToSpeech), PartChoice::Off);
    }

    #[test]
    fn keychain_commands_quote_what_they_pass() {
        assert_eq!(keychain::security_quote(r#"a"b\c"#), r#""a\"b\\c""#);
        let server = BrainChoice::Server {
            provider: Provider::Cloud,
            base_url: "https://api.example.com/v1/".into(),
            model: "m".into(),
        };
        assert_eq!(
            server.key_account().as_deref(),
            Some("https://api.example.com/v1")
        );
    }

    /// Writes to the real keychain: run by hand (`--ignored`) on a desktop session.
    #[test]
    #[ignore = "touches the OS keychain"]
    fn keychain_round_trip() {
        let account = format!("test-{}", std::process::id());
        keychain::set(&account, "s3cret \"quoted\"").unwrap();
        assert_eq!(
            keychain::get(&account).unwrap().as_deref(),
            Some("s3cret \"quoted\"")
        );
        keychain::delete(&account).unwrap();
        assert_eq!(keychain::get(&account).unwrap(), None);
    }
}
