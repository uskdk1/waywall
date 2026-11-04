//! `scene.toml` → `SceneSpec` (docs/wallpaper-format.md#씬-배경).
//!
//! 레이어 필드의 계층: defaults.toml `[types.scene.layer]`/`[types.scene.<타입>]` < 레이어 자신 < 사용자의 같은 섹션.
//! 표 수준에서 병합한 뒤 엄격한 스키마로 역직렬화하므로 모르는 필드는 오류다.
//! 예외는 파티클 `sprite`: 레이어가 무늬(`texture`/`sprite`)를 정했으면 사용자 값이 그것을 덮지 않는다.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use toml::{Table, Value};
use waywall_proto::Color;
use waywall_proto::color::{linear_to_srgb, srgb_to_linear};
use waywall_proto::config::{
    Blend, ChannelDefaults, Emitter, EmitterShape, OnLeave, ParticlePointer, PointerMode, Ramp,
    Scale, SceneOptions, Span, Turbulence, VelocityRange,
};
use waywall_proto::project::WallpaperKind;

use super::{
    ChannelInput, EffectKind, EffectSpec, Follow, ImageLayer, Layer, Light, ParticleLayer,
    SceneSpec, ShaderLayer, Texture,
};
use crate::assets::Assets;
use crate::backend::shader::{CHANNEL_SECTION, channel_name, parse_inputs};
use crate::config::{Layers, OutputConfig, merge};
use crate::gpu::compile_wgsl;
use crate::gpu::shadertoy::{self, Language, UniformLayout};
use crate::properties::{self, PropValue, Property};
use crate::wallpaper::Wallpaper;

const IMAGE_SHADER: &str = "shaders/scene/image.wgsl";
const LIGHTING_SHADER: &str = "shaders/scene/lighting.wgsl";
const POST_SHADER: &str = "shaders/scene/post.wgsl";
const TONEMAP_DIR: &str = "shaders/tonemap";
const VIGNETTE_SHADER: &str = "shaders/vignette.wgsl";
const BLOOM_DOWN_SHADER: &str = "shaders/bloom/downsample.wgsl";
const BLOOM_UP_SHADER: &str = "shaders/bloom/upsample.wgsl";
const SIMULATE_SHADER: &str = "shaders/particles/simulate.wgsl";
const PARTICLE_SHADER: &str = "shaders/particles/render.wgsl";
const INPUT_SHADER: &str = "shaders/scene/input.wgsl";
const PROPERTY_SIGIL: char = '$';
/// `scene.toml` 최상위 섹션.
const TOP_KEYS: [&str; 4] = ["scene", "layer", "light", "post"];
/// `[scene]`에서 배경 옵션(사용자가 덮어쓸 수 있는 값)으로 넘기는 키.
const SCENE_OPTION_KEYS: [&str; 2] = ["fit", "clear"];
const SCENE_SIZE_KEYS: [&str; 2] = ["width", "height"];
/// 사용자 설정에서 레이어 필드를 덮어쓰는 섹션: `[types.scene.layer]`, `[types.scene.<타입>]`.
const COMMON_SECTION: &str = "layer";
const IMAGE: &str = "image";
const PARTICLES: &str = "particles";
const SHADER: &str = "shader";
/// shader 레이어 입력: 이 레이어 아래에 지금까지 그려진 장면.
const BELOW: &str = "below";
/// 파티클 무늬를 정하는 레이어 필드.
const LOOK_KEYS: [&str; 2] = ["texture", "sprite"];
const SPRITE_KEY: &str = "sprite";
/// 레이어 타입이 받지 않는 공통 필드: (타입, 필드).
const UNSUPPORTED: [(&str, &str); 4] = [
    (PARTICLES, "clip"),
    (PARTICLES, "follow"),
    (PARTICLES, "effects"),
    (SHADER, "effects"),
];
const FOLLOW_SECTION: &str = "follow";
const EFFECTS_SECTION: &str = "effects";
const EFFECTS_DIR: &str = "shaders/effects";
const EFFECT_MASK_SHADER: &str = "shaders/scene/effect_mask.wgsl";
/// 직접 쓴 셰이더 효과의 타입 이름.
const EFFECT_SHADER: &str = "shader";
/// 효과 파일의 엔트리 (인터페이스).
const EFFECT_ENTRY: &str = "fs_main";
/// 효과 파일 uniform 중 waywall이 채우는 멤버 (docs/wallpaper-format.md#레이어-효과-effects).
const EFFECT_BUILTINS: [&str; 3] = ["time", "size", "resolution"];

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
    lit: bool,
    normal_map: Option<String>,
    clip: Option<String>,
    follow: Option<Table>,
    effects: Option<Vec<Table>>,
}

/// shader 레이어 (병합 후).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShaderLayerDef {
    #[serde(rename = "type")]
    _kind: String,
    id: Option<String>,
    blend: Blend,
    opacity: f64,
    parallax: f64,
    visible: bool,
    source: String,
    rect: Option<[f64; 4]>,
    inputs: Option<Table>,
    /// 채널에서 생략한 filter·wrap.
    channel: ChannelDefaults,
    render_scale: f64,
    clip: Option<String>,
    follow: Option<Table>,
}

/// particles 레이어 (병합 후).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParticleLayerDef {
    #[serde(rename = "type")]
    _kind: String,
    id: Option<String>,
    blend: Blend,
    opacity: f64,
    parallax: f64,
    visible: bool,
    max: u32,
    texture: Option<String>,
    sprite: String,
    emitter: Emitter,
    lifetime: Span,
    velocity: VelocityRange,
    gravity: [f64; 2],
    drag: f64,
    turbulence: Turbulence,
    size: Ramp<f64>,
    color: Ramp<Color>,
    emissive: f64,
    pointer: ParticlePointer,
    max_step: f64,
}

