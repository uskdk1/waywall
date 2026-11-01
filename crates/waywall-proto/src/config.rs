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
    pub tonemap: String,
    pub hdr_format: String,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneOptions {
    pub fps: u32,
    pub render_scale: f64,
    pub interactive: bool,
    pub fit: Fit,
    pub clear: Color,
    pub post: ScenePost,
    pub layer: SceneLayerDefaults,
    pub particles: SceneParticleDefaults,
    pub light: SceneLightDefaults,
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
pub struct SceneParticleDefaults {
    pub sprite: String,
    pub blend: Blend,
    pub drag: f64,
    pub emissive: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneLightDefaults {
    pub point: PointLightDefaults,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointLightDefaults {
    pub radius: f64,
    pub height: f64,
    pub intensity: f64,
    pub flicker: f64,
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
