//! Compare first-identity deduplication over the real cached collision export.
//! Usage: cargo run -p horizon-world --profile fast --example collision_shape_dedup_bench -- RESOURCES
use horizon_core::geometry::VoxelShape;
use horizon_world::{
    collision::CachedCollisionRegistry,
    level::data::load_registry,
    nbt::{self, Limits},
    registry::BlockStateId,
};
use std::{collections::HashMap, fs, path::Path, time::Instant};

fn linear(shapes: &[Option<&VoxelShape>]) -> Vec<i32> {
    let mut unique = Vec::<VoxelShape>::new();
    shapes
        .iter()
        .map(|shape| match shape {
            None => -1,
            Some(shape) => match unique.iter().position(|s| s.same_identity(shape)) {
                Some(index) => index as i32,
                None => {
                    unique.push((*shape).clone());
                    (unique.len() - 1) as i32
                }
            },
        })
        .collect()
}

fn indexed(shapes: &[Option<&VoxelShape>]) -> Vec<i32> {
    let mut unique = Vec::<VoxelShape>::new();
    let mut lookup = HashMap::<usize, usize>::new();
    shapes
        .iter()
        .map(|shape| match shape {
            None => -1,
            Some(shape) => *lookup.entry(shape.identity_key()).or_insert_with(|| {
                let index = unique.len();
                unique.push((*shape).clone());
                index
            }) as i32,
        })
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args()
        .nth(1)
        .expect("resource directory argument");
    let root = Path::new(&root);
    let started = Instant::now();
    let registry = load_registry(root)?;
    let registry_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let collision_tag = nbt::read_named(
        &fs::read(root.join("cached-collision.nbt"))?,
        Limits::default(),
    )?
    .0
    .tag;
    let nbt_read_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let cached = CachedCollisionRegistry::from_nbt(&collision_tag, &registry)?;
    let collision_decode_ms = started.elapsed().as_secs_f64() * 1000.0;
    let shapes = (0..registry.len())
        .map(|i| cached.cached_shape(BlockStateId(i as u32)))
        .collect::<Result<Vec<_>, _>>()?;

    let expected = linear(&shapes);
    assert_eq!(expected, indexed(&shapes), "shape IDs changed");
    let mut linear_ms = Vec::new();
    let mut indexed_ms = Vec::new();
    for i in 0..24 {
        let start = Instant::now();
        let result = if i % 2 == 0 {
            linear(&shapes)
        } else {
            indexed(&shapes)
        };
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(result, expected);
        if i % 2 == 0 {
            linear_ms.push(elapsed);
        } else {
            indexed_ms.push(elapsed);
        }
    }
    linear_ms.sort_by(f64::total_cmp);
    indexed_ms.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::json!({
            "states": shapes.len(),
            "unique_identities": expected.iter().copied().filter(|id| *id >= 0).max().unwrap_or(-1) + 1,
            "linear_median_ms": (linear_ms[5] + linear_ms[6]) / 2.0,
            "indexed_median_ms": (indexed_ms[5] + indexed_ms[6]) / 2.0,
            "samples_per_algorithm": 12,
            "registry_load_ms": registry_ms,
            "collision_nbt_read_ms": nbt_read_ms,
            "collision_decode_ms": collision_decode_ms,
            "scope": "isolated shape identity deduplication over real cached shapes (warm process)"
        })
    );
    Ok(())
}