/// `[[light]]` (병합 후). 필드 설명은 docs/wallpaper-format.md#조명.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
enum LightDef {
    Ambient {
        color: Color,
        intensity: f64,
    },
    Point {
        #[serde(default)]
        position: Option<LightPosition>,
        color: Color,
        intensity: f64,
        radius: f64,
        height: f64,
        flicker: f64,
        flicker_speed: f64,
    },
    Directional {
        direction: [f64; 3],
        color: Color,
        intensity: f64,
    },
}

/// `[x, y]` 또는 `"pointer"`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum LightPosition {
    At([f64; 2]),
    Named(String),
}

const LIGHT_TYPES: [&str; 3] = ["ambient", "point", "directional"];
const LIGHT_SECTION: &str = "light";
const POINTER: &str = "pointer";

enum LayerDef {
    Image(Box<ImageLayerDef>),
    Shader(Box<ShaderLayerDef>),
    /// `bool`: `sprite`가 레이어 자신의 값(배경 디렉터리 기준)인지. 아니면 assets 기준.
    Particles(Box<ParticleLayerDef>, bool),
}

impl LayerDef {
    fn id(&self) -> Option<&String> {
        match self {
            LayerDef::Image(d) => d.id.as_ref(),
            LayerDef::Shader(d) => d.id.as_ref(),
            LayerDef::Particles(d, _) => d.id.as_ref(),
        }
    }

    fn visible(&self) -> bool {
        match self {
            LayerDef::Image(d) => d.visible,
            LayerDef::Shader(d) => d.visible,
            LayerDef::Particles(d, _) => d.visible,
        }
    }

    fn texture(&self) -> Option<&String> {
        match self {
            LayerDef::Image(d) => Some(&d.texture),
            LayerDef::Shader(_) => None,
            LayerDef::Particles(d, _) => d.texture.as_ref(),
        }
    }

    fn clip(&self) -> Option<&String> {
        match self {
            LayerDef::Image(d) => d.clip.as_ref(),
            LayerDef::Shader(d) => d.clip.as_ref(),
            LayerDef::Particles(..) => None,
        }
    }
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

    let image_shader = compile_image(assets)?;
    let post_shader = compile_post(assets, &opts.post.tonemap)?;
    let simulate_shader = compile_wgsl(SIMULATE_SHADER, &asset_text(assets, SIMULATE_SHADER)?)?;
    let input_shader = compile_wgsl(INPUT_SHADER, &asset_text(assets, INPUT_SHADER)?)?;

    let user_common = layers.user_type_section(WallpaperKind::Scene, COMMON_SECTION);
    let mut defs = Vec::new();
    let mut index = HashMap::new();
    for (i, raw) in doc.layers.into_iter().enumerate() {
        let def = layer_def(i, raw, &opts, user_common, layers)?;
        let id = def.id().cloned().unwrap_or_else(|| format!("layer{i}"));
        ensure!(
            index.insert(id.clone(), i).is_none(),
            "duplicate layer id `{id}`"
        );
        defs.push((id, def));
    }

    // shader 레이어의 채널 입력과 clip 대상. 레이어 id는 그보다 아래 레이어만 가리킬 수 있고,
    // 입력·clip으로 쓰이는 레이어는 visible = false여도 남긴다(그리지는 않음).
    let user_channel = layers
        .user_type_section(WallpaperKind::Scene, SHADER)
        .and_then(|t| t.get(CHANNEL_SECTION))
        .and_then(Value::as_table);
    let mut channel_defs = Vec::with_capacity(defs.len());
    let mut referenced = HashSet::new();
    for (k, (id, def)) in defs.iter().enumerate() {
        if let Some(c) = def.clip() {
            let j = *index
                .get(c)
                .with_context(|| format!("layer `{id}`: clip: no layer `{c}`"))?;
            ensure!(j < k, "layer `{id}`: clip `{c}` must be a layer below it");
            ensure!(
                !matches!(defs[j].1, LayerDef::Shader(_)),
                "layer `{id}`: clip `{c}` must be an image or particles layer"
            );
            referenced.insert(j);
        }
        let LayerDef::Shader(d) = def else {
            channel_defs.push(Default::default());
            continue;
        };
        let parsed = parse_inputs(d.inputs.as_ref(), &d.channel, user_channel)
            .with_context(|| format!("layer `{id}`"))?;
        for c in parsed.iter().flatten() {
            if let Some(&j) = index.get(&c.source) {
                ensure!(
                    j < k,
                    "layer `{id}`: input `{}` must be a layer below it",
                    c.source
                );
                referenced.insert(j);
            }
        }
        channel_defs.push(parsed);
    }
    let kept: Vec<usize> = (0..defs.len())
        .filter(|i| defs[*i].1.visible() || referenced.contains(i))
        .collect();
    let new_index: HashMap<usize, usize> = kept.iter().enumerate().map(|(n, &o)| (o, n)).collect();
    let mut defs: Vec<_> = defs.into_iter().zip(channel_defs).enumerate().collect();
    defs.retain(|(i, _)| new_index.contains_key(i));
    let drawn: Vec<bool> = defs.iter().map(|(_, ((_, d), _))| d.visible()).collect();

