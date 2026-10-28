//! Shaders a Horizon mod ships in its `.hmod` and registers from its code
//! (docs/mods.md, "Shaders from a mod"): a post stage's WGSL, written as a
//! pack's is, plus `params`, a uniform of the mod's own numbers that it sets
//! every frame. A mod's shader is checked as a pack's stage is, with tighter
//! limits, since a mod registers it while the game runs.
use crate::check::{CheckedStage, Problem, check_source};
use crate::stages::{GROUP, Source, StageKind};
use std::time::{Duration, Instant};

/// Bytes of one mod shader's WGSL.
pub const MAX_SOURCE: usize = 64 << 10;
/// Numbers a mod shader may read from `params`.
pub const MAX_PARAMS: usize = 16;
/// naga expressions a mod shader may hold: a fifth of a pack stage's.
pub const MAX_EXPRESSIONS: usize = 4_000;
/// How long its check (parse, validation, translation for Vulkan, Metal and
/// DirectX 12) may take.
pub const COMPILE_BUDGET: Duration = Duration::from_millis(250);
/// `params`' binding in group 3, after the post stage's resources.
pub const PARAMS_BINDING: u32 = 6;
/// `params`' size in bytes: [`MAX_PARAMS`] `f32`s, however many the mod names.
pub const PARAMS_SIZE: u64 = MAX_PARAMS as u64 * 4;

fn problem(file: &str, message: String) -> Vec<Problem> {
    vec![Problem {
        file: file.to_owned(),
        line: 0,
        column: 0,
        message,
    }]
}

/// Checks the mod shader `file`, whose WGSL is `text`, as a stage of `kind`
/// reading the numbers `params` (by name, in order) from `params`.
pub fn check(
    kind: StageKind,
    file: &str,
    text: &str,
    params: &[String],
) -> Result<CheckedStage, Vec<Problem>> {
    check_within(kind, file, text, params, COMPILE_BUDGET)
}

fn check_within(
    kind: StageKind,
    file: &str,
    text: &str,
    params: &[String],
    budget: Duration,
) -> Result<CheckedStage, Vec<Problem>> {
    let start = Instant::now();
    if kind != StageKind::Post {
        return Err(problem(
            file,
            format!("a mod's shader is a post stage, not {}", kind.name()),
        ));
    }
    if text.len() > MAX_SOURCE {
        return Err(problem(file, format!("is over {} KiB", MAX_SOURCE >> 10)));
    }
    let params = params_struct(params).map_err(|m| problem(file, m))?;
    let source = Source {
        kind,
        prelude: format!(
            "{}{params}@group({GROUP}) @binding({PARAMS_BINDING}) var<uniform> params: Params;\n",
            kind.prelude()
        ),
        shared: &[],
        file,
        text,
    };
    let stage = check_source(&source, &[], MAX_EXPRESSIONS)?;
    let took = start.elapsed();
    if took > budget {
        return Err(problem(
            file,
            format!(
                "took {} ms to check; a mod's shader may take {}",
                took.as_millis(),
                budget.as_millis()
            ),
        ));
    }
    Ok(stage)
}

