//! Horizon shader packs (docs/shader-packs.md): folders in `shaderpacks/`
//! holding a `pack.toml` and WGSL stages. Loading reads the manifest and the
//! player's option values, puts the prelude before each stage's source and
//! has naga parse and validate it, so a broken pack is reported (file, line,
//! column) and never reaches the GPU.
//!
//! Api 2 ("Horizon shaders v1") adds `deferred` stages, the pack's own render
//! targets (formats, scale, previous frame) and textures, a shadow map and
//! world hooks (`[gbuffer]`): WGSL functions Horizon's terrain, entity and sky
//! shaders call, which also write a G-buffer the stages read.
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// The stage and uniform contract this build provides.
pub const API: u32 = 2;
/// The folder beside `options.txt` that holds the packs.
pub const DIR: &str = "shaderpacks";
/// The `Frame` uniform and helpers every pack shader gets.
pub const COMMON: &str = include_str!("shader_pack_common.wgsl");
/// The full-screen stages' bindings and helpers (after `COMMON`).
pub const PRELUDE: &str = include_str!("shader_pack_prelude.wgsl");
/// The world hooks' bindings and types (after Horizon's world shader and `COMMON`).
const WORLD: &str = include_str!("shader_pack_world.wgsl");
const TERRAIN_ENTRIES: &str = include_str!("shader_pack_terrain.wgsl");
const ENTITY_ENTRIES: &str = include_str!("shader_pack_entity.wgsl");
/// The prelude's vertex entry point, shared by every stage.
pub const VERTEX_ENTRY: &str = "horizon_stage_vertex";
const MAX_STAGES: usize = 16;
const MAX_OPTIONS: usize = 64;
const MAX_TARGETS: usize = 8;
const MAX_TEXTURES: usize = 8;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    pack: Info,
    #[serde(default)]
    stage: Vec<StageEntry>,
    #[serde(default)]
    options: BTreeMap<String, OptionEntry>,
    #[serde(default)]
    target: Vec<TargetEntry>,
    #[serde(default)]
    texture: Vec<TextureEntry>,
    shadow: Option<ShadowEntry>,
    gbuffer: Option<GbufferEntry>,
    /// Material id (16..=255) to sprite name patterns (`*` matches anything).
    #[serde(default)]
    materials: BTreeMap<String, Vec<String>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetEntry {
    name: String,
    format: Option<String>,
    scale: Option<f32>,
    #[serde(default)]
    history: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextureEntry {
    name: String,
    path: String,
    filter: Option<String>,
    wrap: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShadowEntry {
    resolution: Option<u32>,
    distance: Option<f32>,
    distortion: Option<f32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GbufferEntry {
    shader: Option<String>,
    sun_moon: Option<bool>,
    stars: Option<bool>,
    clouds: Option<bool>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Info {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub license: String,
    pub api: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StageEntry {
    kind: String,
    shader: String,
    entry: Option<String>,
    scale: Option<f32>,
    target: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OptionEntry {
    label: Option<String>,
    default: toml::Value,
    min: Option<f64>,
    max: Option<f64>,
    step: Option<f64>,
    values: Option<Vec<i64>>,
    /// The specialization constant id compiled (SPIR-V) stages read it by.
    id: Option<u32>,
}

/// Where a stage runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageKind {
    /// After the world pass, before the hand: reads the scene's colour and depth.
    Composite,
    /// After opaque terrain and entities, before translucent terrain, water
    /// and clouds: its `color` output becomes the world's colour.
    Deferred,
}
#[derive(Clone, Debug)]
pub struct Stage {
    pub kind: StageKind,
    /// The stage's file, relative to the pack.
    pub file: String,
    /// The prelude followed by the stage's source.
    pub source: String,
    pub entry: String,
    /// Target size relative to the world's (`color` stages; a named
    /// target has its own).
    pub scale: f32,
    /// The pack target it writes (index into `ShaderPack::targets`), or the
    /// `color` chain.
    pub target: Option<usize>,
    /// The vertex shader's module when `source` has none (a compiled stage).
    pub vertex: Option<String>,
    /// The pipeline constants: the option values its module declares (by
    /// name, or by id for a compiled stage).
    pub constants: Vec<(String, f64)>,
}
/// A render target of the pack's own, read by every stage that doesn't
/// write it (`<name>`; `<name>_history` holds the last frame's).
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub name: String,
    pub format: TargetFormat,
    pub scale: f32,
    pub history: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetFormat {
    Rgba8,
    Rgba16Float,
    Rgba32Float,
    R16Float,
    Rg16Float,
    R32Float,
}
impl TargetFormat {
    const NAMES: [(&str, Self); 6] = [
        ("rgba8unorm", Self::Rgba8),
        ("rgba16float", Self::Rgba16Float),
        ("rgba32float", Self::Rgba32Float),
        ("r16float", Self::R16Float),
        ("rg16float", Self::Rg16Float),
        ("r32float", Self::R32Float),
    ];
    pub fn wgpu(self) -> wgpu::TextureFormat {
        match self {
            Self::Rgba8 => wgpu::TextureFormat::Rgba8Unorm,
            Self::Rgba16Float => wgpu::TextureFormat::Rgba16Float,
            Self::Rgba32Float => wgpu::TextureFormat::Rgba32Float,
            Self::R16Float => wgpu::TextureFormat::R16Float,
            Self::Rg16Float => wgpu::TextureFormat::Rg16Float,
            Self::R32Float => wgpu::TextureFormat::R32Float,
        }
    }
    /// 32-bit float targets can't be filtered everywhere: read them with
    /// `nearest_sampler` or `textureLoad`.
    pub fn filterable(self) -> bool {
        !matches!(self, Self::Rgba32Float | Self::R32Float)
    }
}
/// A texture of the pack's (`textures/*.png`), RGBA8, not sRGB-decoded.
#[derive(Clone, Debug)]
pub struct Texture {
    pub name: String,
    pub size: [u32; 2],
    pub pixels: Vec<u8>,
    pub linear: bool,
    pub repeat: bool,
}
/// The `[shadow]` map: terrain (and entities) seen from the sun or moon.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub resolution: u32,
    /// Blocks from the eye the map covers, on each side.
    pub distance: f32,
    /// 0: a plain orthographic map; towards 1: more texels near the eye.
    pub distortion: f32,
}
/// The `[gbuffer]` hooks, composed into Horizon's world shaders.
#[derive(Clone, Debug)]
pub struct Gbuffer {
    /// The hooks' file ("" when the pack only wants the G-buffer).
    pub file: String,
    /// terrain.wgsl with the hooks and the pack entry points.
    pub terrain: String,
    /// entity.wgsl (after the overlay constants) with the hooks.
    pub entity: String,
    /// The pack defines `sky_color`: the sky is drawn through it.
    pub sky: bool,
    pub sun_moon: bool,
    pub stars: bool,
    pub clouds: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OptionKind {
    Bool,
    /// An `i32`, cycling through `values`.
    Int,
    /// An `f32` slider.
    Float {
        min: f64,
        max: f64,
        step: f64,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct PackOption {
    /// The WGSL `override` it sets.
    pub key: String,
    pub label: String,
    pub kind: OptionKind,
    pub values: Vec<i64>,
    pub default: f64,
    pub value: f64,
    /// Its specialization constant id, for compiled (SPIR-V) stages.
    pub id: Option<u32>,
}

pub struct ShaderPack {
    /// The pack's folder name.
    pub id: String,
    pub info: Info,
    pub stages: Vec<Stage>,
    pub options: Vec<PackOption>,
    pub targets: Vec<Target>,
    pub textures: Vec<Texture>,
    pub shadow: Option<Shadow>,
    pub gbuffer: Option<Gbuffer>,
    /// The pack's own materials: id and sprite name patterns.
    pub materials: Vec<(u8, Vec<String>)>,
    /// A Java (OptiFine/Iris) pack's translated passes, instead of `stages`.
    pub java: Option<crate::java_pack::JavaPasses>,
    dir: PathBuf,
    stamp: Stamp,
}

/// Every file's path, length and modification time: a changed stamp means
/// the pack was edited (hot reload).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stamp(Vec<(PathBuf, u64, Option<SystemTime>)>);
impl Stamp {
    pub fn of(dir: &Path) -> Self {
        let mut files = Vec::new();
        if let Ok(meta) = fs::metadata(dir)
            && meta.is_file()
        {
            files.push((dir.to_path_buf(), meta.len(), meta.modified().ok()));
        }
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = fs::read_dir(&d) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(meta) = entry.metadata() else { continue };
                if meta.is_dir() {
                    // A Rust pack's build folder: thousands of files the game
                    // never reads (it reads the .spv copied into shaders/).
                    if entry.file_name() != "target" {
                        stack.push(path);
                    }
                } else {
                    files.push((path, meta.len(), meta.modified().ok()));
                }
            }
        }
        files.sort();
        Self(files)
    }
}

/// The packs in `dir` (`shaderpacks/`): folders holding a `pack.toml`, by
/// folder name, with their manifest or why it can't be read, and Java packs
/// (folders holding `shaders/`, zips).
pub fn list(dir: &Path) -> Vec<(String, Result<Info, String>)> {
    let mut packs: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            if path.join("pack.toml").is_file() {
                Some((id, manifest(&path).map(|m| m.pack)))
            } else if crate::java_pack::is_java_pack(&path) {
                Some((id.clone(), Ok(java_info(id))))
            } else {
                None
            }
        })
        .collect();
    packs.sort_by(|a, b| a.0.cmp(&b.0));
    packs
}

/// `shaderpacks/` beside `options.txt` (the game's working folder: the game
/// home, not the launcher's version folder).
pub fn folder() -> PathBuf {
    std::env::current_dir().unwrap_or_default().join(DIR)
}

/// `shaderpacks/` and its packs as the Shader Packs page lists them, read
/// again at most once a second while the page is open, so a pack dropped
/// into the folder shows up.
#[derive(Debug, Default)]
pub struct Folder {
    pub dir: PathBuf,
    pub packs: Vec<(String, Result<Info, String>)>,
    read: Option<std::time::Instant>,
}

impl Folder {
    pub fn poll(&mut self, dir: PathBuf) {
        let fresh = self
            .read
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(1));
        if fresh && self.dir == dir {
            return;
        }
        self.packs = list(&dir);
        self.dir = dir;
        self.read = Some(std::time::Instant::now());
    }
}

/// A Java pack has no manifest: its name is its file name.
fn java_info(name: String) -> Info {
    Info {
        name,
        description: "Iris/OptiFine pack: its full-screen passes run.".into(),
        authors: Vec::new(),
        version: String::new(),
        license: String::new(),
        api: API,
    }
}

fn manifest(dir: &Path) -> Result<Manifest, String> {
    let text = fs::read_to_string(dir.join("pack.toml")).map_err(|e| format!("pack.toml: {e}"))?;
    let manifest: Manifest = toml::from_str(&text).map_err(|e| format!("pack.toml: {e}"))?;
    if manifest.pack.api > API {
        return Err(format!(
            "pack.toml: needs shader pack api {}, this Horizon has {API}",
            manifest.pack.api
        ));
    }
    Ok(manifest)
}

impl ShaderPack {
    /// Load and validate the pack in `dir` with the option values in
    /// `values` (missing ones take their default).
    pub fn load(dir: &Path, values: &BTreeMap<String, String>) -> Result<Self, String> {
        let stamp = Stamp::of(dir);
        let manifest = manifest(dir)?;
        if manifest.stage.len() > MAX_STAGES
            || (manifest.stage.is_empty() && manifest.gbuffer.is_none())
        {
            return Err(format!("pack.toml: 1 to {MAX_STAGES} stages"));
        }
        if manifest.options.len() > MAX_OPTIONS {
            return Err(format!("pack.toml: at most {MAX_OPTIONS} options"));
        }
        if manifest.target.len() > MAX_TARGETS || manifest.texture.len() > MAX_TEXTURES {
            return Err(format!(
                "pack.toml: at most {MAX_TARGETS} targets and {MAX_TEXTURES} textures"
            ));
        }
        let api = manifest.pack.api;
        let since = |what: &str, version: u32| -> Result<(), String> {
            if api < version {
                return Err(format!("pack.toml: {what} needs `api = {version}`"));
            }
            Ok(())
        };
        let options = manifest
            .options
            .into_iter()
            .map(|(key, entry)| option(key, entry, values))
            .collect::<Result<Vec<_>, _>>()?;
        let mut names = std::collections::BTreeSet::new();
        let mut name = |name: &str| -> Result<(), String> {
            let ident = name.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            if !ident || name == "color" || !names.insert(name.to_string()) {
                return Err(format!(
                    "pack.toml: `{name}`: names are lowercase identifiers, used once, not `color`"
                ));
            }
            Ok(())
        };
        let mut targets = Vec::new();
        for entry in manifest.target {
            since("[[target]]", 2)?;
            name(&entry.name)?;
            let format = match entry.format.as_deref() {
                None => TargetFormat::Rgba16Float,
                Some(f) => TargetFormat::NAMES
                    .iter()
                    .find(|(n, _)| *n == f)
                    .map(|(_, t)| *t)
                    .ok_or_else(|| {
                        let known: Vec<_> = TargetFormat::NAMES.iter().map(|(n, _)| *n).collect();
                        format!(
                            "pack.toml: target `{}`: format `{f}` isn't one of {}",
                            entry.name,
                            known.join(", ")
                        )
                    })?,
            };
            let scale = entry.scale.unwrap_or(1.0);
            if !(0.125..=2.0).contains(&scale) {
                return Err(format!(
                    "pack.toml: target `{}`: scale must be 0.125 to 2",
                    entry.name
                ));
            }
            if entry.history {
                name(&format!("{}_history", entry.name))?;
            }
            targets.push(Target {
                name: entry.name,
                format,
                scale,
                history: entry.history,
            });
        }
        let mut textures = Vec::new();
        for entry in manifest.texture {
            since("[[texture]]", 2)?;
            name(&entry.name)?;
            let path = inside(dir, &entry.path)?;
            let (width, height, pixels) =
                crate::hud::decode(&path).map_err(|e| format!("{}: {e}", entry.path))?;
            if width > 4096 || height > 4096 {
                return Err(format!("{}: at most 4096 x 4096", entry.path));
            }
            let choice = |value: Option<&str>, a: &str, b: &str| match value {
                None => Ok(true),
                Some(v) if v == a => Ok(true),
                Some(v) if v == b => Ok(false),
                Some(v) => Err(format!(
                    "pack.toml: texture `{}`: `{v}` isn't `{a}` or `{b}`",
                    entry.name
                )),
            };
            textures.push(Texture {
                linear: choice(entry.filter.as_deref(), "linear", "nearest")?,
                repeat: choice(entry.wrap.as_deref(), "repeat", "clamp")?,
                name: entry.name,
                size: [width, height],
                pixels,
            });
        }
        let shadow = match manifest.shadow {
            None => None,
            Some(s) => {
                since("[shadow]", 2)?;
                let resolution = s.resolution.unwrap_or(2048);
                let distance = s.distance.unwrap_or(128.0);
                let distortion = s.distortion.unwrap_or(0.85);
                if !resolution.is_power_of_two() || !(512..=8192).contains(&resolution) {
                    return Err("pack.toml: shadow resolution: a power of two, 512 to 8192".into());
                }
                if !(16.0..=512.0).contains(&distance) || !(0.0..=0.95).contains(&distortion) {
                    return Err(
                        "pack.toml: shadow distance 16 to 512 blocks, distortion 0 to 0.95".into(),
                    );
                }
                Some(Shadow {
                    resolution,
                    distance,
                    distortion,
                })
            }
        };
        let gbuffer = match manifest.gbuffer {
            None => None,
            Some(g) => {
                since("[gbuffer]", 2)?;
                let (file, code) = match &g.shader {
                    Some(file) => {
                        let path = inside(dir, file)?;
                        let code = fs::read_to_string(&path).map_err(|e| format!("{file}: {e}"))?;
                        (file.clone(), code)
                    }
                    None => (String::new(), String::new()),
                };
                let hooks = hooks(&code);
                let world = |shader: &str, entries: &str, entry: &str| -> Result<String, String> {
                    let before = format!("{shader}\n{COMMON}\n{WORLD}\n");
                    let source = format!("{before}{hooks}\n{entries}");
                    let shown = if file.is_empty() { "[gbuffer]" } else { &file };
                    validate(&source, before.lines().count() as u32, shown, entry)?;
                    Ok(source)
                };
                let terrain = world(
                    include_str!("terrain.wgsl"),
                    TERRAIN_ENTRIES,
                    "fs_solid_pack",
                )?;
                let entity = world(
                    &(crate::overlay::wgsl_constants() + include_str!("entity.wgsl")),
                    ENTITY_ENTRIES,
                    "fs_entity_pack",
                )?;
                Some(Gbuffer {
                    sky: defines(&code, "sky_color"),
                    file,
                    terrain,
                    entity,
                    sun_moon: g.sun_moon.unwrap_or(true),
                    stars: g.stars.unwrap_or(true),
                    clouds: g.clouds.unwrap_or(true),
                })
            }
        };
        let mut materials = Vec::new();
        for (id, patterns) in manifest.materials {
            since("[materials]", 2)?;
            match id.parse::<u8>() {
                Ok(id) if id >= 16 => materials.push((id, patterns)),
                _ => return Err(format!("pack.toml: material `{id}`: an id from 16 to 255")),
            }
        }
        let resources = declarations(&targets, &textures);
        let mut stages = Vec::new();
        for entry in manifest.stage {
            let kind = match entry.kind.as_str() {
                "composite" => StageKind::Composite,
                "deferred" => {
                    since("a `deferred` stage", 2)?;
                    StageKind::Deferred
                }
                other => {
                    return Err(format!(
                        "pack.toml: stage kind `{other}` isn't supported (api {API} has `composite` and `deferred`)"
                    ));
                }
            };
            let target = match entry.target.as_deref() {
                None | Some("color") => None,
                Some(t) => Some(
                    targets
                        .iter()
                        .position(|x| x.name == t)
                        .ok_or_else(|| format!("pack.toml: {}: no target `{t}`", entry.shader))?,
                ),
            };
            let scale = entry.scale.unwrap_or(1.0);
            if !(0.25..=2.0).contains(&scale) {
                return Err(format!(
                    "pack.toml: {}: scale must be 0.25 to 2",
                    entry.shader
                ));
            }
            if kind == StageKind::Deferred && gbuffer.is_none() {
                return Err(format!(
                    "pack.toml: {}: a deferred stage needs `[gbuffer]`",
                    entry.shader
                ));
            }
            if kind == StageKind::Deferred && target.is_none() && scale != 1.0 {
                return Err(format!(
                    "pack.toml: {}: a deferred stage writing `color` has scale 1",
                    entry.shader
                ));
            }
            let path = inside(dir, &entry.shader)?;
            let entry_point = entry.entry.unwrap_or_else(|| "fs_main".into());
            let before = format!("{COMMON}\n{PRELUDE}\n{resources}");
            let (source, vertex, module) = if entry.shader.ends_with(".spv") {
                // Compiled (Rust, rust-gpu): the module brings its own
                // bindings; the prelude only gives the vertex shader.
                since("a compiled (.spv) stage", 2)?;
                let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", entry.shader))?;
                let (source, module) = spirv_stage(&bytes, &entry.shader, &entry_point)?;
                (source, Some(before), module)
            } else {
                let code =
                    fs::read_to_string(&path).map_err(|e| format!("{}: {e}", entry.shader))?;
                let source = format!("{before}{code}");
                let module = validate(
                    &source,
                    before.lines().count() as u32,
                    &entry.shader,
                    &entry_point,
                )?;
                (source, None, module)
            };
            let constants = stage_constants(&module, &options);
            stages.push(Stage {
                kind,
                file: entry.shader,
                source,
                entry: entry_point,
                scale,
                target,
                vertex,
                constants,
            });
        }
        Ok(Self {
            id: dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            info: manifest.pack,
            stages,
            options,
            targets,
            textures,
            shadow,
            gbuffer,
            materials,
            java: None,
            dir: dir.to_path_buf(),
            stamp,
        })
    }
    /// Load the pack at `path` (a folder or a zip): a Horizon pack, or a
    /// Java pack translated for the GPU `vendor` and `renderer` names.
    pub fn load_any(
        path: &Path,
        values: &BTreeMap<String, String>,
        vendor: &str,
        renderer: &str,
    ) -> Result<Self, String> {
        if path.join("pack.toml").is_file() || !crate::java_pack::is_java_pack(path) {
            return Self::load(path, values);
        }
        let stamp = Stamp::of(path);
        let java = crate::java_pack::load(path, values, vendor, renderer)?;
        let id = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Self {
            info: java_info(id.clone()),
            id,
            stages: Vec::new(),
            options: Vec::new(),
            targets: Vec::new(),
            textures: Vec::new(),
            shadow: None,
            gbuffer: None,
            materials: Vec::new(),
            java: Some(java),
            dir: path.to_path_buf(),
            stamp,
        })
    }
    /// The pipeline-overridable constants every stage gets: the option values.
    pub fn constants(&self) -> Vec<(&str, f64)> {
        self.options
            .iter()
            .map(|o| (o.key.as_str(), o.value))
            .collect()
    }
    /// The pack's files changed since it loaded.
    pub fn changed(&self) -> bool {
        Stamp::of(&self.dir) != self.stamp
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

fn option(
    key: String,
    entry: OptionEntry,
    values: &BTreeMap<String, String>,
) -> Result<PackOption, String> {
    let bad = |what: &str| format!("pack.toml: option `{key}`: {what}");
    let (kind, default) = match entry.default {
        toml::Value::Boolean(b) => (OptionKind::Bool, f64::from(u8::from(b))),
        toml::Value::Integer(i) => (OptionKind::Int, i as f64),
        toml::Value::Float(f) => (
            OptionKind::Float {
                min: entry.min.ok_or_else(|| bad("a number needs min"))?,
                max: entry.max.ok_or_else(|| bad("a number needs max"))?,
                step: entry.step.unwrap_or(0.0),
            },
            f,
        ),
        _ => return Err(bad("default must be true/false, an integer or a number")),
    };
    let values_list = entry.values.unwrap_or_default();
    if kind == OptionKind::Int && !values_list.contains(&(default as i64)) {
        return Err(bad("an integer needs `values` holding its default"));
    }
    let value = values
        .get(&key)
        .and_then(|v| match kind {
            OptionKind::Bool => v.parse::<bool>().ok().map(|b| f64::from(u8::from(b))),
            OptionKind::Int => v
                .parse::<i64>()
                .ok()
                .filter(|i| values_list.contains(i))
                .map(|i| i as f64),
            OptionKind::Float { min, max, .. } => v
                .parse::<f64>()
                .ok()
                .filter(|f| f.is_finite())
                .map(|f| f.clamp(min, max)),
        })
        .unwrap_or(default);
    Ok(PackOption {
        label: entry.label.unwrap_or_else(|| key.clone()),
        id: entry.id,
        key,
        kind,
        values: values_list,
        default,
        value,
    })
}

/// naga parses and validates the stage (the `prelude_lines` before the
/// pack's code included) and checks its fragment entry point; errors point
/// into the pack's own file.
fn validate(
    source: &str,
    prelude_lines: u32,
    file: &str,
    entry: &str,
) -> Result<naga::Module, String> {
    let at = |location: Option<naga::SourceLocation>| match location {
        Some(l) if l.line_number > prelude_lines => {
            format!(
                "{file}:{}:{}",
                l.line_number - prelude_lines,
                l.line_position
            )
        }
        _ => file.to_string(),
    };
    let module = naga::front::wgsl::parse_str(source)
        .map_err(|e| format!("{}: {}", at(e.location(source)), e.message()))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map_err(|e| format!("{}: {}", at(e.location(source)), e.as_inner()))?;
    if !module
        .entry_points
        .iter()
        .any(|e| e.name == entry && e.stage == naga::ShaderStage::Fragment)
    {
        return Err(format!("{file}: no fragment entry point `{entry}`"));
    }
    Ok(module)
}

/// A compiled stage (SPIR-V, from rust-gpu): parsed and validated by naga,
/// with its fragment entry point, and written back as WGSL for wgpu.
fn spirv_stage(bytes: &[u8], file: &str, entry: &str) -> Result<(String, naga::Module), String> {
    let module = naga::front::spv::parse_u8_slice(bytes, &Default::default())
        .map_err(|e| format!("{file}: {e}"))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map_err(|e| format!("{file}: {}", e.as_inner()))?;
    if !module
        .entry_points
        .iter()
        .any(|e| e.name == entry && e.stage == naga::ShaderStage::Fragment)
    {
        return Err(format!("{file}: no fragment entry point `{entry}`"));
    }
    let wgsl =
        naga::back::wgsl::write_string(&module, &info, naga::back::wgsl::WriterFlags::empty())
            .map_err(|e| format!("{file}: {e}"))?;
    Ok((wgsl, module))
}

/// The option values a stage's module declares an override for: by name,
/// or by specialization constant id (compiled stages, which get every value
/// as the bits of an `f32`).
fn stage_constants(module: &naga::Module, options: &[PackOption]) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    for (_, o) in module.overrides.iter() {
        let by_name = o
            .name
            .as_deref()
            .and_then(|n| options.iter().find(|p| p.key == n));
        if let Some(p) = by_name {
            out.push((p.key.clone(), p.value));
        } else if let Some(id) = o.id
            && let Some(p) = options.iter().find(|p| p.id == Some(u32::from(id)))
        {
            out.push((id.to_string(), f64::from((p.value as f32).to_bits())));
        }
    }
    out
}

/// `file` inside the pack folder `dir`.
fn inside(dir: &Path, file: &str) -> Result<PathBuf, String> {
    let path = dir.join(file);
    if !path.starts_with(dir) || file.contains("..") || Path::new(file).is_absolute() {
        return Err(format!("pack.toml: {file} is outside the pack"));
    }
    Ok(path)
}

/// `code` defines the function `name`.
fn defines(code: &str, name: &str) -> bool {
    code.match_indices("fn").any(|(i, _)| {
        let before = code[..i].chars().next_back();
        let rest = code[i + 2..].trim_start();
        before.is_none_or(|c| !c.is_alphanumeric() && c != '_')
            && code[i + 2..].starts_with(char::is_whitespace)
            && rest
                .strip_prefix(name)
                .is_some_and(|r| r.trim_start().starts_with('('))
    })
}

/// The pack's world hooks, with Horizon's defaults (vanilla's look) for the
/// ones it leaves out.
fn hooks(code: &str) -> String {
    let mut out = code.to_string();
    for (name, default) in [
        (
            "surface_vertex",
            "fn surface_vertex(v: SurfaceVertex) -> vec3<f32> { return v.world; }",
        ),
        (
            "surface_color",
            "fn surface_color(s: Surface) -> vec4<f32> { return s.vanilla; }",
        ),
        (
            "surface_normal",
            "fn surface_normal(s: Surface) -> vec3<f32> { return s.normal; }",
        ),
        (
            "sky_color",
            "fn sky_color(p: SkyPixel) -> vec4<f32> { return p.vanilla; }",
        ),
    ] {
        if !defines(code, name) {
            out.push('\n');
            out.push_str(default);
        }
    }
    out.push('\n');
    out
}

/// terrain.wgsl with the default hooks and the pack entry points: the shadow
/// pass of a pack without `[gbuffer]`.
pub fn shadow_only_terrain() -> String {
    format!(
        "{}\n{COMMON}\n{WORLD}\n{}\n{TERRAIN_ENTRIES}",
        include_str!("terrain.wgsl"),
        hooks("")
    )
}

/// What a full-screen stage reads in group 1: every pack target (and its
/// history), then every texture, in that order.
pub fn resources(targets: &[Target], textures: &[Texture]) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for t in targets {
        out.push((t.name.clone(), t.format.filterable()));
        if t.history {
            out.push((format!("{}_history", t.name), t.format.filterable()));
        }
    }
    out.extend(textures.iter().map(|t| (t.name.clone(), true)));
    out
}
fn declarations(targets: &[Target], textures: &[Texture]) -> String {
    resources(targets, textures)
        .iter()
        .enumerate()
        .map(|(i, (name, _))| format!("@group(1) @binding({i}) var {name}: texture_2d<f32>;\n"))
        .collect()
}

/// A sprite's material (MATERIAL_* in shader_pack_common.wgsl) by name: the
/// pack's patterns first, then Horizon's own sorting.
pub fn material(name: &str, pack: &[(u8, Vec<String>)]) -> u8 {
    let path = name.strip_prefix("minecraft:").unwrap_or(name);
    if let Some((id, _)) = pack
        .iter()
        .find(|(_, patterns)| patterns.iter().any(|p| glob(p, path)))
    {
        return *id;
    }
    let Some(n) = path.strip_prefix("block/") else {
        return 1;
    };
    const PLANTS: &[&str] = &[
        "short_grass",
        "tall_grass_*",
        "fern",
        "large_fern_*",
        "dead_bush",
        "bush",
        "firefly_bush",
        "short_dry_grass",
        "tall_dry_grass",
        "*_sapling",
        "dandelion",
        "poppy",
        "blue_orchid",
        "allium",
        "azure_bluet",
        "*_tulip",
        "oxeye_daisy",
        "cornflower",
        "lily_of_the_valley",
        "wither_rose",
        "torchflower",
        "*_eyeblossom",
        "sunflower_*",
        "lilac_*",
        "rose_bush_*",
        "peony_*",
        "pitcher_plant_*",
        "wheat_stage*",
        "carrots_stage*",
        "potatoes_stage*",
        "beetroots_stage*",
        "sugar_cane",
        "sweet_berry_bush_stage*",
        "seagrass",
        "tall_seagrass_*",
        "kelp",
        "kelp_plant",
        "vine",
        "pink_petals",
        "wildflowers",
        "crimson_roots",
        "warped_roots",
        "nether_sprouts",
        "hanging_roots",
    ];
    const EMISSIVE: &[&str] = &[
        "glowstone",
        "sea_lantern",
        "shroomlight",
        "jack_o_lantern",
        "redstone_lamp_on",
        "torch",
        "soul_torch",
        "redstone_torch",
        "lantern",
        "soul_lantern",
        "end_rod",
        "beacon",
        "magma",
        "*campfire_fire",
        "fire_*",
        "soul_fire_*",
        "*_froglight_*",
        "glow_lichen",
        "crying_obsidian",
        "sea_pickle",
        "cave_vines*_lit",
        "copper_bulb_lit*",
        "*_copper_bulb_lit*",
        "respawn_anchor_top",
        "conduit",
    ];
    const GLASS: &[&str] = &[
        "glass",
        "*_stained_glass",
        "*glass_pane_top",
        "tinted_glass",
        "ice",
        "slime_block",
        "honey_block_*",
        "nether_portal",
    ];
    if matches!(n, "water_still" | "water_flow" | "water_overlay") {
        4
    } else if matches!(n, "lava_still" | "lava_flow") {
        5
    } else if n.ends_with("_leaves") {
        2
    } else if PLANTS.iter().any(|p| glob(p, n)) {
        3
    } else if GLASS.iter().any(|p| glob(p, n)) {
        6
    } else if EMISSIVE.iter().any(|p| glob(p, n)) {
        7
    } else {
        1
    }
}
/// `*` matches any run of characters.
fn glob(pattern: &str, text: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == text,
        Some((head, tail)) => {
            text.starts_with(head)
                && (0..=text.len() - head.len()).any(|i| {
                    text.is_char_boundary(head.len() + i) && glob(tail, &text[head.len() + i..])
                })
        }
    }
}
/// The material of every sprite layer, four to a word, as the world hooks'
/// `materials` buffer holds them.
pub fn material_table(layers: &BTreeMap<String, u32>, pack: &[(u8, Vec<String>)]) -> Vec<u32> {
    let count = layers.values().max().map_or(0, |&m| m as usize + 1);
    let mut words = vec![0x0101_0101u32; count.div_ceil(4).max(1)];
    for (name, &layer) in layers {
        let shift = (layer % 4) * 8;
        let word = &mut words[layer as usize / 4];
        *word = (*word & !(0xff << shift)) | (u32::from(material(name, pack)) << shift);
    }
    words
}