    // 디코딩은 텍스처마다 스레드 하나로 병렬 처리하고 모두 끝날 때까지 기다린다.
    let textures: Vec<Option<Result<Texture>>> = std::thread::scope(|s| {
        let handles: Vec<_> = defs
            .iter()
            .map(|(_, ((id, def), _))| {
                let path = base.join(def.texture()?);
                Some(s.spawn(move || {
                    decode(&path).with_context(|| format!("layer `{id}`: {}", path.display()))
                }))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.map(|h| {
                    h.join()
                        .unwrap_or_else(|_| bail!("texture decoder panicked"))
                })
            })
            .collect()
    });
    let mut scene_layers = Vec::with_capacity(defs.len());
    for ((old, ((id, def), channels)), texture) in defs.into_iter().zip(textures) {
        let texture = texture.transpose()?;
        scene_layers.push(match def {
            LayerDef::Image(d) => {
                let normal_map = d
                    .normal_map
                    .as_ref()
                    .map(|n| {
                        let file = base.join(n);
                        decode_raw(&file)
                            .with_context(|| format!("layer `{id}`: normal_map {}", file.display()))
                    })
                    .transpose()?;
                let position = d.position.unwrap_or([doc.size[0] / 2.0, doc.size[1] / 2.0]);
                Layer::Image(Box::new(ImageLayer {
                    id: id.clone(),
                    blend: d.blend,
                    opacity: d.opacity,
                    parallax: d.parallax,
                    position,
                    scale: d.scale.xy(),
                    rotation: d.rotation,
                    texture: texture.context("image layer without a texture")?,
                    lit: d.lit,
                    normal_map,
                    clip: d.clip.as_ref().map(|c| new_index[&index[c]]),
                    follow: follow_spec(d.follow, &opts, layers, position)
                        .with_context(|| format!("layer `{id}`: follow"))?,
                    effects: d
                        .effects
                        .unwrap_or_default()
                        .into_iter()
                        .enumerate()
                        .map(|(i, e)| {
                            effect_spec(e, base, &opts, layers, assets, &props)
                                .with_context(|| format!("layer `{id}`: effects[{i}]"))
                        })
                        .collect::<Result<_>>()?,
                }))
            }
            LayerDef::Particles(d, own_sprite) => {
                let sprite = if own_sprite {
                    let path = base.join(&d.sprite);
                    std::fs::read_to_string(&path)
                        .with_context(|| format!("layer `{id}`: {}", path.display()))?
                } else {
                    asset_text(assets, &d.sprite).with_context(|| format!("layer `{id}`"))?
                };
                let shader = compile_wgsl(
                    &format!("{PARTICLE_SHADER} + {}", d.sprite),
                    &format!("{}\n{sprite}", asset_text(assets, PARTICLE_SHADER)?),
                )
                .with_context(|| format!("layer `{id}`"))?;
                Layer::Particles(Box::new(particle_layer(id, *d, texture, shader, doc.size)))
            }
            LayerDef::Shader(d) => {
                let path = base.join(&d.source);
                let text = std::fs::read_to_string(&path)
                    .with_context(|| format!("layer `{id}`: {}", path.display()))?;
                let lang = Language::from_path(&path);
                let compiled =
                    shadertoy::compile(assets, lang, &text, &props, channels.len(), &d.source)
                        .with_context(|| format!("layer `{id}`"))?;
                let mut inputs: Vec<Option<ChannelInput>> = channels.iter().map(|_| None).collect();
                let mut sampling = vec![d.channel; channels.len()];
                for (i, c) in channels.into_iter().enumerate() {
                    let Some(c) = c else { continue };
                    sampling[i] = c.sampling;
                    inputs[i] = Some(if c.source == BELOW {
                        ChannelInput::Below
                    } else if let Some(j) = index.get(&c.source) {
                        ChannelInput::Layer(new_index[j])
                    } else {
                        let file = base.join(&c.source);
                        let mut t = decode(&file).with_context(|| {
                            format!("layer `{id}`: {}: {}", channel_name(i), file.display())
                        })?;
                        if lang == Language::Glsl {
                            flip_rows(&mut t);
                        }
                        ChannelInput::Image(t)
                    });
                }
                let rect = d.rect.unwrap_or([0.0, 0.0, doc.size[0], doc.size[1]]);
                let center = [rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0];
                Layer::Shader(Box::new(ShaderLayer {
                    id: id.clone(),
                    blend: d.blend,
                    opacity: d.opacity,
                    parallax: d.parallax,
                    rect,
                    compiled,
                    props: props.clone(),
                    lang,
                    inputs,
                    sampling,
                    render_scale: d.render_scale,
                    direct: d.opacity == 1.0
                        && d.render_scale == 1.0
                        && d.clip.is_none()
                        && !referenced.contains(&old),
                    clip: d.clip.as_ref().map(|c| new_index[&index[c]]),
                    follow: follow_spec(d.follow, &opts, layers, center)
                        .with_context(|| format!("layer `{id}`: follow"))?,
                }))
            }
        });
    }

