//! The Horizon side of a code mod's machine harness (tools/mod_machine_harness.py,
//! docs/research/code-mods-plan.md): runs a ported mod's block entity through
//! horizon-mods on Horizon's own item, fuel and cooking tables, and writes its
//! slots, block state and saved fields after every tick.
//!
//! cargo run -p horizon-client --example mod_machine_probe -- PREPARED MOD.hmod SCENARIOS.json OUT.json
//!
//! SCENARIOS.json is the list the NeoForge oracle replays on the Java mod:
//! `[{"name", "block", "ticks", "slots": {"0": ["minecraft:raw_iron", 8]},
//! "properties": {"lit": ["false", ["false", "true"]]}, "fields": ["CookTime"],
//! "events": [{"tick": 40, "slot": 1, "stack": ["minecraft:coal", 1]}]}]`.
//! Fields are read from the mod's save when it is a JSON object.
use horizon_client::{
    Result,
    assets::Assets,
    blocks::BlockFacts,
    containers::Providers,
    crafting::Recipes,
    items::{ItemStack, Items},
};
use horizon_mods::{
    BlockEntityView, CookingKind, CookingRecipe, ItemFacts, Mods, Property, Stack, Status,
};
use serde_json::{Map, Value, json};
use std::{fs, path::Path, sync::Arc};

struct Facts {
    items: Items,
    recipes: Recipes,
    providers: Providers,
}
impl ItemFacts for Facts {
    fn max_stack_size(&self, id: &str) -> u32 {
        self.items.get(id).map_or(0, |i| i.max_stack)
    }
    fn burn_time(&self, id: &str) -> u32 {
        let stack = ItemStack::new(id, 1);
        self.providers.burn_time(&self.items, &stack, false).max(0) as u32
    }
    fn remainder(&self, id: &str) -> Option<String> {
        self.items.get(id)?.crafting_remainder.clone()
    }
    fn cooking_recipe(&self, kind: CookingKind, input: &str) -> Option<CookingRecipe> {
        let kind = match kind {
            CookingKind::Smelting => "minecraft:smelting",
            CookingKind::Blasting => "minecraft:blasting",
            CookingKind::Smoking => "minecraft:smoking",
            CookingKind::Campfire => "minecraft:campfire_cooking",
        };
        let c = self.recipes.cook(kind, &ItemStack::new(input, 1))?;
        Some(CookingRecipe {
            output: Stack {
                id: c.result.item.to_string(),
                count: c.result.count,
            },
            cooking_time: c.time,
            experience: c.experience,
        })
    }
}

fn stack(v: &Value) -> Option<Stack> {
    Some(Stack {
        id: v[0].as_str()?.to_owned(),
        count: v[1].as_u64()? as u32,
    })
}

fn run(mods: &mut Mods, scenario: &Value) -> Result<Value> {
    let block = scenario["block"].as_str().ok_or("scenario without block")?;
    let (handle, slots) = mods
        .create_block_entity(block, &[])
        .ok_or_else(|| format!("no running mod has a block entity for {block}"))?;
    let mut view = BlockEntityView {
        handle,
        slots: vec![None; slots as usize],
        ..Default::default()
    };
    for (slot, s) in scenario["slots"].as_object().into_iter().flatten() {
        view.slots[slot.parse::<usize>()?] = stack(s);
    }
    for (name, p) in scenario["properties"].as_object().into_iter().flatten() {
        view.properties.push(Property {
            name: name.clone(),
            value: p[0].as_str().unwrap_or_default().to_owned(),
            values: p[1]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| Some(v.as_str()?.to_owned()))
                .collect(),
        });
    }
    let fields: Vec<&str> = scenario["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut trace = Vec::new();
    for tick in 0..scenario["ticks"].as_u64().unwrap_or(0) {
        for e in scenario["events"].as_array().into_iter().flatten() {
            if e["tick"].as_u64() == Some(tick) {
                view.slots[e["slot"].as_u64().unwrap_or(0) as usize] = stack(&e["stack"]);
            }
        }
        let mut views = [view];
        mods.tick_block_entities(&mut views);
        [view] = views;
        let saved = mods.save_block_entity(handle).unwrap_or_default();
        let saved: Value = serde_json::from_slice(&saved).unwrap_or(Value::Null);
        let slots: Map<String, Value> = view
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| Some((i.to_string(), json!([s.as_ref()?.id, s.as_ref()?.count]))))
            .collect();
        let properties: Map<String, Value> = view
            .properties
            .iter()
            .map(|p| (p.name.clone(), json!(p.value)))
            .collect();
        let values: Map<String, Value> = fields
            .iter()
            .map(|f| (f.to_string(), saved[f].clone()))
            .collect();
        trace.push(json!({"slots": slots, "properties": properties, "fields": values}));
    }
    mods.remove_block_entity(handle);
    Ok(json!({"name": scenario["name"], "trace": trace}))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, hmod, scenarios, out] = args.as_slice() else {
        return Err("usage: mod_machine_probe PREPARED MOD.hmod SCENARIOS.json OUT.json".into());
    };
    let root = Path::new(root);
    let assets = Assets::load(root)?;
    let facts = BlockFacts::load(root, assets.registry)?;
    let items = Items::load(root, assets.registry, &facts)?;
    let recipes = Recipes::load(root, &items)?;
    let providers = Providers::load(root);
    let mut mods = Mods::load(&[hmod.into()], |_| true, |_| false);
    if let Status::Failed(why) = &mods.entries()[0].status {
        return Err(format!("{hmod}: {why}").into());
    }
    mods.set_item_facts(Arc::new(Facts {
        items,
        recipes,
        providers,
    }));
    let scenarios: Value = serde_json::from_slice(&fs::read(scenarios)?)?;
    let mut results = Vec::new();
    for scenario in scenarios.as_array().into_iter().flatten() {
        results.push(run(&mut mods, scenario)?);
    }
    fs::write(out, serde_json::to_vec_pretty(&results)?)?;
    Ok(())
}
