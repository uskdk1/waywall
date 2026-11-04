//! 셰이더 배경 소스 → naga 모듈 (docs/wallpaper-format.md#셰이더-배경).
//!
//! GLSL: `prelude.glsl` + 사용자 소스 + `epilogue.glsl`, WGSL: `prelude.wgsl` + 사용자 소스.
//! uniform 블록의 레이아웃은 코드에 고정하지 않고, 파싱한 모듈에서 멤버 이름과 offset을 읽어 채운다.
//! 그래서 프리앰블 asset을 바꿔도(멤버 순서·패딩) CPU 쪽이 따라간다.

use anyhow::{Context, Result, anyhow, bail};
use naga::{AddressSpace, Module, ScalarKind, ShaderStage, Span, TypeInner, VectorSize};

use crate::assets::Assets;
use crate::properties::{PropValue, Property};

const GLSL_PRELUDE: &str = "shaders/shadertoy/prelude.glsl";
const GLSL_EPILOGUE: &str = "shaders/shadertoy/epilogue.glsl";
const WGSL_PRELUDE: &str = "shaders/shadertoy/prelude.wgsl";
/// 프리앰블에서 속성 선언으로 바뀌는 줄.
const PROPERTIES_MARKER: &str = "// waywall:properties";
/// 프리앰블에서 채널 수만큼의 해상도 배열(uniform 멤버)로 바뀌는 줄.
const CHANNEL_RESOLUTION_MARKER: &str = "// waywall:channel_resolution";
/// 프리앰블에서 채널 텍스처·샘플러 선언으로 바뀌는 줄.
const CHANNELS_MARKER: &str = "// waywall:channels";
const PROPERTY_PREFIX: &str = "u_";

/// Shadertoy와 같이 늘 선언하는 채널 수(`iChannel0`–`iChannel3`). 입력을 지정하지 않은 Shadertoy 셰이더도 컴파일되게 한다.
pub const SHADERTOY_CHANNELS: usize = 4;
/// 바인딩: 0 = uniform, 1..=N = 채널 텍스처, N+1..=2N = 채널 샘플러 (N = 채널 수).
const TEXTURE_BINDING: u32 = 1;

pub fn texture_binding(i: usize) -> u32 {
    TEXTURE_BINDING + i as u32
}

pub fn sampler_binding(channels: usize, i: usize) -> u32 {
    TEXTURE_BINDING + (channels + i) as u32
}

/// 엔트리 포인트 이름 (인터페이스).
pub const GLSL_ENTRY: &str = "main";
pub const WGSL_ENTRY: &str = "main_image";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Glsl,
    Wgsl,
}

impl Language {
    pub fn from_path(path: &std::path::Path) -> Self {
        let wgsl = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("wgsl"));
        if wgsl { Language::Wgsl } else { Language::Glsl }
    }

    pub fn entry(self) -> &'static str {
        match self {
            Language::Glsl => GLSL_ENTRY,
            Language::Wgsl => WGSL_ENTRY,
        }
    }
}

pub struct Compiled {
    pub module: Module,
    pub entry: &'static str,
    pub layout: UniformLayout,
}

impl std::fmt::Debug for Compiled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Compiled")
            .field("entry", &self.entry)
            .field("layout", &self.layout)
            .finish()
    }
}

