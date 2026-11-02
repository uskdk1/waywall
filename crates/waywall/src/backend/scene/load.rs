//! `scene.toml` → `SceneSpec` (docs/wallpaper-format.md#씬-배경).
//!
//! 레이어 필드의 계층: defaults.toml `[types.scene.layer]`/`[types.scene.<타입>]` < 레이어 자신 < 사용자의 같은 섹션.
//! 표 수준에서 병합한 뒤 엄격한 스키마로 역직렬화하므로 모르는 필드는 오류다.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use toml::{Table, Value};
use waywall_proto::color::{linear_to_srgb, srgb_to_linear};
use waywall_proto::config::{Blend, Scale, SceneOptions};
use waywall_proto::project::WallpaperKind;

use super::{ImageLayer, SceneSpec, Texture};
use crate::assets::Assets;
use crate::config::{Layers, OutputConfig, merge};
use crate::gpu::compile_wgsl;
use crate::properties::{self, PropValue, Property};
use crate::wallpaper::Wallpaper;

const IMAGE_SHADER: &str = "shaders/scene/image.wgsl";
const POST_SHADER: &str = "shaders/scene/post.wgsl";
const TONEMAP_DIR: &str = "shaders/tonemap";
const PROPERTY_SIGIL: char = '$';
/// `scene.toml` 최상위 섹션.
const TOP_KEYS: [&str; 4] = ["scene", "layer", "light", "post"];
/// `[scene]`에서 배경 옵션(사용자가 덮어쓸 수 있는 값)으로 넘기는 키.
const SCENE_OPTION_KEYS: [&str; 2] = ["fit", "clear"];
const SCENE_SIZE_KEYS: [&str; 2] = ["width", "height"];
/// 사용자 설정에서 레이어 필드를 덮어쓰는 섹션: `[types.scene.layer]`, `[types.scene.<타입>]`.
const COMMON_SECTION: &str = "layer";
/// 아직 없는 기능: (필드 또는 레이어 타입, 들어오는 마일스톤).
const NOT_YET_TYPES: [(&str, &str); 2] = [("particles", "M6b"), ("shader", "M6c")];
const NOT_YET_FIELDS: [(&str, &str); 5] = [
    ("normal_map", "M6c"),
    ("lit", "M6c"),
    ("clip", "M6d"),
    ("follow", "M6d"),
    ("effects", "M6d"),
];

/// image 레이어 (병합 후). 필드 설명은 docs/wallpaper-format.md#레이어-타입.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageLayerDef {
    #[serde(rename = "type")]
    _kind: String,
    id: Option<String>,
    blend: Blend,
    opacity: f64,
    parallax: f64,
    visible: bool,
    texture: String,
    position: Option<[f64; 2]>,
    scale: Scale,
    rotation: f64,
}