    let user_lights = layers.user_type_section(WallpaperKind::Scene, LIGHT_SECTION);
    let scene_lights = doc
        .lights
        .into_iter()
        .enumerate()
        .map(|(i, raw)| light(i, raw, &opts, user_lights, doc.size))
        .collect::<Result<Vec<_>>>()?;
    let lit = scene_layers
        .iter()
        .any(|l| matches!(l, Layer::Image(i) if i.lit));
    ensure!(
        !lit || !scene_lights.is_empty(),
        "a layer has `lit = true` but the scene has no [[light]]"
    );
    let shader_effects = scene_layers.iter().any(|l| {
        matches!(l, Layer::Image(i) if i.effects.iter().any(|e| matches!(e.kind, EffectKind::Shader { .. })))
    });
    let effect_mask_shader = shader_effects
        .then(|| compile_wgsl(EFFECT_MASK_SHADER, &asset_text(assets, EFFECT_MASK_SHADER)?))
        .transpose()?;
    let post = &opts.post;
    ensure!(
        post.bloom.intensity >= 0.0 && post.bloom.threshold >= 0.0,
        "[post] bloom: `intensity` and `threshold` must be >= 0"
    );
    ensure!(
        (0.0..=1.0).contains(&post.vignette),
        "[post] vignette must be 0–1"
    );
    let bloom_shaders = if post.bloom.intensity > 0.0 && post.bloom.radius > 0 {
        Some((
            compile_wgsl(BLOOM_DOWN_SHADER, &asset_text(assets, BLOOM_DOWN_SHADER)?)?,
            compile_wgsl(BLOOM_UP_SHADER, &asset_text(assets, BLOOM_UP_SHADER)?)?,
        ))
    } else {
        None
    };
    let spec = SceneSpec {
        size: doc.size,
        fit: opts.fit,
        clear: opts.clear,
        exposure: opts.post.exposure,
        bloom: opts.post.bloom.clone(),
        vignette: opts.post.vignette,
        bloom_shaders,
        pointer_smoothing: opts.pointer_smoothing,
        settle_px: opts.settle_px,
        layers: scene_layers,
        drawn,
        lights: scene_lights,
        image_shader,
        post_shader,
        simulate_shader,
        input_shader,
        effect_mask_shader,
    };
    Ok((spec, opts))
}

fn particle_layer(
    id: String,
    d: ParticleLayerDef,
    texture: Option<Texture>,
    shader: naga::Module,
    design: [f64; 2],
) -> ParticleLayer {
    let e = &d.emitter;
    let extent = match e.shape {
        EmitterShape::Point => [0.0, 0.0],
        EmitterShape::Box => {
            let s = e.size.unwrap_or(design);
            [s[0] / 2.0, s[1] / 2.0]
        }
        EmitterShape::Circle => [e.radius.unwrap_or_default(); 2],
    };
    ParticleLayer {
        id,
        blend: d.blend,
        opacity: d.opacity,
        parallax: d.parallax,
        max: d.max,
        texture,
        shader,
        shape: e.shape,
        center: e.position.unwrap_or([design[0] / 2.0, design[1] / 2.0]),
        extent,
        rate: e.rate,
        burst: e.burst,
        lifetime: d.lifetime.range(),
        velocity: d.velocity,
        gravity: d.gravity,
        drag: d.drag,
        turbulence: d.turbulence,
        size: d.size.ends(),
        color: d.color.ends(),
        emissive: d.emissive,
        pointer: d.pointer,
        max_step: d.max_step,
    }
}

/// `scene.toml`을 나눈 결과.
struct SceneDoc {
    size: [f64; 2],
    /// 배경 옵션으로 병합할 작성자 값 (`fit`, `clear`, `post`).
    options: Table,
    layers: Vec<Table>,
    lights: Vec<Table>,
}

impl SceneDoc {
    fn split(mut doc: Table) -> Result<Self> {
        if let Some(k) = doc.keys().find(|k| !TOP_KEYS.contains(&k.as_str())) {
            bail!("unknown section `{k}` (scene | layer | light | post)");
        }
        let lights = match doc.remove(LIGHT_SECTION) {
            None => Vec::new(),
            Some(Value::Array(a)) => a
                .into_iter()
                .enumerate()
                .map(|(i, v)| match v {
                    Value::Table(t) => Ok(t),
                    _ => bail!("[[light]] #{i} must be a table"),
                })
                .collect::<Result<_>>()?,
            Some(_) => bail!("`light` must be an array of tables ([[light]])"),
        };
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
            lights,
        })
    }
}

fn layer_def(
    i: usize,
    raw: Table,
    opts: &SceneOptions,
    user_common: Option<&Table>,
    layers: &Layers,
) -> Result<LayerDef> {
    let name = raw
        .get("id")
        .and_then(Value::as_str)
        .map_or_else(|| format!("#{i}"), |id| format!("`{id}`"));
    let ctx = || format!("[[layer]] {name}");
    let kind = raw
        .get("type")
        .and_then(Value::as_str)
        .with_context(|| format!("{}: missing `type`", ctx()))?
        .to_owned();
    let type_defaults = match kind.as_str() {
        IMAGE => Table::try_from(&opts.image)?,
        PARTICLES => Table::try_from(&opts.particles)?,
        SHADER => Table::try_from(&opts.shader)?,
        _ => bail!(
            "{}: unknown layer type `{kind}` (image | particles | shader)",
            ctx()
        ),
    };
    if let Some((_, f)) = UNSUPPORTED
        .iter()
        .find(|(t, f)| *t == kind && raw.contains_key(*f))
    {
        bail!("{}: `{f}` is not supported for {kind} layers", ctx());
    }
    let own_look = LOOK_KEYS.iter().any(|k| raw.contains_key(*k));
    let own_sprite = raw.contains_key(SPRITE_KEY);

    // opts.layer/opts.<타입>은 이미 defaults < 사용자. 레이어 값을 얹은 뒤 사용자 값을 다시 얹는다.
    let mut t = Table::try_from(&opts.layer)?;
    merge(&mut t, &type_defaults);
    merge(&mut t, &raw);
    if let Some(u) = user_common {
        merge(&mut t, u);
    }
    if let Some(u) = layers.user_type_section(WallpaperKind::Scene, &kind) {
        let mut u = u.clone();
        if own_look {
            u.remove(SPRITE_KEY);
        }
        merge(&mut t, &u);
    }
    match kind.as_str() {
        IMAGE => return Ok(LayerDef::Image(Box::new(t.try_into().with_context(ctx)?))),
        SHADER => {
            let d: ShaderLayerDef = t.try_into().with_context(ctx)?;
            if let Some(r) = d.rect {
                ensure!(
                    r[2] > 0.0 && r[3] > 0.0,
                    "{}: `rect` size must be positive",
                    ctx()
                );
            }
            ensure!(
                d.render_scale > 0.0 && d.render_scale.is_finite(),
                "{}: `render_scale` must be positive",
                ctx()
            );
            return Ok(LayerDef::Shader(Box::new(d)));
        }
        _ => {}
    }
    let d: ParticleLayerDef = t.try_into().with_context(ctx)?;
    check_particles(&d).with_context(ctx)?;
    Ok(LayerDef::Particles(Box::new(d), own_sprite))
}

