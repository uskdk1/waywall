//! 설정 스키마.
//!
//! 모든 구조체는 `deny_unknown_fields`이고 serde 기본값을 쓰지 않는다. 값은 항상
//! `assets/defaults.toml`과 사용자 설정을 병합한 결과에서 오므로, 키가 하나라도 빠지면
//! 역직렬화가 실패한다 (docs/config.md).

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::color::Color;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub volume: u8,
    pub mute: bool,
    pub pause_when: PauseWhen,
    pub log: Log,
    pub surface: SurfaceConfig,
    pub render: Render,
    pub visibility: Visibility,
    pub cursor: Cursor,
    pub appearance: Appearance,
    pub assets: AssetsConfig,
    pub detect: Detect,
    pub types: Types,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PauseWhen {
    Never,
    Fullscreen,
    Maximized,
    AnyWindow,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Log {
    /// `RUST_LOG`이 없을 때 쓰는 tracing 필터.
    pub level: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceConfig {
    pub layer: LayerKind,
    pub namespace: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LayerKind {
    Background,
    Bottom,
    Top,
    Overlay,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Render {
    pub gpu: GpuPreference,
    pub max_fps: u32,
    pub render_scale: f64,
    pub present_mode: PresentMode,
    pub frame_latency: u32,
    pub hdr_format: HdrFormat,
}

/// 씬 내부 HDR 버퍼 포맷.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HdrFormat {
    Rgba16float,
    Rg11b10ufloat,
    Rgba32float,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PresentMode {
    Mailbox,
    Fifo,
    Immediate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GpuPreference {
    LowPower,
    HighPerformance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Visibility {
    pub frame_timeout_ms: u64,
    pub slow_frame_count: u32,
    pub foreign_toplevel: bool,
    pub niri: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    /// `wp_cursor_shape_v1` / CSS 커서 이름, 또는 `hidden`.
    pub shape: String,
    /// 빈 문자열이면 `XCURSOR_THEME` 환경변수.
    pub theme: String,
    pub size: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Appearance {
    pub empty: Fill,
    pub error: FillOrPrevious,
    pub loading: FillOrPrevious,
}

/// 색(`#…`) 또는 배경 경로.
#[derive(Debug, Clone, PartialEq)]
pub enum Fill {
    Color(Color),
    Wallpaper(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FillOrPrevious {
    Previous,
    Fill(Fill),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetsConfig {
    pub dir: String,
}

/// 파일 확장자(소문자, 점 없이) → 배경 타입.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Detect {
    pub image: Vec<String>,
    pub video: Vec<String>,
    pub shader: Vec<String>,
    pub web: Vec<String>,
    /// 이 접두사로 시작하는 경로는 web 타입.
    pub url_schemes: Vec<String>,
    /// 디렉터리형 배경의 프로젝트 파일 이름.
    pub project_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Types {
    pub image: ImageOptions,
    pub video: VideoOptions,
    pub shader: ShaderOptions,
    pub scene: SceneOptions,
    pub web: WebOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageOptions {
    pub fit: Fit,
    pub background: Color,
    pub interactive: bool,
    /// 출력보다 큰 이미지를 메모리에 올리기 전에 줄일지.
    pub downscale: bool,
    pub downscale_filter: ResizeFilter,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VideoOptions {
    pub fit: Fit,
    pub background: Color,
    pub interactive: bool,
    #[serde(rename = "loop")]
    pub looping: bool,
    pub audio: bool,
    pub speed: f64,
    pub hwdec: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShaderOptions {
    pub fps: u32,
    pub render_scale: f64,
    pub interactive: bool,
    /// 첫 프레임 전에 보이는 부모 surface 색.
    pub background: Color,
    /// `[inputs]` 채널에서 생략한 값.
    pub channel: ChannelDefaults,
}

/// 텍스처 채널의 샘플링 (docs/wallpaper-format.md#텍스처-입력-ichannel0ichannel3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelDefaults {
    pub filter: ChannelFilter,
    pub wrap: ChannelWrap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChannelFilter {
    Linear,
    Nearest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChannelWrap {
    Repeat,
    Clamp,
    Mirror,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneOptions {
    pub fps: u32,
    pub render_scale: f64,
    pub interactive: bool,
    pub fit: Fit,
    pub clear: Color,
    /// 패럴랙스가 포인터를 따라가는 시간 상수(초).
    pub pointer_smoothing: f64,
    /// 목표와의 차이가 이보다 작으면(출력 px) 움직임을 멈춘다.
    pub settle_px: f64,
    pub post: ScenePost,
    /// 레이어 필드의 내장값/사용자 값. 레이어마다 병합하는 쪽은 데몬의 씬 로더.
    pub layer: SceneLayerDefaults,
    pub image: SceneImageDefaults,
    pub shader: SceneShaderDefaults,
    pub particles: SceneParticleDefaults,
    pub light: SceneLightDefaults,
    /// 효과 타입마다 생략한 필드 (`[types.scene.effects.<타입>]`). 효과 파일을 새로 둘 수 있어 표 그대로.
    pub effects: toml::Table,
    pub follow: FollowDefaults,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FollowDefaults {
    pub range: f64,
    pub smoothing: f64,
    pub on_leave: OnLeave,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnLeave {
    Center,
    Stay,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenePost {
    pub exposure: f64,
    pub tonemap: String,
    pub bloom: Bloom,
    pub vignette: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bloom {
    pub threshold: f64,
    pub intensity: f64,
    pub radius: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneLayerDefaults {
    pub blend: Blend,
    pub opacity: f64,
    pub parallax: f64,
    pub visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneImageDefaults {
    pub scale: Scale,
    pub rotation: f64,
    pub lit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneShaderDefaults {
    pub channel: ChannelDefaults,
    pub render_scale: f64,
}

/// 숫자 하나(가로세로 같음) 또는 `[x, y]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Scale {
    Uniform(f64),
    Xy([f64; 2]),
}

impl Scale {
    pub fn xy(self) -> [f64; 2] {
        match self {
            Scale::Uniform(s) => [s, s],
            Scale::Xy(xy) => xy,
        }
    }
}

/// particles 레이어 필드의 내장값/사용자 값 (docs/wallpaper-format.md#레이어-타입).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneParticleDefaults {
    pub max: u32,
    /// 무늬를 정하지 않은 레이어의 스프라이트 (assets 기준 경로).
    pub sprite: String,
    pub blend: Blend,
    pub emitter: Emitter,
    pub lifetime: Span,
    pub velocity: VelocityRange,
    pub gravity: [f64; 2],
    pub drag: f64,
    pub turbulence: Turbulence,
    pub size: Ramp<f64>,
    pub color: Ramp<Color>,
    pub emissive: f64,
    pub pointer: ParticlePointer,
    pub max_step: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Emitter {
    pub shape: EmitterShape,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    pub rate: f64,
    pub burst: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EmitterShape {
    Point,
    Box,
    Circle,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VelocityRange {
    pub min: [f64; 2],
    pub max: [f64; 2],
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Turbulence {
    pub strength: f64,
    pub scale: f64,
    pub speed: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParticlePointer {
    pub mode: PointerMode,
    pub radius: f64,
    pub strength: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PointerMode {
    None,
    Attract,
    Repel,
}

/// 숫자 하나 또는 `[최소, 최대]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Span {
    One(f64),
    Range([f64; 2]),
}

impl Span {
    pub fn range(self) -> [f64; 2] {
        match self {
            Span::One(v) => [v, v],
            Span::Range(r) => r,
        }
    }
}

/// 값 하나 또는 수명에 따라 보간하는 `{ start, end }`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Ramp<T> {
    Const(T),
    Range { start: T, end: T },
}

impl<T: Copy> Ramp<T> {
    pub fn ends(self) -> [T; 2] {
        match self {
            Ramp::Const(v) => [v, v],
            Ramp::Range { start, end } => [start, end],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneLightDefaults {
    pub ambient: AmbientLightDefaults,
    pub point: PointLightDefaults,
    pub directional: DirectionalLightDefaults,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AmbientLightDefaults {
    pub color: Color,
    pub intensity: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectionalLightDefaults {
    pub color: Color,
    pub intensity: f64,
    pub direction: [f64; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointLightDefaults {
    pub color: Color,
    pub intensity: f64,
    pub radius: f64,
    pub height: f64,
    pub flicker: f64,
    pub flicker_speed: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebOptions {
    pub background: Color,
    pub interactive: bool,
    pub audio: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Fit {
    Cover,
    Contain,
    Fill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Blend {
    Normal,
    Add,
    Multiply,
    Screen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResizeFilter {
    Nearest,
    Triangle,
    CatmullRom,
    Gaussian,
    Lanczos3,
}

const PREVIOUS: &str = "previous";

impl fmt::Display for Fill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fill::Color(c) => c.fmt(f),
            Fill::Wallpaper(p) => f.write_str(p),
        }
    }
}

impl fmt::Display for FillOrPrevious {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FillOrPrevious::Previous => f.write_str(PREVIOUS),
            FillOrPrevious::Fill(fill) => fill.fmt(f),
        }
    }
}

impl Serialize for Fill {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Fill {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s.starts_with('#') {
            return s
                .parse::<Color>()
                .map(Fill::Color)
                .map_err(serde::de::Error::custom);
        }
        Ok(Fill::Wallpaper(s))
    }
}

impl Serialize for FillOrPrevious {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for FillOrPrevious {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s == PREVIOUS {
            return Ok(FillOrPrevious::Previous);
        }
        Fill::deserialize(serde::de::value::StrDeserializer::<D::Error>::new(&s))
            .map(FillOrPrevious::Fill)
    }
}