/// `name`: 오류 메시지에 쓸 사용자 파일 이름. `channels`: 셰이더가 받는 채널 수(`SHADERTOY_CHANNELS` 이상).
pub fn compile(
    assets: &Assets,
    lang: Language,
    user: &str,
    props: &[Property],
    channels: usize,
    name: &str,
) -> Result<Compiled> {
    let (prelude, epilogue) = match lang {
        Language::Glsl => (
            asset_text(assets, GLSL_PRELUDE)?,
            asset_text(assets, GLSL_EPILOGUE)?,
        ),
        Language::Wgsl => (asset_text(assets, WGSL_PRELUDE)?, String::new()),
    };
    let prelude = insert_properties(&prelude, lang, props)?;
    let prelude = insert_channels(&prelude, lang, channels)?;
    let source = format!("{prelude}{user}\n{epilogue}");
    let map = LineMap {
        name,
        prelude_lines: prelude.matches('\n').count() as u32,
        user_lines: user.lines().count() as u32,
    };

    let module = match lang {
        Language::Glsl => naga::front::glsl::Frontend::default()
            .parse(
                &naga::front::glsl::Options::from(ShaderStage::Fragment),
                &source,
            )
            .map_err(|errs| {
                let msgs: Vec<String> = errs
                    .errors
                    .iter()
                    .map(|e| map.describe(e.meta, &source, &e.kind.to_string()))
                    .collect();
                anyhow!("{}", msgs.join("\n"))
            })?,
        Language::Wgsl => naga::front::wgsl::parse_str(&source).map_err(|e| {
            let at = e
                .location(&source)
                .map(|l| map.line(l.line_number, l.line_position));
            anyhow!("{}: {}", at.unwrap_or_else(|| name.to_owned()), e.message())
        })?,
    };

    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map_err(|e| {
        let mut msg = format!("{name}: {}", e.as_inner());
        for (span, label) in e.spans() {
            msg.push('\n');
            msg.push_str(&map.describe(*span, &source, label));
        }
        anyhow!(msg)
    })?;

    let entry = lang.entry();
    if !module
        .entry_points
        .iter()
        .any(|e| e.name == entry && e.stage == ShaderStage::Fragment)
    {
        match lang {
            Language::Glsl => bail!("{name}: no mainImage(out vec4, in vec2) found"),
            Language::Wgsl => bail!("{name}: no @fragment fn {WGSL_ENTRY} found"),
        }
    }
    let layout = UniformLayout::from_module(&module)?;
    Ok(Compiled {
        module,
        entry,
        layout,
    })
}

fn asset_text(assets: &Assets, rel: &str) -> Result<String> {
    let bytes = assets.read(rel)?;
    String::from_utf8(bytes.into_owned()).with_context(|| format!("{rel} is not UTF-8"))
}

/// 앞뒤 공백을 빼고 `marker`와 같은 줄을 `text`로 바꾼다(주석 안에 적힌 이름은 그대로). 그런 줄이 없으면 `None`.
fn replace_marker_line(prelude: &str, marker: &str, text: &str) -> Option<String> {
    let mut found = false;
    let out: String = prelude
        .split_inclusive('\n')
        .map(|line| {
            if line.trim() == marker {
                found = true;
                let eol = if line.ends_with('\n') { "\n" } else { "" };
                format!("{text}{eol}")
            } else {
                line.to_owned()
            }
        })
        .collect();
    found.then_some(out)
}