/// `[[light]]` 하나: 계층은 레이어 필드와 같다 (defaults < 씬 < 사용자 `[types.scene.light.<타입>]`).
fn light(
    i: usize,
    raw: Table,
    opts: &SceneOptions,
    user: Option<&Table>,
    design: [f64; 2],
) -> Result<Light> {
    let ctx = || format!("[[light]] #{i}");
    let kind = raw
        .get("type")
        .and_then(Value::as_str)
        .with_context(|| format!("{}: missing `type`", ctx()))?
        .to_owned();
    let defaults = Table::try_from(&opts.light)?;
    let Some(Value::Table(base)) = defaults.get(&kind) else {
        bail!(
            "{}: unknown light type `{kind}` ({})",
            ctx(),
            LIGHT_TYPES.join(" | ")
        );
    };
    let mut t = base.clone();
    merge(&mut t, &raw);
    if let Some(Value::Table(u)) = user.and_then(|u| u.get(&kind)) {
        merge(&mut t, u);
    }
    let rgb = |c: Color, k: f64| {
        let [r, g, b, _] = c.to_linear_premultiplied();
        [r * k, g * k, b * k]
    };
    Ok(match t.try_into::<LightDef>().with_context(ctx)? {
        LightDef::Ambient { color, intensity } => Light::Ambient {
            color: rgb(color, intensity),
        },
        LightDef::Point {
            position,
            color,
            intensity,
            radius,
            height,
            flicker,
            flicker_speed,
        } => {
            ensure!(radius > 0.0, "{}: `radius` must be positive", ctx());
            ensure!(
                (0.0..=1.0).contains(&flicker),
                "{}: `flicker` must be 0–1",
                ctx()
            );
            ensure!(
                flicker_speed >= 0.0,
                "{}: `flicker_speed` must be >= 0",
                ctx()
            );
            let position = match position {
                None => Some([design[0] / 2.0, design[1] / 2.0]),
                Some(LightPosition::At(p)) => Some(p),
                Some(LightPosition::Named(n)) if n == POINTER => None,
                Some(LightPosition::Named(n)) => {
                    bail!(
                        "{}: `position` must be [x, y] or \"{POINTER}\" (got `{n}`)",
                        ctx()
                    )
                }
            };
            Light::Point {
                position,
                color: rgb(color, intensity),
                radius,
                height,
                flicker,
                flicker_speed,
            }
        }
        LightDef::Directional {
            direction,
            color,
            intensity,
        } => {
            ensure!(
                direction.iter().any(|v| *v != 0.0),
                "{}: `direction` must not be zero",
                ctx()
            );
            Light::Directional {
                direction,
                color: rgb(color, intensity),
            }
        }
    })
}

/// `follow` 표 → 따라가기. 계층: `[types.scene.follow]` 내장값 < 레이어 < 사용자 섹션.
fn follow_spec(
    raw: Option<Table>,
    opts: &SceneOptions,
    layers: &Layers,
    default_anchor: [f64; 2],
) -> Result<Option<Follow>> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FollowDef {
        anchor: Option<[f64; 2]>,
        max_offset: [f64; 2],
        range: f64,
        smoothing: f64,
        on_leave: OnLeave,
    }
    let Some(raw) = raw else { return Ok(None) };
    let mut t = Table::try_from(&opts.follow)?;
    merge(&mut t, &raw);
    if let Some(u) = layers.user_type_section(WallpaperKind::Scene, FOLLOW_SECTION) {
        merge(&mut t, u);
    }
    let d: FollowDef = t.try_into()?;
    ensure!(d.range > 0.0, "`range` must be positive");
    ensure!(d.smoothing >= 0.0, "`smoothing` must be >= 0");
    ensure!(
        d.max_offset.iter().all(|v| *v >= 0.0),
        "`max_offset` must not be negative"
    );
    Ok(Some(Follow {
        anchor: d.anchor.unwrap_or(default_anchor),
        max_offset: d.max_offset,
        range: d.range,
        smoothing: d.smoothing,
        on_leave: d.on_leave,
    }))
}

