//! 셰이더 배경 소스 → naga 모듈 (docs/wallpaper-format.md#셰이더-배경).
//!
//! GLSL: `prelude.glsl` + 사용자 소스 + `epilogue.glsl`, WGSL: `prelude.wgsl` + 사용자 소스.
//! uniform 블록의 레이아웃은 코드에 고정하지 않고, 파싱한 모듈에서 멤버 이름과 offset을 읽어 채운다.
//! 그래서 프리앰블 asset을 바꿔도(멤버 순서·패딩) CPU 쪽이 따라간다.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use naga::{AddressSpace, Module, ScalarKind, ShaderStage, Span, TypeInner, VectorSize};

use crate::assets::Assets;
use crate::properties::{PropValue, Property};

const GLSL_PRELUDE: &str = "shaders/shadertoy/prelude.glsl";
const GLSL_EPILOGUE: &str = "shaders/shadertoy/epilogue.glsl";
const WGSL_PRELUDE: &str = "shaders/shadertoy/prelude.wgsl";
/// 프리앰블에서 속성 선언으로 바뀌는 줄.
const PROPERTIES_MARKER: &str = "// waywall:properties";
const PROPERTY_PREFIX: &str = "u_";

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

/// `name`: 오류 메시지에 쓸 사용자 파일 이름.
pub fn compile(
    assets: &Assets,
    lang: Language,
    user: &str,
    props: &[Property],
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

fn insert_properties(prelude: &str, lang: Language, props: &[Property]) -> Result<String> {
    if !prelude.contains(PROPERTIES_MARKER) {
        if props.is_empty() {
            return Ok(prelude.to_owned());
        }
        bail!("shader prelude has no `{PROPERTIES_MARKER}` line for wallpaper properties");
    }
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
    Ok(prelude.replace(PROPERTIES_MARKER, &decls))
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
    /// 채널 픽셀 크기 (z = 1, 없는 채널은 0).
    pub channel_resolution: [[f32; 3]; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
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
    fn from_module(module: &Module) -> Result<Self> {
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

    pub fn write(&self, inputs: &Inputs, props: &[Property]) -> Vec<u8> {
        let index: HashMap<&str, usize> = props
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name.as_str(), i))
            .collect();
        let mut buf = vec![0u8; self.buffer_size() as usize];
        for m in &self.members {
            let slot = builtin_slot(&m.name).or_else(|| {
                m.name
                    .strip_prefix(PROPERTY_PREFIX)
                    .and_then(|n| index.get(n))
                    .map(|&i| Slot::Property(i))
            });
            let Some(slot) = slot else { continue };
            let values = slot_values(slot, inputs, props);
            // 값은 원소마다 slot_components개씩 이어져 있다.
            let per = values.len() / m.count.max(1) as usize;
            let elems =
                (0..m.count as usize).flat_map(|k| (0..m.components as usize).map(move |i| (k, i)));
            for (k, i) in elems {
                let Some(v) = values.get(k * per + i) else {
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
        buf
    }
}

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

fn slot_values(slot: Slot, x: &Inputs, props: &[Property]) -> Vec<Num> {
    let f = |v: &[f32]| v.iter().map(|&f| Num::F(f)).collect();
    match slot {
        Slot::Resolution => f(&x.resolution),
        Slot::Time => f(&[x.time]),
        Slot::TimeDelta => f(&[x.time_delta]),
        Slot::Frame => vec![Num::I(x.frame)],
        Slot::FrameRate => f(&[x.frame_rate]),
        Slot::Date => f(&x.date),
        Slot::Mouse => f(&x.mouse),
        Slot::Pointer => f(&x.pointer),
        Slot::Click => f(&x.click),
        Slot::ChannelResolution => f(x.channel_resolution.as_flattened()),
        Slot::Property(i) => match props[i].value {
            PropValue::Float(v) => vec![Num::F(v)],
            PropValue::Int(v) => vec![Num::I(v)],
            PropValue::Bool(v) => vec![Num::I(i32::from(v))],
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
            let c = compile(&assets(), Language::from_path(&path), &src, &[], "x")
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
            let c = compile(&assets(), lang, src, &props, "t").unwrap();
            let inputs = Inputs {
                resolution: [1920.0, 1080.0, 1.0],
                time: 3.0,
                channel_resolution: [[0.0; 3], [640.0, 360.0, 1.0], [0.0; 3], [7.0, 8.0, 1.0]],
                ..Default::default()
            };
            let buf = c.layout.write(&inputs, &props);
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

    #[test]
    fn errors_point_at_user_lines() {
        let src = "void mainImage(out vec4 c, in vec2 p) {\n    c = vec4(1.0);\n    nope();\n}\n";
        let e = compile(&assets(), Language::Glsl, src, &[], "bad.glsl").unwrap_err();
        assert!(format!("{e:#}").contains("bad.glsl:3:"), "{e:#}");
        let e = compile(&assets(), Language::Glsl, "void foo() {}", &[], "a.glsl").unwrap_err();
        assert!(format!("{e:#}").contains("a.glsl"), "{e:#}");
    }
}