fn insert_properties(prelude: &str, lang: Language, props: &[Property]) -> Result<String> {
    let decls: String = props
        .iter()
        .map(|p| {
            let n = format!("{PROPERTY_PREFIX}{}", p.name);
            match (lang, p.value) {
                (Language::Glsl, PropValue::Float(_)) => format!("float {n};"),
                (Language::Glsl, PropValue::Int(_) | PropValue::Bool(_)) => format!("int {n};"),
                (Language::Glsl, PropValue::Color(_)) => format!("vec4 {n};"),
                (Language::Wgsl, PropValue::Float(_)) => format!("{n}: f32,"),
                (Language::Wgsl, PropValue::Int(_) | PropValue::Bool(_)) => format!("{n}: i32,"),
                (Language::Wgsl, PropValue::Color(_)) => format!("{n}: vec4<f32>,"),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    match replace_marker_line(prelude, PROPERTIES_MARKER, &decls) {
        Some(out) => Ok(out),
        None if props.is_empty() => Ok(prelude.to_owned()),
        None => bail!("shader prelude has no `{PROPERTIES_MARKER}` line for wallpaper properties"),
    }
}

/// 채널 해상도 배열과 채널 텍스처·샘플러 선언을 채널 수만큼 만든다. 표시 줄이 없는 프리앰블(직접 바꾼 예전 파일)은
/// 채널이 Shadertoy 기본 수일 때만 그대로 쓴다.
fn insert_channels(prelude: &str, lang: Language, channels: usize) -> Result<String> {
    let resolution = match lang {
        Language::Glsl => format!("    vec3  iChannelResolution[{channels}];"),
        Language::Wgsl => format!("    channel_resolution: array<vec3<f32>, {channels}>,"),
    };
    let decls: Vec<String> = (0..channels)
        .flat_map(|i| {
            let (t, s) = (texture_binding(i), sampler_binding(channels, i));
            match lang {
                Language::Glsl => vec![
                    format!("layout(set = 0, binding = {t}) uniform texture2D waywall_channel{i};"),
                    format!("layout(set = 0, binding = {s}) uniform sampler waywall_sampler{i};"),
                    format!(
                        "#define iChannel{i} sampler2D(waywall_channel{i}, waywall_sampler{i})"
                    ),
                ],
                Language::Wgsl => vec![
                    format!("@group(0) @binding({t}) var channel{i}: texture_2d<f32>;"),
                    format!("@group(0) @binding({s}) var channel{i}_sampler: sampler;"),
                ],
            }
        })
        .collect();
    let has = |marker: &str| prelude.lines().any(|l| l.trim() == marker);
    match (has(CHANNEL_RESOLUTION_MARKER), has(CHANNELS_MARKER)) {
        (true, true) => {
            let out = replace_marker_line(prelude, CHANNEL_RESOLUTION_MARKER, &resolution)
                .and_then(|p| replace_marker_line(&p, CHANNELS_MARKER, &decls.join("\n")));
            out.context("shader prelude channel markers")
        }
        (false, false) if channels == SHADERTOY_CHANNELS => Ok(prelude.to_owned()),
        _ => bail!(
            "shader prelude needs both `{CHANNEL_RESOLUTION_MARKER}` and `{CHANNELS_MARKER}` lines for {channels} channels"
        ),
    }
}

/// 합친 소스의 줄 번호를 사용자 파일 기준으로 바꾼다.
struct LineMap<'a> {
    name: &'a str,
    prelude_lines: u32,
    user_lines: u32,
}

impl LineMap<'_> {
    fn line(&self, line: u32, col: u32) -> String {
        match line.checked_sub(self.prelude_lines) {
            Some(l) if l >= 1 && l <= self.user_lines => format!("{}:{l}:{col}", self.name),
            _ => format!("{} (waywall prelude/epilogue line {line})", self.name),
        }
    }

    fn describe(&self, span: Span, source: &str, msg: &str) -> String {
        if !span.is_defined() {
            return format!("{}: {msg}", self.name);
        }
        let loc = span.location(source);
        format!("{}: {msg}", self.line(loc.line_number, loc.line_position))
    }
}

// ── uniform ─────────────────────────────────────────────────────────────

/// 렌더마다 채우는 입력 값.
#[derive(Debug, Clone, Default)]
pub struct Inputs {
    pub resolution: [f32; 3],
    pub time: f32,
    pub time_delta: f32,
    pub frame: i32,
    pub frame_rate: f32,
    pub date: [f32; 4],
    pub mouse: [f32; 4],
    pub pointer: [f32; 4],
    pub click: [f32; 4],
    /// 채널 픽셀 크기 (z = 1, 없는 채널은 0). 채널 수만큼.
    pub channel_resolution: Vec<[f32; 3]>,
    /// 그리는 영역의 왼쪽 위 (렌더 타깃 px). 배경과 텍스처에 그리는 레이어는 0.
    pub origin: [f32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Resolution,
    Time,
    TimeDelta,
    Frame,
    FrameRate,
    Date,
    Mouse,
    Pointer,
    Click,
    ChannelResolution,
    Origin,
    Property(usize),
}

/// GLSL(Shadertoy) 이름과 WGSL 이름 둘 다 받는다.
fn builtin_slot(name: &str) -> Option<Slot> {
    Some(match name {
        "iResolution" | "resolution" => Slot::Resolution,
        "iTime" | "time" => Slot::Time,
        "iTimeDelta" | "time_delta" => Slot::TimeDelta,
        "iFrame" | "frame" => Slot::Frame,
        "iFrameRate" | "frame_rate" => Slot::FrameRate,
        "iDate" | "date" => Slot::Date,
        "iMouse" | "mouse" => Slot::Mouse,
        "iPointer" | "pointer" => Slot::Pointer,
        "iClick" | "click" => Slot::Click,
        "iChannelResolution" | "channel_resolution" => Slot::ChannelResolution,
        "waywall_origin" | "origin" => Slot::Origin,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scalar {
    Float,
    Int,
    Uint,
}

#[derive(Debug, Clone)]
struct Member {
    name: String,
    offset: u32,
    scalar: Scalar,
    components: u32,
    /// 배열이면 원소 수와 간격(바이트), 아니면 1과 0.
    count: u32,
    stride: u32,
}

#[derive(Debug, Clone, Default)]
pub struct UniformLayout {
    pub size: u32,
    members: Vec<Member>,
}

impl UniformLayout {
    /// group 0 binding 0의 uniform 구조체. 셰이더가 uniform을 쓰지 않아 최적화로 사라졌어도 동작하도록,
    /// 없으면 빈 레이아웃(최소 크기)으로 둔다.
    pub fn from_module(module: &Module) -> Result<Self> {
        let Some((_, var)) = module.global_variables.iter().find(|(_, v)| {
            v.space == AddressSpace::Uniform
                && v.binding
                    .as_ref()
                    .is_some_and(|b| b.group == 0 && b.binding == 0)
        }) else {
            return Ok(Self::default());
        };
        let TypeInner::Struct { members, span } = &module.types[var.ty].inner else {
            bail!("uniform at set 0 binding 0 must be a struct/block");
        };
        let mut out = Vec::new();
        for m in members {
            let (ty, count, stride) = match &module.types[m.ty].inner {
                TypeInner::Array {
                    base,
                    size: naga::ArraySize::Constant(n),
                    stride,
                } => (*base, n.get(), *stride),
                _ => (m.ty, 1, 0),
            };
            let (scalar, components) = match &module.types[ty].inner {
                TypeInner::Scalar(s) => (s.kind, 1),
                TypeInner::Vector { size, scalar } => (
                    scalar.kind,
                    match size {
                        VectorSize::Bi => 2,
                        VectorSize::Tri => 3,
                        VectorSize::Quad => 4,
                    },
                ),
                _ => continue,
            };
            let scalar = match scalar {
                ScalarKind::Float => Scalar::Float,
                ScalarKind::Sint => Scalar::Int,
                ScalarKind::Uint | ScalarKind::Bool => Scalar::Uint,
                _ => continue,
            };
            if let Some(name) = &m.name {
                out.push(Member {
                    name: name.clone(),
                    offset: m.offset,
                    scalar,
                    components,
                    count,
                    stride,
                });
            }
        }
        Ok(Self {
            size: *span,
            members: out,
        })
    }

    /// 버퍼 크기. wgpu는 0 크기 버퍼에 바인딩할 수 없고 uniform은 16바이트 단위다.
    pub fn buffer_size(&self) -> u64 {
        u64::from(self.size.max(1)).next_multiple_of(16)
    }

    /// 멤버 이름 (선언 순서).
    pub fn member_names(&self) -> impl Iterator<Item = &str> {
        self.members.iter().map(|m| m.name.as_str())
    }

    /// 멤버 이름으로 값(성분을 이어 붙인 f32)을 찾아 쓴다. 효과 파일처럼 이름이 정해지지 않은 uniform용.
    pub fn write_named<'a>(&self, value: impl Fn(&str) -> Option<&'a [f32]>) -> Vec<u8> {
        let mut buf = vec![0u8; self.buffer_size() as usize];
        for m in &self.members {
            let Some(values) = value(&m.name) else {
                continue;
            };
            let per = m.components as usize;
            for k in 0..m.count as usize {
                for i in 0..per {
                    let Some(v) = values.get(k * per + i) else {
                        continue;
                    };
                    let at = m.offset as usize + k * m.stride as usize + i * 4;
                    let bytes = match m.scalar {
                        Scalar::Float => v.to_ne_bytes(),
                        Scalar::Int => (*v as i32).to_ne_bytes(),
                        Scalar::Uint => (v.max(0.0) as u32).to_ne_bytes(),
                    };
                    if let Some(dst) = buf.get_mut(at..at + 4) {
                        dst.copy_from_slice(&bytes);
                    }
                }
            }
        }
        buf
    }

    /// 멤버마다 채울 값. 속성 목록은 렌더러가 사는 동안 바뀌지 않으므로 한 번만 찾는다.
    pub fn slots(&self, props: &[Property]) -> Slots {
        Slots(
            self.members
                .iter()
                .map(|m| {
                    builtin_slot(&m.name).or_else(|| {
                        let name = m.name.strip_prefix(PROPERTY_PREFIX)?;
                        props
                            .iter()
                            .position(|p| p.name == name)
                            .map(Slot::Property)
                    })
                })
                .collect(),
        )
    }

    pub fn write(&self, slots: &Slots, inputs: &Inputs, props: &[Property]) -> Vec<u8> {
        let mut buf = vec![0u8; self.buffer_size() as usize];
        for (m, slot) in self.members.iter().zip(&slots.0) {
            let Some(slot) = *slot else { continue };
            // 값은 배열 원소마다 slot_components(slot)개씩 이어져 있다.
            let per = slot_components(slot, props);
            for k in 0..m.count as usize {
                for i in 0..(m.components as usize).min(per) {
                    let Some(v) = slot_value(slot, inputs, props, k * per + i) else {
                        continue;
                    };
                    let at = m.offset as usize + k * m.stride as usize + i * 4;
                    let bytes = match m.scalar {
                        Scalar::Float => v.to_f32().to_ne_bytes(),
                        Scalar::Int => v.to_i32().to_ne_bytes(),
                        Scalar::Uint => (v.to_i32().max(0) as u32).to_ne_bytes(),
                    };
                    if let Some(dst) = buf.get_mut(at..at + 4) {
                        dst.copy_from_slice(&bytes);
                    }
                }
            }
        }
        buf
    }
}

/// `UniformLayout::slots`의 결과 (멤버 순서).
#[derive(Debug, Clone, Default)]
pub struct Slots(Vec<Option<Slot>>);

#[derive(Debug, Clone, Copy)]
enum Num {
    F(f32),
    I(i32),
}

impl Num {
    fn to_f32(self) -> f32 {
        match self {
            Num::F(f) => f,
            Num::I(i) => i as f32,
        }
    }
    fn to_i32(self) -> i32 {
        match self {
            Num::F(f) => f as i32,
            Num::I(i) => i,
        }
    }
}

/// 슬롯 값의 원소 하나당 성분 수 (배열이면 원소마다 이만큼씩 이어짐).
fn slot_components(slot: Slot, props: &[Property]) -> usize {
    match slot {
        Slot::Time | Slot::TimeDelta | Slot::Frame | Slot::FrameRate => 1,
        Slot::Origin => 2,
        Slot::Resolution | Slot::ChannelResolution => 3,
        Slot::Date | Slot::Mouse | Slot::Pointer | Slot::Click => 4,
        Slot::Property(i) => match props[i].value {
            PropValue::Color(_) => 4,
            _ => 1,
        },
    }
}

/// 슬롯 값을 이어 붙인 것의 `i`번째.
fn slot_value(slot: Slot, x: &Inputs, props: &[Property], i: usize) -> Option<Num> {
    let f = |v: &[f32]| v.get(i).copied().map(Num::F);
    match slot {
        Slot::Resolution => f(&x.resolution),
        Slot::Time => f(&[x.time]),
        Slot::TimeDelta => f(&[x.time_delta]),
        Slot::Frame => (i == 0).then_some(Num::I(x.frame)),
        Slot::FrameRate => f(&[x.frame_rate]),
        Slot::Date => f(&x.date),
        Slot::Mouse => f(&x.mouse),
        Slot::Pointer => f(&x.pointer),
        Slot::Click => f(&x.click),
        Slot::ChannelResolution => f(x.channel_resolution.as_flattened()),
        Slot::Origin => f(&x.origin),
        Slot::Property(p) => match props[p].value {
            PropValue::Float(v) => f(&[v]),
            PropValue::Int(v) => (i == 0).then_some(Num::I(v)),
            PropValue::Bool(v) => (i == 0).then_some(Num::I(i32::from(v))),
            PropValue::Color(c) => {
                f(&[c.r, c.g, c.b, c.a].map(|v| f32::from(v) / f32::from(u8::MAX)))
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::properties::PropValue;

    fn assets() -> Assets {
        Assets::new(std::path::PathBuf::from("/nonexistent"))
    }

    fn examples() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/shaders")
    }

    /// 예제 셰이더는 전부 컴파일되어야 한다 (docs/development.md#테스트-방침).
    #[test]
    fn examples_compile() {
        let mut n = 0;
        let layers = crate::config::Layers::load_for_test("");
        let detect = layers.global().unwrap().detect;
        for entry in std::fs::read_dir(examples()).unwrap() {
            let path = entry.unwrap().path();
            let wp = crate::wallpaper::resolve(
                path.to_str().unwrap(),
                &detect,
                std::path::Path::new("/"),
            )
            .unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
            let crate::wallpaper::Source::File(path) = wp.source else {
                panic!("{}", path.display());
            };
            // 디렉터리형이면 채널 이미지까지 읽혀야 한다.
            let opts: waywall_proto::config::ShaderOptions = layers
                .type_options(wp.kind, wp.options.as_ref(), &layers.output("X").unwrap())
                .unwrap();
            let channels =
                crate::backend::shader::parse_inputs(wp.inputs.as_ref(), &opts.channel, None)
                    .unwrap();
            for c in channels.iter().flatten() {
                let file = path.parent().unwrap().join(&c.source);
                crate::backend::shader::decode_channel(&file, Language::from_path(&path))
                    .unwrap_or_else(|e| panic!("{}: {e:#}", file.display()));
            }
            let src = std::fs::read_to_string(&path).unwrap();
            let c = compile(
                &assets(),
                Language::from_path(&path),
                &src,
                &[],
                channels.len(),
                "x",
            )
            .unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
            assert!(c.layout.size > 0, "{}", path.display());
            n += 1;
        }
        assert!(n >= 2);
    }

    #[test]
    fn layout_follows_prelude() {
        let props = [
            Property {
                name: "density".into(),
                value: PropValue::Float(2.5),
            },
            Property {
                name: "tint".into(),
                value: PropValue::Color("#ff000080".parse().unwrap()),
            },
        ];
        for (lang, src) in [
            (
                Language::Glsl,
                "void mainImage(out vec4 c, in vec2 p) { c = u_tint * u_density + iMouse + vec4(iResolution, iTime) + vec4(iChannelResolution[1], 0.0); }",
            ),
            (
                Language::Wgsl,
                "@fragment fn main_image(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> { return g.u_tint * g.u_density + g.mouse + vec4<f32>(g.resolution, g.time) + vec4<f32>(g.channel_resolution[1], 0.0); }",
            ),
        ] {
            let c = compile(&assets(), lang, src, &props, SHADERTOY_CHANNELS, "t").unwrap();
            let inputs = Inputs {
                resolution: [1920.0, 1080.0, 1.0],
                time: 3.0,
                channel_resolution: vec![[0.0; 3], [640.0, 360.0, 1.0], [0.0; 3], [7.0, 8.0, 1.0]],
                ..Default::default()
            };
            let buf = c.layout.write(&c.layout.slots(&props), &inputs, &props);
            let at = |name: &str| {
                c.layout
                    .members
                    .iter()
                    .find(|m| m.name == name)
                    .unwrap_or_else(|| panic!("{lang:?}: no member {name}"))
                    .offset as usize
            };
            let f = |off: usize| f32::from_ne_bytes(buf[off..off + 4].try_into().unwrap());
            let res = if lang == Language::Glsl {
                "iResolution"
            } else {
                "resolution"
            };
            let time = if lang == Language::Glsl {
                "iTime"
            } else {
                "time"
            };
            assert_eq!(f(at(res)), 1920.0, "{lang:?}");
            assert_eq!(f(at(res) + 4), 1080.0);
            assert_eq!(f(at(time)), 3.0);
            let ch = if lang == Language::Glsl {
                "iChannelResolution"
            } else {
                "channel_resolution"
            };
            // vec3 배열의 원소 간격은 16바이트.
            assert_eq!(f(at(ch) + 16), 640.0, "{lang:?}");
            assert_eq!(f(at(ch) + 20), 360.0);
            assert_eq!(f(at(ch) + 48), 7.0);
            assert_eq!(f(at("u_density")), 2.5);
            assert_eq!(f(at("u_tint")), 1.0);
            assert!((f(at("u_tint") + 12) - 128.0 / 255.0).abs() < 1e-6);
        }
    }

    /// 4개를 넘는 채널: 선언·바인딩·해상도 배열이 채널 수만큼 생기고, 0–3은 Shadertoy와 같다
    /// (docs/wallpaper-format.md#텍스처-입력-ichanneln).
    #[test]
    fn more_than_four_channels() {
        let n = 9;
        for (lang, src) in [
            (
                Language::Glsl,
                "void mainImage(out vec4 c, in vec2 p) { c = texture(iChannel8, p) + texture(iChannel0, p) + vec4(iChannelResolution[8], 0.0); }",
            ),
            (
                Language::Wgsl,
                "@fragment fn main_image(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> { return textureSample(channel8, channel8_sampler, p.xy) + textureSample(channel0, channel0_sampler, p.xy) + vec4<f32>(g.channel_resolution[8], 0.0); }",
            ),
        ] {
            let c = compile(&assets(), lang, src, &[], n, "t")
                .unwrap_or_else(|e| panic!("{lang:?}: {e:#}"));
            // 텍스처는 1..=n, 샘플러는 n+1..=2n.
            let mut bindings: Vec<u32> = c
                .module
                .global_variables
                .iter()
                .filter_map(|(_, v)| v.binding.as_ref())
                .filter(|b| b.group == 0 && b.binding > 0)
                .map(|b| b.binding)
                .collect();
            bindings.sort_unstable();
            assert_eq!(bindings.first(), Some(&texture_binding(0)), "{lang:?}");
            assert_eq!(
                bindings.last(),
                Some(&sampler_binding(n, n - 1)),
                "{lang:?}"
            );
            let mut res = vec![[0.0; 3]; n];
            res[8] = [5.0, 6.0, 1.0];
            let inputs = Inputs {
                channel_resolution: res,
                ..Default::default()
            };
            let buf = c.layout.write(&c.layout.slots(&[]), &inputs, &[]);
            let ch = c
                .layout
                .members
                .iter()
                .find(|m| m.name == "iChannelResolution" || m.name == "channel_resolution")
                .unwrap();
            assert_eq!(ch.count, n as u32, "{lang:?}");
            let at = ch.offset as usize + 8 * ch.stride as usize;
            assert_eq!(f32::from_ne_bytes(buf[at..at + 4].try_into().unwrap()), 5.0);
        }
        // 표시 줄이 없는 예전 프리앰블은 기본 4채널일 때만 받는다.
        let old = "#version 450\nlayout(location = 0) out vec4 waywall_fragColor;\n";
        assert!(insert_channels(old, Language::Glsl, SHADERTOY_CHANNELS).is_ok());
        let e = insert_channels(old, Language::Glsl, 5).unwrap_err();
        assert!(format!("{e:#}").contains("waywall:channels"), "{e:#}");
        // 주석 안에 적힌 표시 이름은 바뀌지 않는다.
        let p =
            "// see `// waywall:channels`\n// waywall:channel_resolution\n// waywall:channels\n";
        let out = insert_channels(p, Language::Glsl, 4).unwrap();
        assert!(out.starts_with("// see `// waywall:channels`\n"), "{out}");
    }

    #[test]
    fn errors_point_at_user_lines() {
        let src = "void mainImage(out vec4 c, in vec2 p) {\n    c = vec4(1.0);\n    nope();\n}\n";
        let e = compile(
            &assets(),
            Language::Glsl,
            src,
            &[],
            SHADERTOY_CHANNELS,
            "bad.glsl",
        )
        .unwrap_err();
        assert!(format!("{e:#}").contains("bad.glsl:3:"), "{e:#}");
        let e = compile(
            &assets(),
            Language::Glsl,
            "void foo() {}",
            &[],
            SHADERTOY_CHANNELS,
            "a.glsl",
        )
        .unwrap_err();
        assert!(format!("{e:#}").contains("a.glsl"), "{e:#}");
    }
}