/// `effects`의 한 항목 (docs/wallpaper-format.md#레이어-효과-effects).
fn effect_spec(
    mut raw: Table,
    base: &Path,
    opts: &SceneOptions,
    layers: &Layers,
    assets: &Assets,
    props: &[Property],
) -> Result<EffectSpec> {
    let kind = match raw.remove("type") {
        Some(Value::String(s)) => s,
        _ => bail!("missing `type`"),
    };
    let mask = match raw.remove("mask") {
        None => None,
        Some(Value::String(m)) => {
            let file = base.join(&m);
            Some(decode_raw(&file).with_context(|| format!("mask {}", file.display()))?)
        }
        Some(_) => bail!("`mask` must be a path"),
    };
    if kind == EFFECT_SHADER {
        return shader_effect(raw, mask, base, opts, layers, assets, props);
    }
    let valid = !kind.is_empty()
        && kind
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    ensure!(
        valid,
        "effect type `{kind}`: expected a file name like `sway`"
    );
    let path = format!("{EFFECTS_DIR}/{kind}.wgsl");
    let text = asset_text(assets, &path).with_context(|| format!("unknown effect `{kind}`"))?;
    let module = compile_wgsl(&path, &text)?;
    ensure!(
        module
            .entry_points
            .iter()
            .any(|e| e.name == EFFECT_ENTRY && e.stage == naga::ShaderStage::Fragment),
        "{path}: no @fragment fn {EFFECT_ENTRY}"
    );
    let layout = UniformLayout::from_module(&module)?;

    // 값: [types.scene.effects.<type>](내장 < 사용자) < 효과 표 < 사용자가 직접 적은 섹션.
    let mut t = match opts.effects.get(&kind) {
        Some(Value::Table(d)) => d.clone(),
        _ => Table::new(),
    };
    merge(&mut t, &raw);
    if let Some(Value::Table(u)) = layers
        .user_type_section(WallpaperKind::Scene, EFFECTS_SECTION)
        .and_then(|e| e.get(&kind))
    {
        merge(&mut t, u);
    }
    let members: Vec<&str> = layout.member_names().collect();
    let mut values = HashMap::new();
    for (key, value) in t {
        ensure!(
            members.contains(&key.as_str()) && !EFFECT_BUILTINS.contains(&key.as_str()),
            "`{key}` is not a field of effect `{kind}` (see {path})"
        );
        let nums = match &value {
            Value::Array(a) => a.iter().map(number).collect::<Option<Vec<_>>>(),
            v => number(v).map(|n| vec![n]),
        }
        .with_context(|| format!("`{key}` must be a number or an array of numbers"))?;
        values.insert(key, nums.into_iter().map(|n| n as f32).collect());
    }
    if let Some(m) = members
        .iter()
        .find(|m| !EFFECT_BUILTINS.contains(m) && !m.starts_with('_') && !values.contains_key(**m))
    {
        bail!("effect `{kind}`: missing `{m}`");
    }
    Ok(EffectSpec {
        kind: EffectKind::Builtin {
            name: kind,
            module,
            layout,
            values,
        },
        mask,
    })
}

/// `type = "shader"` 효과: Shadertoy 형식, iChannel0 = 효과를 적용할 이미지, iChannel1–3 = 이미지 경로.
#[allow(clippy::too_many_arguments)]
fn shader_effect(
    mut raw: Table,
    mask: Option<Texture>,
    base: &Path,
    opts: &SceneOptions,
    layers: &Layers,
    assets: &Assets,
    props: &[Property],
) -> Result<EffectSpec> {
    let source = match raw.remove("source") {
        Some(Value::String(s)) => s,
        _ => bail!("missing `source`"),
    };
    let inputs = match raw.remove("inputs") {
        None => None,
        Some(Value::Table(t)) => Some(t),
        Some(_) => bail!("`inputs` must be a table"),
    };
    if let Some(k) = raw.keys().next() {
        bail!("unknown field `{k}` (source | inputs | mask)");
    }
    let path = base.join(&source);
    let text = std::fs::read_to_string(&path).with_context(|| path.display().to_string())?;
    let lang = Language::from_path(&path);
    let user_channel = layers
        .user_type_section(WallpaperKind::Scene, SHADER)
        .and_then(|t| t.get(CHANNEL_SECTION))
        .and_then(Value::as_table);
    let defs = parse_inputs(inputs.as_ref(), &opts.shader.channel, user_channel)?;
    let compiled = shadertoy::compile(assets, lang, &text, props, defs.len(), &source)?;
    let mut channels: Vec<Option<Texture>> = defs.iter().map(|_| None).collect();
    let mut sampling = vec![opts.shader.channel; defs.len()];
    for (i, c) in defs.into_iter().enumerate() {
        let Some(c) = c else { continue };
        ensure!(
            i != 0,
            "inputs: iChannel0 is the image the effect applies to"
        );
        ensure!(
            c.source != BELOW,
            "inputs: effects take image files only (`{}`)",
            c.source
        );
        let file = base.join(&c.source);
        let mut t =
            decode(&file).with_context(|| format!("{}: {}", channel_name(i), file.display()))?;
        if lang == Language::Glsl {
            flip_rows(&mut t);
        }
        channels[i] = Some(t);
        sampling[i] = c.sampling;
    }
    Ok(EffectSpec {
        kind: EffectKind::Shader {
            compiled,
            props: props.to_vec(),
            lang,
            channels,
            sampling,
        },
        mask,
    })
}

fn check_particles(d: &ParticleLayerDef) -> Result<()> {
    ensure!(d.max > 0, "`max` must be at least 1");
    let [lo, hi] = d.lifetime.range();
    ensure!(
        lo > 0.0 && lo <= hi,
        "`lifetime` must be positive and [min, max] ordered"
    );
    let e = &d.emitter;
    ensure!(
        e.rate >= 0.0 && e.rate.is_finite(),
        "`emitter.rate` must be >= 0"
    );
    match e.shape {
        EmitterShape::Circle => ensure!(
            e.radius.is_some_and(|r| r > 0.0),
            "`emitter.radius` is required for circle (> 0)"
        ),
        EmitterShape::Box => ensure!(
            e.size.is_none_or(|s| s[0] >= 0.0 && s[1] >= 0.0),
            "`emitter.size` must not be negative"
        ),
        EmitterShape::Point => {}
    }
    ensure!(d.drag >= 0.0, "`drag` must be >= 0");
    ensure!(d.max_step > 0.0, "`max_step` must be positive");
    ensure!(
        d.turbulence.strength == 0.0 || d.turbulence.scale > 0.0,
        "`turbulence.scale` must be positive"
    );
    ensure!(
        d.pointer.mode == PointerMode::None || d.pointer.radius > 0.0,
        "`pointer.radius` must be positive"
    );
    let [s0, s1] = d.size.ends();
    ensure!(s0 >= 0.0 && s1 >= 0.0, "`size` must not be negative");
    Ok(())
}

