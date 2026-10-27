//! Bounded emissive texture overlays for sandboxed render-capable mods.
use crate::sprites::Sprites;
use std::{collections::BTreeMap, path::Path};

pub const MAX: usize = 128;

/// Documented pack suffix conventions, plus a safe suffix.emissive property.
pub fn suffixes(root: &Path) -> Vec<String> {
    let mut out = vec!["_e".into(), "_emissive".into()];
    let path = root.join("assets/minecraft/optifine/emissive.properties");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() <= 4096)
        && let Ok(text) = std::fs::read_to_string(path)
    {
        for line in text.lines().take(128) {
            if let Some((key, value)) = line.trim().split_once('=') {
                if key.trim() != "suffix.emissive" {
                    continue;
                }
                let value = value.trim();
                if !value.is_empty()
                    && value.len() <= 32
                    && value
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
                    && !out.iter().any(|s| s == value)
                {
                    out.push(value.into());
                }
            }
        }
    }
    out
}

/// Admit at most 256 extra texture layers. Header and compressed-file bounds
/// are checked before decoding. Invalid optional overlays never break a pack.
pub fn load(root: &Path, sprites: &mut Sprites) {
    let suffixes = suffixes(root);
    let bases: Vec<_> = sprites.layers.keys().cloned().collect();
    let mut candidates: Vec<String> = bases
        .into_iter()
        .flat_map(|base| suffixes.iter().map(move |s| format!("{base}{s}")))
        .collect();
    candidates.extend(
        crate::atlas_sources::list_textures(root)
            .into_iter()
            .filter(|id| {
                id.split_once(':')
                    .is_some_and(|(_, p)| p.starts_with("emissive/"))
            }),
    );
    candidates.sort();
    candidates.dedup();
    let mut count = 0;
    for id in candidates {
        if count >= 256 {
            break;
        }
        let path = crate::atlas_sources::texture_path(root, &id, "textures");
        if std::fs::metadata(&path).map_or(true, |m| m.len() > 1 << 20) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
            continue;
        }
        let size = |n| u32::from_be_bytes(bytes[n..n + 4].try_into().unwrap());
        if !(1..=64).contains(&size(16)) || !(1..=64).contains(&size(20)) {
            continue;
        }
        if crate::hud::decode_bytes(&bytes).is_err() {
            continue;
        }
        if sprites.insert(root, &id).is_ok() {
            count += 1;
        }
    }
}

/// Resolve stable, first-owner-wins pairs; the shader binary-searches by base.
pub fn mappings(
    layers: &BTreeMap<String, u32>,
    explicit: impl Iterator<Item = (String, String)>,
    suffixes: &[String],
) -> Vec<[u32; 4]> {
    let mut out = BTreeMap::new();
    for (base, overlay) in explicit {
        if out.len() >= MAX {
            break;
        }
        if let (Some(&b), Some(&o)) = (layers.get(&base), layers.get(&overlay)) {
            out.entry(b).or_insert(o);
        }
    }
    if !suffixes.is_empty() {
        for (name, &base) in layers {
            if out.len() >= MAX {
                break;
            }
            if let Some(&overlay) = suffixes
                .iter()
                .find_map(|s| layers.get(&format!("{name}{s}")))
            {
                out.entry(base).or_insert(overlay);
            }
        }
    }
    out.into_iter().map(|(b, o)| [b, o, 0, 0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlays_are_opt_in_sorted_and_first_owner_wins() {
        let layers = [
            ("minecraft:block/ore".into(), 2),
            ("minecraft:block/ore_e".into(), 9),
            ("a:emissive/red".into(), 3),
        ]
        .into();
        assert!(mappings(&layers, std::iter::empty(), &[]).is_empty());
        assert_eq!(
            mappings(&layers, std::iter::empty(), &["_e".into()]),
            vec![[2, 9, 0, 0]]
        );
        assert_eq!(
            mappings(
                &layers,
                std::iter::once(("minecraft:block/ore".into(), "a:emissive/red".into())),
                &["_e".into()]
            ),
            vec![[2, 3, 0, 0]]
        );
    }
    #[test]
    fn suffix_properties_are_bounded_and_cannot_name_paths() {
        let root = std::env::temp_dir().join(format!(
            "horizon-emissive-properties-{}",
            std::process::id()
        ));
        let dir = root.join("assets/minecraft/optifine");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("emissive.properties");
        std::fs::write(
            &file,
            "suffix.emissive = _glow\nsuffix.emissive=../../bad\n",
        )
        .unwrap();
        assert_eq!(suffixes(&root), vec!["_e", "_emissive", "_glow"]);
        std::fs::write(&file, "x".repeat(4097)).unwrap();
        assert_eq!(suffixes(&root), vec!["_e", "_emissive"]);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn mapping_budget_is_fixed_even_with_many_owners() {
        let mut layers = BTreeMap::new();
        for i in 0..200 {
            layers.insert(format!("minecraft:block/{i}"), i);
            layers.insert(format!("a:emissive/{i}"), i + 1000);
        }
        let maps = mappings(
            &layers,
            (0..200).map(|i| (format!("minecraft:block/{i}"), format!("a:emissive/{i}"))),
            &[],
        );
        assert_eq!(maps.len(), MAX);
        assert!(maps.windows(2).all(|w| w[0][0] < w[1][0]));
    }
}
