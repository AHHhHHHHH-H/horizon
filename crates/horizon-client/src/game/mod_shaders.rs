//! Mods' own shaders (docs/mods.md, "Shaders from a mod"): the passes mods register with
//! `render.register-shader`, handed to the renderer when they change and their `params`
//! every frame. The renderer times each on the GPU; a pass that keeps going over its budget is
//! removed and the player told, as a mod out of fuel is stopped. With no mod shader, this is
//! one comparison of two empty lists a frame.
use super::Game;
use horizon_client::gfx::{ModPass, PASS_BUDGET_MS};

impl Game<'_> {
    pub(super) fn sync_mod_shaders(&mut self) {
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let mods = &mut self.mods.mods;
        let keys: Vec<(usize, u32)> = mods.shaders().map(|(i, _, s)| (i, s.handle)).collect();
        if keys != self.mods.shaders {
            let passes = mods
                .shaders()
                .map(|(_, id, s)| ModPass {
                    label: format!("{id}/{}", s.name),
                    stage: (*s.stage).clone(),
                })
                .collect();
            if let Err((i, e)) = renderer.set_mod_shaders(passes) {
                // The others are rebuilt without it next frame.
                let (owner, handle) = keys[i];
                mods.drop_shader(owner, handle, &format!("the GPU refused it: {e}"));
                self.mods.shaders.clear();
                self.show_mod_output();
                return;
            }
            self.mods.shaders = keys;
        }
        for (i, (_, _, s)) in mods.shaders().enumerate() {
            renderer.set_mod_shader_params(i, &s.values);
        }
        let over = renderer.mod_shaders_over_budget();
        for &(i, ms) in &over {
            let (owner, handle) = self.mods.shaders[i];
            let why =
                format!("it took {ms:.1} ms of GPU time a frame, over its {PASS_BUDGET_MS} ms");
            mods.drop_shader(owner, handle, &why);
        }
        if !over.is_empty() {
            self.show_mod_output();
        }
    }
}