/// The option values saved for a pack (`shaderpacks/<id>.txt`, `key=value`).
pub fn load_values(dir: &Path, id: &str) -> BTreeMap<String, String> {
    fs::read_to_string(dir.join(format!("{id}.txt")))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("horizon-shader-pack-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn pack(files: &[(&str, &str)]) -> PathBuf {
        let dir = scratch();
        for (name, text) in files {
            let path = dir.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        dir
    }
    const MANIFEST: &str = r#"
[pack]
name = "Test"
api = 1

[[stage]]
kind = "composite"
shader = "shaders/grade.wgsl"

[options.strength]
label = "Strength"
default = 0.5
min = 0.0
max = 1.0
step = 0.1

[options.invert]
default = false

[options.mode]
default = 1
values = [0, 1, 2]
"#;
    const GRADE: &str = r#"
override strength: f32 = 0.5;
override invert: bool = false;
override mode: i32 = 1;
@fragment fn fs_main(in: StageIn) -> @location(0) vec4<f32> {
    var c = textureSampleLevel(previous, linear_sampler, in.uv, 0.0).rgb;
    let d = linear_depth(depth_at(in.uv));
    c = mix(c, vec3(luminance(c)), strength) * f32(mode) + d * frame.fog_color.rgb * 0.0;
    if invert { c = 1.0 - c; }
    return vec4(c, 1.0);
}
"#;

    #[test]
    fn the_sample_pack_loads() {
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/shaderpacks/horizon-sample");
        let pack = ShaderPack::load(&dir, &BTreeMap::new()).unwrap();
        assert_eq!(pack.id, "horizon-sample");
        assert!(!pack.stages.is_empty());
        assert!(!pack.changed());
    }

    /// Every module of a pack, composed as the game composes it, translates
    /// to MSL (Metal), HLSL (DirectX 12) and SPIR-V (Vulkan), entry point by
    /// entry point, with the pack's option values as pipeline constants.
    fn translates_everywhere(pack: &ShaderPack) {
        // Each module with the constants wgpu gives it: a stage's own (by
        // name, or by id for a compiled stage), the hooks every option they
        // declare. A compiled stage's vertex shader is checked too.
        let named = |source: &str| -> Vec<(String, f64)> {
            let module = naga::front::wgsl::parse_str(source).expect("hooks parse");
            let declared: std::collections::HashSet<_> = module
                .overrides
                .iter()
                .filter_map(|(_, o)| o.name.clone())
                .collect();
            pack.constants()
                .into_iter()
                .filter(|(k, _)| declared.contains(*k))
                .map(|(k, v)| (k.to_string(), v))
                .collect()
        };
        let mut modules: Vec<(&str, &str, Vec<(String, f64)>)> = Vec::new();
        for s in &pack.stages {
            modules.push((s.file.as_str(), s.source.as_str(), s.constants.clone()));
            if let Some(v) = &s.vertex {
                modules.push(("stage vertex", v.as_str(), Vec::new()));
            }
        }
        let shadow_terrain = shadow_only_terrain();
        match &pack.gbuffer {
            Some(g) => {
                modules.push(("terrain hooks", &g.terrain, named(&g.terrain)));
                modules.push(("entity hooks", &g.entity, named(&g.entity)));
            }
            None if pack.shadow.is_some() => modules.push(("shadow", &shadow_terrain, Vec::new())),
            None => {}
        }
        for (name, source, constants) in modules {
            let module = naga::front::wgsl::parse_str(source)
                .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
            let info = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
            let constants: naga::back::PipelineConstants = constants.into_iter().collect();
            for entry in &module.entry_points {
                let (module, info) = naga::back::pipeline_constants::process_overrides(
                    &module,
                    &info,
                    Some((entry.stage, &entry.name)),
                    &constants,
                )
                .unwrap_or_else(|e| panic!("{name} {}: {e}", entry.name));
                let at = |what: &str| format!("{name} {}: {what}", entry.name);
                let mut hlsl = String::new();
                naga::back::hlsl::Writer::new(
                    &mut hlsl,
                    &naga::back::hlsl::Options::default(),
                    &naga::back::hlsl::PipelineOptions {
                        entry_point: Some((entry.stage, entry.name.clone())),
                    },
                )
                .write(&module, &info, None)
                .unwrap_or_else(|e| panic!("{}", at(&format!("HLSL: {e}"))));
                naga::back::msl::write_string(
                    &module,
                    &info,
                    &naga::back::msl::Options::default(),
                    &naga::back::msl::PipelineOptions {
                        entry_point: Some((entry.stage, entry.name.clone())),
                        ..Default::default()
                    },
                )
                .unwrap_or_else(|e| panic!("{}", at(&format!("MSL: {e}"))));
                naga::back::spv::write_vec(
                    &module,
                    &info,
                    &naga::back::spv::Options::default(),
                    Some(&naga::back::spv::PipelineOptions {
                        shader_stage: entry.stage,
                        entry_point: entry.name.clone(),
                    }),
                )
                .unwrap_or_else(|e| panic!("{}", at(&format!("SPIR-V: {e}"))));
            }
        }
    }

    #[test]
    fn every_example_pack_loads_and_translates_for_metal_directx_and_vulkan() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/shaderpacks");
        let mut seen = 0;
        for entry in fs::read_dir(&root).unwrap().flatten() {
            if !entry.path().join("pack.toml").is_file() {
                continue;
            }
            let pack = ShaderPack::load(&entry.path(), &BTreeMap::new())
                .unwrap_or_else(|e| panic!("{}: {e}", entry.path().display()));
            translates_everywhere(&pack);
            seen += 1;
        }
        assert!(seen >= 2, "{seen} example packs");
    }

    #[test]
    fn compiled_stages_get_their_options_by_id() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/shaderpacks/daybreak");
        let saved = BTreeMap::from([("fog_density".to_string(), "2".to_string())]);
        let pack = ShaderPack::load(&dir, &saved).unwrap();
        let atmosphere = pack
            .stages
            .iter()
            .find(|s| s.entry == "atmosphere")
            .unwrap();
        assert!(atmosphere.vertex.is_some());
        assert!(atmosphere.source.contains("fn atmosphere"));
        let fog = atmosphere.constants.iter().find(|(k, _)| k == "1").unwrap();
        assert_eq!(fog.1, f64::from(2.0f32.to_bits()));
        // The module holds every entry point, so every option with an id.
        assert_eq!(atmosphere.constants.len(), 6);
    }

    #[test]
    fn api_2_reads_targets_textures_shadow_and_hooks() {
        let manifest = r#"
[pack]
name = "Two"
api = 2

[shadow]
resolution = 1024
distance = 64.0

[gbuffer]
shader = "shaders/world.wgsl"
clouds = false

[[target]]
name = "glow"
format = "rgba16float"
scale = 0.5
history = true

[[texture]]
name = "noise"
path = "textures/noise.png"
filter = "nearest"

[materials]
20 = ["block/*_ore"]

[[stage]]
kind = "deferred"
shader = "shaders/light.wgsl"

[[stage]]
kind = "composite"
shader = "shaders/glow.wgsl"
target = "glow"
"#;
        let stage = "@fragment fn fs_main(in: StageIn) -> @location(0) vec4<f32> {\n    let g = gbuffer_at(in.uv);\n    let n = textureSampleLevel(noise, repeat_sampler, in.uv, 0.0);\n    let h = textureSampleLevel(glow_history, linear_sampler, in.uv, 0.0);\n    return vec4(g.normal * shadow_lit(vec3(0.0), g.normal), 1.0) + n + h;\n}\n";
        let world = "fn surface_color(s: Surface) -> vec4<f32> { return s.albedo * shadow_lit(s.world, s.normal); }\n";
        let dir = pack(&[
            ("pack.toml", manifest),
            ("shaders/world.wgsl", world),
            ("shaders/light.wgsl", stage),
            ("shaders/glow.wgsl", stage),
        ]);
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 2, 2);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[255; 16])
                .unwrap();
        }
        fs::create_dir_all(dir.join("textures")).unwrap();
        fs::write(dir.join("textures/noise.png"), png).unwrap();
        let pack = ShaderPack::load(&dir, &BTreeMap::new()).unwrap();
        assert_eq!(pack.shadow.unwrap().resolution, 1024);
        assert_eq!(pack.targets[0].format, TargetFormat::Rgba16Float);
        assert!(pack.targets[0].history);
        assert_eq!(pack.textures[0].size, [2, 2]);
        assert!(!pack.textures[0].linear && pack.textures[0].repeat);
        let g = pack.gbuffer.as_ref().unwrap();
        assert!(!g.clouds && g.sun_moon && !g.sky);
        assert_eq!(pack.stages[0].kind, StageKind::Deferred);
        assert_eq!(pack.stages[1].target, Some(0));
        assert_eq!(
            resources(&pack.targets, &pack.textures),
            [
                ("glow".to_string(), true),
                ("glow_history".to_string(), true),
                ("noise".to_string(), true)
            ]
        );
        assert_eq!(material("minecraft:block/iron_ore", &pack.materials), 20);
        translates_everywhere(&pack);
        // A broken hook points into its file.
        fs::write(
            dir.join("shaders/world.wgsl"),
            "\nfn surface_color(s: Surface) -> vec4<f32> { return nope; }\n",
        )
        .unwrap();
        let error = ShaderPack::load(&dir, &BTreeMap::new()).err().unwrap();
        assert!(error.starts_with("shaders/world.wgsl:2:"), "{error}");
        // Deferred stages need the split world pass of `[gbuffer]`.
        let no_gbuffer = manifest.replace(
            "[gbuffer]\nshader = \"shaders/world.wgsl\"\nclouds = false\n",
            "",
        );
        fs::write(dir.join("pack.toml"), no_gbuffer).unwrap();
        let error = ShaderPack::load(&dir, &BTreeMap::new()).err().unwrap();
        assert!(error.contains("needs `[gbuffer]`"), "{error}");
    }

    #[test]
    fn sprites_are_sorted_into_materials() {
        for (name, id) in [
            ("minecraft:block/water_still", 4),
            ("minecraft:block/oak_leaves", 2),
            ("minecraft:block/short_grass", 3),
            ("minecraft:block/tall_grass_top", 3),
            ("minecraft:block/red_tulip", 3),
            ("minecraft:block/glowstone", 7),
            ("minecraft:block/lava_flow", 5),
            ("minecraft:block/white_stained_glass", 6),
            ("minecraft:block/stone", 1),
            ("minecraft:block/torchflower_crop_stage0", 1),
        ] {
            assert_eq!(material(name, &[]), id, "{name}");
        }
        let layers = BTreeMap::from([
            ("minecraft:block/stone".to_string(), 0),
            ("minecraft:block/water_still".to_string(), 5),
        ]);
        assert_eq!(material_table(&layers, &[]), [0x0101_0101, 0x0101_0401]);
        assert!(glob("a*c*e", "abcde") && !glob("a*c", "abd"));
    }

    #[test]
    fn options_take_saved_values_within_their_range() {
        let dir = pack(&[("pack.toml", MANIFEST), ("shaders/grade.wgsl", GRADE)]);
        let values = BTreeMap::from([
            ("strength".to_string(), "7".to_string()),
            ("invert".to_string(), "true".to_string()),
            ("mode".to_string(), "5".to_string()),
        ]);
        let pack = ShaderPack::load(&dir, &values).unwrap();
        assert_eq!(
            pack.constants(),
            [("invert", 1.0), ("mode", 1.0), ("strength", 1.0)]
        );
        assert_eq!(pack.stages[0].entry, "fs_main");
        assert_eq!(pack.stages[0].kind, StageKind::Composite);
    }

    #[test]
    fn errors_point_into_the_pack_file() {
        let broken = "\n@fragment fn fs_main(in: StageIn) -> @location(0) vec4<f32> {\n    return vec4(nope, 1.0);\n}\n";
        let dir = pack(&[("pack.toml", MANIFEST), ("shaders/grade.wgsl", broken)]);
        let error = ShaderPack::load(&dir, &BTreeMap::new()).err().unwrap();
        assert!(error.starts_with("shaders/grade.wgsl:3:"), "{error}");
        let wrong_entry = GRADE.replace("fs_main", "other");
        let dir = pack(&[
            ("pack.toml", MANIFEST),
            ("shaders/grade.wgsl", &wrong_entry),
        ]);
        let error = ShaderPack::load(&dir, &BTreeMap::new()).err().unwrap();
        assert_eq!(
            error,
            "shaders/grade.wgsl: no fragment entry point `fs_main`"
        );
    }

    #[test]
    fn manifests_are_checked() {
        for (manifest, expected) in [
            (
                MANIFEST.replace("api = 1", "api = 3"),
                "needs shader pack api 3",
            ),
            (
                MANIFEST.replace("\"composite\"", "\"shadow\""),
                "stage kind `shadow`",
            ),
            (
                MANIFEST.replace("grade.wgsl", "../grade.wgsl"),
                "outside the pack",
            ),
            (MANIFEST.replace("min = 0.0\n", ""), "a number needs min"),
            (
                MANIFEST.replace("values = [0, 1, 2]", "values = [0]"),
                "holding its default",
            ),
            (
                MANIFEST.replace("name = \"Test\"", "name = \"Test\"\ncolour = 1"),
                "unknown field",
            ),
        ] {
            let dir = pack(&[("pack.toml", &manifest), ("shaders/grade.wgsl", GRADE)]);
            let error = ShaderPack::load(&dir, &BTreeMap::new()).err().unwrap();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn edits_change_the_stamp_and_the_list_shows_every_pack() {
        let root = scratch();
        let dir = root.join("mine");
        fs::create_dir_all(dir.join("shaders")).unwrap();
        fs::write(dir.join("pack.toml"), MANIFEST).unwrap();
        fs::write(dir.join("shaders/grade.wgsl"), GRADE).unwrap();
        fs::create_dir_all(root.join("broken")).unwrap();
        fs::write(root.join("broken/pack.toml"), "[pack]\n").unwrap();
        fs::create_dir_all(root.join("not-a-pack")).unwrap();
        let pack = ShaderPack::load(&dir, &BTreeMap::new()).unwrap();
        assert!(!pack.changed());
        fs::write(dir.join("shaders/grade.wgsl"), format!("{GRADE}\n")).unwrap();
        assert!(pack.changed());
        let listed = list(&root);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].0, "broken");
        assert!(listed[0].1.is_err());
        assert_eq!(listed[1].1.as_ref().unwrap().name, "Test");
    }

    #[test]
    fn saved_values_are_read_per_pack() {
        let dir = scratch();
        fs::write(dir.join("mine.txt"), "strength = 0.25\ninvert=true\n").unwrap();
        let values = load_values(&dir, "mine");
        assert_eq!(values["strength"], "0.25");
        assert_eq!(values["invert"], "true");
        assert!(load_values(&dir, "other").is_empty());
    }
}
