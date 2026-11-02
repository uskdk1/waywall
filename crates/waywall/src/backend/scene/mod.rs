//! 씬 배경 (docs/wallpaper-format.md#씬-배경, docs/architecture.md#씬-렌더러).
//!
//! `load`가 `scene.toml`을 검사하고 텍스처를 디코딩해 `SceneSpec`을 만들고(GPU 불필요),
//! `render`가 출력마다 GPU 자원을 만들어 그린다. 파티클의 GPU 쪽은 `particles`.

mod bloom;
mod load;
mod particles;
mod render;
mod shader_layer;

pub use load::load;
pub use render::{SceneRenderer, upload_texture};
use waywall_proto::Color;
use waywall_proto::config::{
    Blend, Bloom, ChannelDefaults, EmitterShape, Fit, ParticlePointer, Turbulence, VelocityRange,
};

use crate::backend::shader::CHANNELS;
use crate::gpu::shadertoy::{Compiled, Language};
use crate::properties::Property;

/// 검사를 마친 씬. 출력 하나의 `SceneRenderer`가 소비한다.
pub struct SceneSpec {
    /// 설계 해상도 `[w, h]`.
    pub size: [f64; 2],
    pub fit: Fit,
    pub clear: Color,
    pub exposure: f64,
    pub bloom: Bloom,
    pub vignette: f64,
    /// 블룸 축소·확대 셰이더. `bloom.intensity`가 0이면 `None`.
    pub bloom_shaders: Option<(naga::Module, naga::Module)>,
    pub pointer_smoothing: f64,
    pub settle_px: f64,
    /// 아래에서 위 순서. 보이는 레이어와, 보이지 않지만 shader 레이어의 입력인 레이어.
    pub layers: Vec<Layer>,
    /// `layers`와 같은 순서로, 화면에 그리는지 (`visible`).
    pub drawn: Vec<bool>,
    pub lights: Vec<Light>,
    pub image_shader: naga::Module,
    pub post_shader: naga::Module,
    pub simulate_shader: naga::Module,
    /// shader 레이어 채널 입력 잘라 내기 (`scene/input.wgsl`).
    pub input_shader: naga::Module,
}

pub enum Layer {
    Image(ImageLayer),
    Particles(Box<ParticleLayer>),
    Shader(Box<ShaderLayer>),
}

impl Layer {
    pub fn id(&self) -> &str {
        match self {
            Layer::Image(l) => &l.id,
            Layer::Particles(l) => &l.id,
            Layer::Shader(l) => &l.id,
        }
    }

    pub fn texture(&self) -> Option<&Texture> {
        match self {
            Layer::Image(l) => Some(&l.texture),
            Layer::Particles(l) => l.texture.as_ref(),
            Layer::Shader(_) => None,
        }
    }
}

pub struct ImageLayer {
    pub id: String,
    pub blend: Blend,
    pub opacity: f64,
    pub parallax: f64,
    /// 중심 좌표 (설계 px).
    pub position: [f64; 2],
    pub scale: [f64; 2],
    /// 도, 시계 방향.
    pub rotation: f64,
    pub texture: Texture,
    pub lit: bool,
    /// 값 그대로의 RGBA8 (OpenGL 규약 탄젠트 공간 노멀).
    pub normal_map: Option<Texture>,
}

/// 필드 설명은 docs/wallpaper-format.md#레이어-타입 (particles).
pub struct ParticleLayer {
    pub id: String,
    pub blend: Blend,
    pub opacity: f64,
    pub parallax: f64,
    pub max: u32,
    /// `None`이면 `shader`의 `sprite()`가 무늬.
    pub texture: Option<Texture>,
    /// `particles/render.wgsl` + 스프라이트.
    pub shader: naga::Module,
    pub shape: EmitterShape,
    /// 방출기 중심 (설계 px).
    pub center: [f64; 2],
    /// box의 반폭·반높이, circle의 `[반지름, 반지름]`, point는 0.
    pub extent: [f64; 2],
    pub rate: f64,
    pub burst: u32,
    pub lifetime: [f64; 2],
    pub velocity: VelocityRange,
    pub gravity: [f64; 2],
    pub drag: f64,
    pub turbulence: Turbulence,
    pub size: [f64; 2],
    pub color: [Color; 2],
    pub emissive: f64,
    pub pointer: ParticlePointer,
    pub max_step: f64,
}

/// 필드 설명은 docs/wallpaper-format.md#레이어-타입 (shader).
pub struct ShaderLayer {
    pub id: String,
    pub blend: Blend,
    pub opacity: f64,
    pub parallax: f64,
    /// `[x, y, 폭, 높이]` (설계 px).
    pub rect: [f64; 4],
    pub compiled: Compiled,
    pub props: Vec<Property>,
    pub lang: Language,
    pub inputs: [Option<ChannelInput>; CHANNELS],
    pub sampling: [ChannelDefaults; CHANNELS],
    /// 그리는 해상도 배율.
    pub render_scale: f64,
    /// HDR 버퍼에 직접 그린다: `opacity = 1`, `render_scale = 1`이고 다른 레이어의 입력이 아님 (docs/architecture.md#씬-렌더러).
    pub direct: bool,
}

/// 색은 선형 RGB × intensity. 좌표는 설계 px (docs/wallpaper-format.md#조명).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Light {
    Ambient {
        color: [f64; 3],
    },
    Point {
        /// `None`이면 포인터를 따라감.
        position: Option<[f64; 2]>,
        color: [f64; 3],
        radius: f64,
        height: f64,
        flicker: f64,
        flicker_speed: f64,
    },
    Directional {
        direction: [f64; 3],
        color: [f64; 3],
    },
}

pub enum ChannelInput {
    /// 이 레이어 아래에 지금까지 그려진 장면.
    Below,
    /// `SceneSpec::layers`의 그 레이어 하나만 그린 결과 (이 레이어보다 아래).
    Layer(usize),
    /// premultiplied sRGB, GLSL이면 이미 상하 반전.
    Image(Texture),
}

/// 알파를 선형 공간에서 곱해 둔 sRGB RGBA8.
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}