/// `entry`: scene.toml 경로. 텍스처 경로는 그 디렉터리 기준.
pub fn load(
    entry: &Path,
    wp: &Wallpaper,
    layers: &Layers,
    cfg: &OutputConfig,
    assets: &Assets,
) -> Result<(SceneSpec, SceneOptions)> {
    let text = std::fs::read_to_string(entry)
        .with_context(|| format!("failed to read {}", entry.display()))?;
    let mut doc: Table = text
        .parse()
        .with_context(|| format!("invalid {}", entry.display()))?;
    let props = properties::resolve(wp.properties.as_ref(), &cfg.properties)?;
    substitute(&mut doc, &props)?;
    let base = entry.parent().unwrap_or(Path::new("."));
    let doc = SceneDoc::split(doc)?;

    let mut author = wp.options.clone().unwrap_or_default();
    merge(&mut author, &doc.options);
    let opts: SceneOptions = layers.type_options(WallpaperKind::Scene, Some(&author), cfg)?;

    let image_shader = compile_wgsl(IMAGE_SHADER, &asset_text(assets, IMAGE_SHADER)?)?;
    let post_shader = compile_post(assets, &opts.post.tonemap)?;

    let user_common = layers.user_type_section(WallpaperKind::Scene, COMMON_SECTION);
    let mut defs = Vec::new();
    let mut ids = HashSet::new();
    for (i, raw) in doc.layers.into_iter().enumerate() {
        let def = image_def(i, raw, &opts, user_common, layers)?;
        let id = def.id.clone().unwrap_or_else(|| format!("layer{i}"));
        ensure!(ids.insert(id.clone()), "duplicate layer id `{id}`");
        if def.visible {
            defs.push((id, def));
        }
    }

    // 디코딩은 텍스처마다 스레드 하나로 병렬 처리하고 모두 끝날 때까지 기다린다.
    let textures: Vec<Result<Texture>> = std::thread::scope(|s| {
        let handles: Vec<_> = defs
            .iter()
            .map(|(id, def)| {
                let path = base.join(&def.texture);
                s.spawn(move || {
                    decode(&path).with_context(|| format!("layer `{id}`: {}", path.display()))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| bail!("texture decoder panicked"))
            })
            .collect()
    });
    let center = [doc.size[0] / 2.0, doc.size[1] / 2.0];
    let mut image_layers = Vec::with_capacity(defs.len());
    for ((id, def), texture) in defs.into_iter().zip(textures) {
        image_layers.push(ImageLayer {
            id,
            blend: def.blend,
            opacity: def.opacity,
            parallax: def.parallax,
            position: def.position.unwrap_or(center),
            scale: def.scale.xy(),
            rotation: def.rotation,
            texture: texture?,
        });
    }

    let spec = SceneSpec {
        size: doc.size,
        fit: opts.fit,
        clear: opts.clear,
        exposure: opts.post.exposure,
        pointer_smoothing: opts.pointer_smoothing,
        settle_px: opts.settle_px,
        layers: image_layers,
        image_shader,
        post_shader,
    };
    Ok((spec, opts))
}

/// `scene.toml`을 나눈 결과.
struct SceneDoc {
    size: [f64; 2],
    /// 배경 옵션으로 병합할 작성자 값 (`fit`, `clear`, `post`).
    options: Table,
    layers: Vec<Table>,
}

impl SceneDoc {
    fn split(mut doc: Table) -> Result<Self> {
        if let Some(k) = doc.keys().find(|k| !TOP_KEYS.contains(&k.as_str())) {
            bail!("unknown section `{k}` (scene | layer | light | post)");
        }
        if let Some(lights) = doc.remove("light") {
            let empty = lights.as_array().is_some_and(Vec::is_empty);
            ensure!(empty, "[[light]] is not supported yet (M6c)");
        }
        let mut scene = match doc.remove("scene") {
            Some(Value::Table(t)) => t,
            Some(_) => bail!("[scene] must be a table"),
            None => bail!("missing [scene]"),
        };
        let mut size = [0.0; 2];
        for (slot, key) in size.iter_mut().zip(SCENE_SIZE_KEYS) {
            let v = scene
                .remove(key)
                .with_context(|| format!("[scene]: missing `{key}`"))?;
            *slot = number(&v).with_context(|| format!("[scene].{key} must be a number"))?;
            ensure!(*slot > 0.0, "[scene].{key} must be positive");
        }
        let mut options = Table::new();
        for (key, value) in scene {
            ensure!(
                SCENE_OPTION_KEYS.contains(&key.as_str()),
                "[scene]: unknown key `{key}`"
            );
            options.insert(key, value);
        }
        if let Some(post) = doc.remove("post") {
            ensure!(post.is_table(), "[post] must be a table");
            options.insert("post".into(), post);
        }
        let layers = match doc.remove("layer") {
            None => Vec::new(),
            Some(Value::Array(a)) => a
                .into_iter()
                .enumerate()
                .map(|(i, v)| match v {
                    Value::Table(t) => Ok(t),
                    _ => bail!("[[layer]] #{i} must be a table"),
                })
                .collect::<Result<_>>()?,
            Some(_) => bail!("`layer` must be an array of tables ([[layer]])"),
        };
        Ok(Self {
            size,
            options,
            layers,
        })
    }
}

fn image_def(
    i: usize,
    raw: Table,
    opts: &SceneOptions,
    user_common: Option<&Table>,
    layers: &Layers,
) -> Result<ImageLayerDef> {
    let name = raw
        .get("id")
        .and_then(Value::as_str)
        .map_or_else(|| format!("#{i}"), |id| format!("`{id}`"));
    let ctx = || format!("[[layer]] {name}");
    let kind = raw
        .get("type")
        .and_then(Value::as_str)
        .with_context(|| format!("{}: missing `type`", ctx()))?;
    if let Some((_, m)) = NOT_YET_TYPES.iter().find(|(t, _)| *t == kind) {
        bail!("{}: {kind} layers are not supported yet ({m})", ctx());
    }
    ensure!(
        kind == "image",
        "{}: unknown layer type `{kind}` (image | particles | shader)",
        ctx()
    );
    if let Some((f, m)) = NOT_YET_FIELDS.iter().find(|(f, _)| raw.contains_key(*f)) {
        bail!("{}: `{f}` is not supported yet ({m})", ctx());
    }

    // opts.layer/opts.image는 이미 defaults < 사용자. 레이어 값을 얹은 뒤 사용자 값을 다시 얹는다.
    let mut t = Table::try_from(&opts.layer)?;
    merge(&mut t, &Table::try_from(&opts.image)?);
    merge(&mut t, &raw);
    if let Some(u) = user_common {
        merge(&mut t, u);
    }
    if let Some(u) = layers.user_type_section(WallpaperKind::Scene, kind) {
        merge(&mut t, u);
    }
    t.try_into().with_context(ctx)
}

fn compile_post(assets: &Assets, tonemap: &str) -> Result<naga::Module> {
    let valid = !tonemap.is_empty()
        && tonemap
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    ensure!(
        valid,
        "tonemap `{tonemap}`: expected a file name like `agx`"
    );
    let tonemap_path = format!("{TONEMAP_DIR}/{tonemap}.wgsl");
    let src = format!(
        "{}\n{}",
        asset_text(assets, POST_SHADER)?,
        asset_text(assets, &tonemap_path)
            .with_context(|| format!("unknown tonemap `{tonemap}`"))?
    );
    compile_wgsl(&format!("{POST_SHADER} + {tonemap_path}"), &src)
}

fn asset_text(assets: &Assets, rel: &str) -> Result<String> {
    let bytes = assets.read(rel)?;
    String::from_utf8(bytes.into_owned()).with_context(|| format!("{rel} is not UTF-8"))
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Float(f) => Some(*f),
        Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

/// 값 전체가 `"$이름"`인 문자열을 속성 값으로 바꾼다.
fn substitute(table: &mut Table, props: &[Property]) -> Result<()> {
    for (_, value) in table.iter_mut() {
        substitute_value(value, props)?;
    }
    Ok(())
}

fn substitute_value(value: &mut Value, props: &[Property]) -> Result<()> {
    match value {
        Value::String(s) => {
            if let Some(name) = s.strip_prefix(PROPERTY_SIGIL) {
                let p = props
                    .iter()
                    .find(|p| p.name == name)
                    .with_context(|| format!("`{s}`: no such property in project.toml"))?;
                *value = match p.value {
                    PropValue::Float(f) => Value::Float(f64::from(f)),
                    PropValue::Int(i) => Value::Integer(i64::from(i)),
                    PropValue::Bool(b) => Value::Boolean(b),
                    PropValue::Color(c) => Value::String(c.to_string()),
                };
            }
        }
        Value::Array(a) => {
            for v in a {
                substitute_value(v, props)?;
            }
        }
        Value::Table(t) => substitute(t, props)?,
        _ => {}
    }
    Ok(())
}

fn decode(path: &Path) -> Result<Texture> {
    let img = image::ImageReader::open(path)?
        .with_guessed_format()?
        .decode()?
        .into_rgba8();
    let (width, height) = img.dimensions();
    let mut data = img.into_raw();
    premultiply(&mut data);
    Ok(Texture {
        width,
        height,
        data,
    })
}

/// straight-alpha sRGB → 선형 공간에서 알파를 곱한 뒤 다시 sRGB로 인코딩. 불투명 픽셀은 그대로.
fn premultiply(rgba: &mut [u8]) {
    let max = f64::from(u8::MAX);
    let lut: Vec<f64> = (0..=u8::MAX)
        .map(|c| srgb_to_linear(f64::from(c) / max))
        .collect();
    for px in rgba.as_chunks_mut::<4>().0 {
        match px[3] {
            u8::MAX => {}
            0 => px[..3].fill(0),
            a => {
                let a = f64::from(a) / max;
                for c in &mut px[..3] {
                    *c = (linear_to_srgb(lut[usize::from(*c)] * a) * max).round() as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props() -> Vec<Property> {
        vec![
            Property {
                name: "speed".into(),
                value: PropValue::Float(2.0),
            },
            Property {
                name: "show".into(),
                value: PropValue::Bool(false),
            },
            Property {
                name: "tint".into(),
                value: PropValue::Color("#ff0000".parse().unwrap()),
            },
        ]
    }

    #[test]
    fn substitutes_whole_strings_only() {
        let mut t: Table =
            "a = \"$speed\"\nb = [\"$show\", 1]\nc = { d = \"$tint\" }\ne = \"x$speed\""
                .parse()
                .unwrap();
        substitute(&mut t, &props()).unwrap();
        assert_eq!(t["a"], Value::Float(2.0));
        assert_eq!(t["b"][0], Value::Boolean(false));
        assert_eq!(t["c"]["d"], Value::String("#ff0000".into()));
        assert_eq!(t["e"], Value::String("x$speed".into()));
        let mut bad: Table = "a = \"$nope\"".parse().unwrap();
        assert!(substitute(&mut bad, &props()).is_err());
    }

    #[test]
    fn premultiplies_in_linear_space() {
        let mut px = [255, 128, 0, 255, 255, 255, 255, 0, 255, 255, 255, 128];
        premultiply(&mut px);
        assert_eq!(&px[..4], &[255, 128, 0, 255], "opaque untouched");
        assert_eq!(&px[4..8], &[0, 0, 0, 0], "transparent is black");
        // 선형 1.0 × 0.502 → sRGB 약 0.735
        assert_eq!(px[8], 188);
        assert_eq!(px[11], 128);
    }

    #[test]
    fn split_rejects_bad_documents() {
        for (bad, why) in [
            ("[scene]\nwidth = 10", "height"),
            ("[scene]\nwidth = 10\nheight = 0", "positive"),
            ("[scene]\nwidth = 10\nheight = 10\nzoom = 2", "zoom"),
            ("[scene]\nwidth = 10\nheight = 10\n[extra]", "extra"),
            (
                "[scene]\nwidth = 10\nheight = 10\n[[light]]\ntype = \"ambient\"",
                "M6c",
            ),
        ] {
            let Err(e) = SceneDoc::split(bad.parse().unwrap()) else {
                panic!("{bad} should be rejected");
            };
            assert!(format!("{e:#}").contains(why), "{bad}: {e:#}");
        }
        let d = SceneDoc::split(
            "[scene]\nwidth = 1920\nheight = 1080\nfit = \"contain\"\n[post]\nexposure = 2.0\n[[layer]]\ntype = \"image\""
                .parse()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(d.size, [1920.0, 1080.0]);
        assert_eq!(d.options["fit"].as_str(), Some("contain"));
        assert!(d.options["post"].is_table());
        assert_eq!(d.layers.len(), 1);
    }

    fn scene_opts(l: &Layers) -> SceneOptions {
        l.type_options(WallpaperKind::Scene, None, &l.output("X").unwrap())
            .unwrap()
    }

    #[test]
    fn layer_fields_user_beats_author() {
        let raw: Table =
            "type = \"image\"\ntexture = \"a.png\"\nparallax = 0.05\nscale = [1.1, 1.2]"
                .parse()
                .unwrap();

        let l = Layers::load_for_test("");
        let o = scene_opts(&l);
        let d = image_def(0, raw.clone(), &o, None, &l).unwrap();
        assert_eq!(d.parallax, 0.05, "layer beats defaults");
        assert_eq!(d.scale.xy(), [1.1, 1.2]);
        assert_eq!(d.opacity, 1.0, "defaults fill the rest");

        let l = Layers::load_for_test(
            "[types.scene.layer]\nparallax = 0.0\n[types.scene.image]\nrotation = 5.0",
        );
        let o = scene_opts(&l);
        let common = l.user_type_section(WallpaperKind::Scene, COMMON_SECTION);
        let d = image_def(0, raw, &o, common, &l).unwrap();
        assert_eq!(d.parallax, 0.0, "user beats layer");
        assert_eq!(d.rotation, 5.0);
    }

    #[test]
    fn layer_errors() {
        let l = Layers::load_for_test("");
        let o = scene_opts(&l);
        for (bad, why) in [
            ("type = \"particles\"", "M6b"),
            ("type = \"shader\"", "M6c"),
            ("type = \"blob\"", "unknown layer type"),
            ("type = \"image\"\ntexture = \"a.png\"\nlit = true", "M6c"),
            ("type = \"image\"\ntexture = \"a.png\"\nclip = \"x\"", "M6d"),
            ("type = \"image\"\ntexture = \"a.png\"\nzoom = 1", "zoom"),
            ("type = \"image\"", "texture"),
        ] {
            let e = image_def(0, bad.parse().unwrap(), &o, None, &l).expect_err(bad);
            assert!(format!("{e:#}").contains(why), "{bad}: {e:#}");
        }
    }

    /// 예제 씬은 전부 로드되어야 한다 (docs/development.md#테스트-방침).
    #[test]
    fn examples_load() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/scenes");
        let l = Layers::load_for_test("");
        let detect = l.global().unwrap().detect;
        let assets = Assets::new(std::path::PathBuf::from("/nonexistent"));
        let mut n = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let wp =
                crate::wallpaper::resolve(path.to_str().unwrap(), &detect, Path::new("/")).unwrap();
            let crate::wallpaper::Source::File(scene) = &wp.source else {
                panic!("{}", path.display());
            };
            let (spec, _) = load(scene, &wp, &l, &l.output("X").unwrap(), &assets)
                .unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
            assert!(!spec.layers.is_empty(), "{}", path.display());
            n += 1;
        }
        assert!(n >= 1);
    }

    #[test]
    fn builtin_shaders_compile() {
        let assets = Assets::new(std::path::PathBuf::from("/nonexistent"));
        compile_wgsl(IMAGE_SHADER, &asset_text(&assets, IMAGE_SHADER).unwrap()).unwrap();
        for t in ["aces", "agx", "reinhard", "none"] {
            compile_post(&assets, t).unwrap_or_else(|e| panic!("{t}: {e:#}"));
        }
        assert!(compile_post(&assets, "../x").is_err());
        assert!(compile_post(&assets, "nope").is_err());
    }
}
