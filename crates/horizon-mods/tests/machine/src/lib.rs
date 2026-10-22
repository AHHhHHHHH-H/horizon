//! A block entity for the runtime's tests (API 0.4): `test_machine:smelter`
//! has an input and an output slot and smelts one input item every recipe's
//! cooking time, `lit` while it does. Its progress is saved and shown as the
//! menu's one data value; TNT in its input makes it panic (after lighting
//! up). `test_machine:crate` holds 4 slots and never ticks.
use horizon_mod_sdk::{
    BlockEntityType, Mod, Registration,
    machine::{self, CookingKind, ItemStack},
};
use std::collections::HashMap;

struct Machines {
    /// Each smelter's ticks of cooking so far.
    progress: HashMap<u32, u32>,
}

impl Mod for Machines {
    fn init() -> Result<(Self, Registration), String> {
        let progress = HashMap::new();
        Ok((Machines { progress }, Registration::new()))
    }
    fn block_entity_types(&mut self) -> Vec<BlockEntityType> {
        vec![
            BlockEntityType {
                block: "test_machine:smelter".into(),
                slots: 2,
                ticks: true,
            },
            BlockEntityType {
                block: "test_machine:crate".into(),
                slots: 4,
                ticks: false,
            },
        ]
    }
    fn create_block_entity(&mut self, handle: u32, block: &str, saved: &[u8]) {
        if block == "test_machine:smelter" {
            let progress = saved.try_into().map_or(0, u32::from_le_bytes);
            self.progress.insert(handle, progress);
        }
    }
    fn tick_block_entities(&mut self, handles: &[u32]) {
        for &h in handles {
            let progress = self.progress.entry(h).or_default();
            if machine::get_slot(h, 0).is_some_and(|s| s.id == "minecraft:tnt") {
                machine::set_property(h, "lit", "true");
                panic!("tnt in the smelter");
            }
            let recipe = machine::get_slot(h, 0)
                .and_then(|input| machine::cooking(CookingKind::Smelting, &input.id));
            let Some(recipe) = recipe else {
                *progress = 0;
                machine::set_property(h, "lit", "false");
                continue;
            };
            machine::set_property(h, "lit", "true");
            *progress += 1;
            if *progress < recipe.cooking_time {
                continue;
            }
            *progress = 0;
            let out = match machine::get_slot(h, 1) {
                Some(o) if o.id == recipe.output.id => ItemStack {
                    count: o.count + recipe.output.count,
                    ..o
                },
                Some(_) => continue,
                None => recipe.output,
            };
            if !machine::set_slot(h, 1, Some(&out)) {
                continue;
            }
            let input = machine::get_slot(h, 0).filter(|i| i.count > 1);
            let rest = input.map(|i| ItemStack {
                count: i.count - 1,
                ..i
            });
            machine::set_slot(h, 0, rest.as_ref());
        }
    }
    fn save_block_entity(&mut self, handle: u32) -> Vec<u8> {
        let progress = self.progress.get(&handle).copied().unwrap_or(0);
        progress.to_le_bytes().to_vec()
    }
    fn remove_block_entity(&mut self, handle: u32) {
        self.progress.remove(&handle);
    }
    fn block_entity_data(&mut self, handle: u32) -> Vec<i32> {
        vec![self.progress.get(&handle).copied().unwrap_or(0) as i32]
    }
}

horizon_mod_sdk::export_mod!(Machines);
