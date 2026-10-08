//! Baked vanilla model layers (`entity-models.json`, exported from
//! `LayerDefinitions`) and the `ModelPart.render` transform chain.
use crate::mesher::Vertex;
use serde_json::Value;
use std::{collections::HashMap, fs, path::Path, sync::Arc};

#[derive(Clone, Debug)]
pub struct Polygon {
    /// x, y, z (model pixels), u, v (0..1 over the layer texture).
    pub vertices: [[f32; 5]; 4],
}
#[derive(Clone, Debug)]
pub struct Part {
    pub name: String,
    /// x, y, z, xRot, yRot, zRot, xScale, yScale, zScale
    pub pose: [f32; 9],
    pub polygons: Vec<Polygon>,
    pub children: Vec<Part>,
}
impl Part {
    pub fn parse(name: &str, v: &Value) -> Self {
        let pose: Vec<f32> = v["pose"].as_array().into_iter().flatten().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect();
        let mut p = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        for (i, x) in pose.iter().enumerate().take(9) {
            p[i] = *x;
        }
        let polygons = v["polygons"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|poly| {
                let verts: Vec<[f32; 5]> = poly
                    .as_array()?
                    .iter()
                    .map(|vx| {
                        let a: Vec<f32> = vx.as_array().into_iter().flatten().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect();
                        [a[0], a[1], a[2], a[3], a[4]]
                    })
                    .collect();
                (verts.len() == 4).then(|| Polygon { vertices: [verts[0], verts[1], verts[2], verts[3]] })
            })
            .collect();
        let mut children: Vec<Part> = v["children"].as_object().into_iter().flatten().map(|(k, c)| Part::parse(k, c)).collect();
        children.sort_by(|a, b| a.name.cmp(&b.name));
        Self { name: name.to_owned(), pose: p, polygons, children }
    }
    /// First descendant (depth-first) with this name.
    pub fn find(&self, name: &str) -> Option<&Part> {
        if self.name == name {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(name))
    }
}
/// Every baked layer keyed by `ModelLayerLocation` (`minecraft:pig#main`).
#[derive(Default)]
pub struct ModelLibrary {
    pub layers: HashMap<String, Arc<Part>>,
}
impl ModelLibrary {
    pub fn load(root: &Path) -> crate::Result<Self> {
        let models: Value = serde_json::from_slice(&fs::read(root.join("entity-models.json"))?)?;
        let layers = models
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), Arc::new(Part::parse("root", v))))
            .collect();
        Ok(Self { layers })
    }
    pub fn get(&self, key: &str) -> Option<&Arc<Part>> {
        self.layers.get(key)
    }
}
/// Load one baked model layer from `entity-models.json` (e.g. `minecraft:chest#main`).
pub fn load_model(root: &Path, key: &str) -> Option<Part> {
    let models: Value = serde_json::from_slice(&fs::read(root.join("entity-models.json")).ok()?).ok()?;
    models.get(key).map(|m| Part::parse("root", m))
}
/// ModelPart.render: translate, rotate ZYX, scale, then children. `pose_for`
/// returns the animated pose of a part (its baked pose when unanimated).
pub fn render_model(part: &Part, base: glam::Mat4, pose_for: &dyn Fn(&Part) -> [f32; 9], uv: [f32; 4], light: u32, overlay: u32, out: &mut Vec<Vertex>) {
    render_model_tinted(part, base, &|p| Some(pose_for(p)), uv, light, overlay, 0xFFFF_FFFF, out);
}
/// As [`render_model`], with an RGBA8 vertex tint; parts whose pose is `None`
/// are hidden together with their children (`ModelPart.visible = false`).
#[allow(clippy::too_many_arguments)]
pub fn render_model_tinted(
    part: &Part,
    base: glam::Mat4,
    pose_for: &dyn Fn(&Part) -> Option<[f32; 9]>,
    uv: [f32; 4],
    light: u32,
    overlay: u32,
    color: u32,
    out: &mut Vec<Vertex>,
) {
    let Some(p) = pose_for(part) else { return };
    let m = base
        * glam::Mat4::from_translation(glam::Vec3::new(p[0], p[1], p[2]) / 16.0)
        * glam::Mat4::from_quat(glam::Quat::from_euler(glam::EulerRot::ZYX, p[5], p[4], p[3]))
        * glam::Mat4::from_scale(glam::Vec3::new(p[6], p[7], p[8]));
    for poly in &part.polygons {
        for v in poly.vertices {
            let pos = m.transform_point3(glam::Vec3::new(v[0], v[1], v[2]) / 16.0);
            out.push(Vertex {
                pos: pos.to_array(),
                uv: [uv[0] + (uv[2] - uv[0]) * v[3], uv[1] + (uv[3] - uv[1]) * v[4]],
                layer: overlay,
                color,
                light,
            });
        }
    }
    for c in &part.children {
        render_model_tinted(c, m, pose_for, uv, light, overlay, color, out);
    }
}