/// RGBA8 행을 위아래로 뒤집는다 (GLSL 채널은 Shadertoy처럼 uv (0, 0)이 왼쪽 아래).
fn flip_rows(t: &mut Texture) {
    let row = t.width as usize * 4;
    let rows: Vec<Vec<u8>> = t.data.chunks(row).rev().map(<[u8]>::to_vec).collect();
    t.data = rows.concat();
}

fn compile_image(assets: &Assets) -> Result<naga::Module> {
    let src = format!(
        "{}\n{}",
        asset_text(assets, IMAGE_SHADER)?,
        asset_text(assets, LIGHTING_SHADER)?
    );
    compile_wgsl(&format!("{IMAGE_SHADER} + {LIGHTING_SHADER}"), &src)
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
        "{}\n{}\n{}",
        asset_text(assets, POST_SHADER)?,
        asset_text(assets, &tonemap_path)
            .with_context(|| format!("unknown tonemap `{tonemap}`"))?,
        asset_text(assets, VIGNETTE_SHADER)?
    );
    compile_wgsl(
        &format!("{POST_SHADER} + {tonemap_path} + {VIGNETTE_SHADER}"),
        &src,
    )
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

/// 값 그대로의 RGBA8 (노멀맵: 선형, premultiply 없음).
fn decode_raw(path: &Path) -> Result<Texture> {
    let img = image::ImageReader::open(path)?
        .with_guessed_format()?
        .decode()?
        .into_rgba8();
    let (width, height) = img.dimensions();
    Ok(Texture {
        width,
        height,
        data: img.into_raw(),
    })
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

    fn image_def(
        raw: Table,
        o: &SceneOptions,
        common: Option<&Table>,
        l: &Layers,
    ) -> ImageLayerDef {
        match layer_def(0, raw, o, common, l).unwrap() {
            LayerDef::Image(d) => *d,
            _ => panic!("not an image layer"),
        }
    }

    fn particle_def(
        raw: &str,
        o: &SceneOptions,
        common: Option<&Table>,
        l: &Layers,
    ) -> (ParticleLayerDef, bool) {
        match layer_def(0, raw.parse().unwrap(), o, common, l).unwrap() {
            LayerDef::Particles(d, own) => (*d, own),
            _ => panic!("not a particles layer"),
        }
    }

    #[test]
    fn layer_fields_user_beats_author() {
        let raw: Table =
            "type = \"image\"\ntexture = \"a.png\"\nparallax = 0.05\nscale = [1.1, 1.2]"
                .parse()
                .unwrap();

        let l = Layers::load_for_test("");
        let o = scene_opts(&l);
        let d = image_def(raw.clone(), &o, None, &l);
        assert_eq!(d.parallax, 0.05, "layer beats defaults");
        assert_eq!(d.scale.xy(), [1.1, 1.2]);
        assert_eq!(d.opacity, 1.0, "defaults fill the rest");

        let l = Layers::load_for_test(
            "[types.scene.layer]\nparallax = 0.0\n[types.scene.image]\nrotation = 5.0",
        );
        let o = scene_opts(&l);
        let common = l.user_type_section(WallpaperKind::Scene, COMMON_SECTION);
        let d = image_def(raw, &o, common, &l);
        assert_eq!(d.parallax, 0.0, "user beats layer");
        assert_eq!(d.rotation, 5.0);
    }

    #[test]
    fn particle_fields_and_sprite_rule() {
        let l = Layers::load_for_test("");
        let o = scene_opts(&l);
        let (d, own) = particle_def(
            "type = \"particles\"\nemitter = { shape = \"box\", rate = 50 }\nsize = { start = 6, end = 2 }",
            &o,
            None,
            &l,
        );
        assert!(!own);
        assert_eq!(d.blend, Blend::Add, "particle defaults beat layer defaults");
        assert_eq!(d.emitter.shape, EmitterShape::Box);
        assert_eq!(d.emitter.rate, 50.0);
        assert_eq!(d.emitter.burst, 0, "emitter table merges with defaults");
        assert_eq!(d.size.ends(), [6.0, 2.0]);
        assert_eq!(d.color.ends()[0], "#ffffff".parse().unwrap());
        assert_eq!(d.sprite, o.particles.sprite);

        // 사용자 sprite는 무늬를 정하지 않은 레이어의 기본값일 뿐, 그 밖의 필드는 사용자가 이긴다.
        let l = Layers::load_for_test(
            "[types.scene.particles]\nsprite = \"sprites/mine.wgsl\"\ndrag = 2.0",
        );
        let o = scene_opts(&l);
        let user = l.user_type_section(WallpaperKind::Scene, PARTICLES);
        assert!(user.is_some());
        let (d, own) = particle_def("type = \"particles\"\ndrag = 0.5", &o, None, &l);
        assert_eq!(
            (d.sprite.as_str(), own, d.drag),
            ("sprites/mine.wgsl", false, 2.0)
        );
        let (d, own) = particle_def("type = \"particles\"\nsprite = \"dot.wgsl\"", &o, None, &l);
        assert_eq!((d.sprite.as_str(), own), ("dot.wgsl", true));
        let (d, own) = particle_def("type = \"particles\"\ntexture = \"glow.png\"", &o, None, &l);
        assert_eq!(d.texture.as_deref(), Some("glow.png"));
        assert!(!own);
    }

    #[test]
    fn layer_errors() {
        let l = Layers::load_for_test("");
        let o = scene_opts(&l);
        for (bad, why) in [
            ("type = \"shader\"", "source"),
            (
                "type = \"shader\"\nsource = \"a.glsl\"\nrect = [0, 0, 0, 10]",
                "rect",
            ),
            (
                "type = \"shader\"\nsource = \"a.glsl\"\nrender_scale = 0",
                "render_scale",
            ),
            ("type = \"blob\"", "unknown layer type"),
            ("type = \"image\"\ntexture = \"a.png\"\nlit = 1", "lit"),
            (
                "type = \"particles\"\nclip = \"x\"",
                "not supported for particles",
            ),
            (
                "type = \"shader\"\nsource = \"a.glsl\"\neffects = []",
                "not supported for shader",
            ),
            ("type = \"image\"\ntexture = \"a.png\"\nzoom = 1", "zoom"),
            ("type = \"image\"", "texture"),
            ("type = \"particles\"\nmax = 0", "max"),
            ("type = \"particles\"\nlifetime = [3.0, 1.0]", "lifetime"),
            (
                "type = \"particles\"\nemitter = { shape = \"circle\" }",
                "radius",
            ),
            (
                "type = \"particles\"\nemitter = { shape = \"star\" }",
                "star",
            ),
            (
                "type = \"particles\"\npointer = { mode = \"repel\", radius = 0 }",
                "radius",
            ),
            ("type = \"particles\"\nemitter = { rate = -1 }", "rate"),
            ("type = \"particles\"\nwind = 3", "wind"),
        ] {
            let e = match layer_def(0, bad.parse().unwrap(), &o, None, &l) {
                Ok(_) => panic!("{bad} should be rejected"),
                Err(e) => e,
            };
            assert!(format!("{e:#}").contains(why), "{bad}: {e:#}");
        }
    }

    #[test]
    fn follow_layers_like_other_fields() {
        let l = Layers::load_for_test("[types.scene.follow]\nsmoothing = 0.5");
        let o = scene_opts(&l);
        let raw: Table = "max_offset = [6, 4]\nsmoothing = 0.0\non_leave = \"stay\""
            .parse()
            .unwrap();
        let f = follow_spec(Some(raw), &o, &l, [10.0, 20.0])
            .unwrap()
            .unwrap();
        assert_eq!(f.anchor, [10.0, 20.0], "defaults to the layer center");
        assert_eq!(f.max_offset, [6.0, 4.0]);
        assert_eq!(f.range, o.follow.range, "built-in default");
        assert_eq!(f.smoothing, 0.5, "user beats layer");
        assert_eq!(f.on_leave, OnLeave::Stay);
        assert!(follow_spec(None, &o, &l, [0.0; 2]).unwrap().is_none());
        let bad: Table = "max_offset = [1, 1]\nrange = 0".parse().unwrap();
        assert!(follow_spec(Some(bad), &o, &l, [0.0; 2]).is_err());
        let missing: Table = "range = 5".parse().unwrap();
        assert!(follow_spec(Some(missing), &o, &l, [0.0; 2]).is_err());
    }

    #[test]
    fn builtin_effect_fields_match_uniform_members() {
        let l = Layers::load_for_test("[types.scene.effects.sway]\nspeed = 2.0");
        let o = scene_opts(&l);
        let assets = Assets::new(std::path::PathBuf::from("/nonexistent"));
        let base = Path::new("/");
        let raw: Table = "type = \"sway\"\nstrength = 9\nspeed = 1.0"
            .parse()
            .unwrap();
        let e = effect_spec(raw, base, &o, &l, &assets, &[]).unwrap();
        let EffectKind::Builtin { values, .. } = e.kind else {
            panic!("sway is a built-in effect");
        };
        assert_eq!(values["strength"], [9.0]);
        assert_eq!(values["speed"], [2.0], "user beats the effect table");
        assert_eq!(values["direction"], [1.0, 0.0], "built-in default");
        assert!(e.mask.is_none());
        for (bad, why) in [
            ("type = \"sway\"\nwobble = 1", "wobble"),
            ("type = \"sway\"\ntime = 1", "time"),
            ("type = \"sway\"\nstrength = \"big\"", "number"),
            ("type = \"nope\"", "unknown effect"),
            ("type = \"../x\"", "file name"),
            ("type = \"shader\"", "source"),
            ("strength = 1", "type"),
        ] {
            let Err(e) = effect_spec(bad.parse().unwrap(), base, &o, &l, &assets, &[]) else {
                panic!("{bad} should be rejected");
            };
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
        compile_image(&assets).unwrap();
        for t in ["aces", "agx", "reinhard", "none"] {
            compile_post(&assets, t).unwrap_or_else(|e| panic!("{t}: {e:#}"));
        }
        assert!(compile_post(&assets, "../x").is_err());
        assert!(compile_post(&assets, "nope").is_err());
        compile_wgsl(
            SIMULATE_SHADER,
            &asset_text(&assets, SIMULATE_SHADER).unwrap(),
        )
        .unwrap();
        let o = scene_opts(&Layers::load_for_test(""));
        let src = format!(
            "{}\n{}",
            asset_text(&assets, PARTICLE_SHADER).unwrap(),
            asset_text(&assets, &o.particles.sprite).unwrap()
        );
        compile_wgsl(PARTICLE_SHADER, &src).unwrap();
    }
}