/// `struct Params`: an `f32` per name, padded to [`MAX_PARAMS`].
fn params_struct(names: &[String]) -> Result<String, String> {
    if names.len() > MAX_PARAMS {
        return Err(format!(
            "{} params; a mod's shader may have {MAX_PARAMS}",
            names.len()
        ));
    }
    let mut out = String::from("struct Params {\n");
    for (i, name) in names.iter().enumerate() {
        let ok = (1..=32).contains(&name.len())
            && name.starts_with(|c: char| c.is_ascii_lowercase())
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        if !ok || name.starts_with("horizon_") {
            return Err(format!(
                "param {name:?} is not [a-z][a-z0-9_]{{0,31}} (and not horizon_...)"
            ));
        }
        if names[..i].contains(name) {
            return Err(format!("param {name} is named twice"));
        }
        out += &format!("    {name}: f32,\n");
    }
    for i in names.len()..MAX_PARAMS {
        out += &format!("    horizon_pad{i}: f32,\n");
    }
    out += "}\n";
    naga::front::wgsl::parse_str(&out)
        .map_err(|_| format!("a param's name is a WGSL keyword: {}", names.join(", ")))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIGNETTE: &str = "fn post(input: PostInput) -> vec4<f32> {
    let d = distance(input.uv, vec2<f32>(0.5));
    let shade = 1.0 - params.strength * smoothstep(0.3, 0.8, d);
    return vec4<f32>(input.color.rgb * shade, input.color.a);
}
";

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| (*s).to_owned()).collect()
    }

    fn message(r: Result<CheckedStage, Vec<Problem>>) -> String {
        r.expect_err("refused")
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    }

    #[test]
    fn a_post_pass_reads_its_params() {
        let stage = check(StageKind::Post, "v.wgsl", VIGNETTE, &names(&["strength"])).unwrap();
        assert_eq!(stage.kind, StageKind::Post);
        assert!(stage.module.entry_points.len() == 2);
        // An unknown param is a WGSL error, with its line.
        let e = message(check(
            StageKind::Post,
            "v.wgsl",
            VIGNETTE,
            &names(&["radius"]),
        ));
        assert!(e.starts_with("v.wgsl:3:"), "{e}");
    }

    #[test]
    fn bad_wgsl_is_refused_where_it_is_wrong() {
        let e = message(check(
            StageKind::Post,
            "v.wgsl",
            "fn post(input: PostInput) -> vec4<f32> { return input.colour; }\n",
            &[],
        ));
        assert!(e.starts_with("v.wgsl:1:"), "{e}");
        let e = message(check(StageKind::Post, "v.wgsl", "fn main() {}\n", &[]));
        assert!(e.contains("defines no `fn post"), "{e}");
    }

    #[test]
    fn bindings_of_its_own_are_refused() {
        // A storage buffer it could write, or any other resource: the stage
        // reads only the prelude's.
        let e = message(check(
            StageKind::Post,
            "v.wgsl",
            "@group(3) @binding(9) var<storage, read_write> out: array<f32>;\n\
             fn post(input: PostInput) -> vec4<f32> { out[0] = 1.0; return input.color; }\n",
            &[],
        ));
        assert!(e.contains("declares the binding `out`"), "{e}");
        let e = message(check(
            StageKind::Post,
            "v.wgsl",
            "@group(0) @binding(0) var other: texture_2d<f32>;\n\
             fn post(input: PostInput) -> vec4<f32> { return input.color; }\n",
            &[],
        ));
        assert!(e.contains("declares the binding `other`"), "{e}");
        let e = message(check(
            StageKind::Post,
            "v.wgsl",
            "@fragment fn horizon_fs(input: PostInput) -> @location(0) vec4<f32> { return input.color; }\n",
            &[],
        ));
        assert!(e.contains("entry point"), "{e}");
    }

    #[test]
    fn limits_hold() {
        let big = format!("{VIGNETTE}// {}\n", "x".repeat(MAX_SOURCE));
        assert!(
            message(check(
                StageKind::Post,
                "v.wgsl",
                &big,
                &names(&["strength"])
            ))
            .contains("KiB")
        );
        let many: Vec<String> = (0..=MAX_PARAMS).map(|i| format!("p{i}")).collect();
        assert!(message(check(StageKind::Post, "v.wgsl", VIGNETTE, &many)).contains("params"));
        for bad in [&["Strength"][..], &["horizon_x"], &["a", "a"], &["loop"]] {
            assert!(
                check(StageKind::Post, "v.wgsl", VIGNETTE, &names(bad)).is_err(),
                "{bad:?}"
            );
        }
        // Under the size cap, over the expression cap.
        let steps: String = (0..1500).map(|_| "    c = c * 0.5 + 0.1;\n").collect();
        let long = format!(
            "fn post(input: PostInput) -> vec4<f32> {{\n    var c = input.color;\n{steps}    return c;\n}}\n"
        );
        assert!(long.len() < MAX_SOURCE);
        let e = message(check(StageKind::Post, "v.wgsl", &long, &[]));
        assert!(e.contains("expressions; a stage may hold 4000"), "{e}");
        let e = message(check(StageKind::Sky, "v.wgsl", VIGNETTE, &[]));
        assert!(e.contains("post stage"), "{e}");
        // The compile-time budget: none at all refuses even this one.
        let e = message(check_within(
            StageKind::Post,
            "v.wgsl",
            VIGNETTE,
            &names(&["strength"]),
            Duration::ZERO,
        ));
        assert!(e.contains("ms to check"), "{e}");
    }
}
