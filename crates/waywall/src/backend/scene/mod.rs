//! 씬 배경 (docs/wallpaper-format.md#씬-배경, docs/architecture.md#씬-렌더러).
//!
//! `load`가 `scene.toml`을 검사하고 텍스처를 디코딩해 `SceneSpec`을 만들고(GPU 불필요),
//! `render`가 출력마다 GPU 자원을 만들어 그린다. 파티클의 GPU 쪽은 `particles`.

use std::collections::HashMap;

mod bloom;
mod effects;
mod live2d_layer;
mod load;
mod particles;
mod render;
mod shader_layer;

pub use load::load;
pub use render::{SceneRenderer, upload_texture};
use waywall_proto::Color;
use waywall_proto::config::{
    Animate, Blend, Bloom, ChannelDefaults, EmitterShape, Fit, Live2dBlink, Live2dBreathParam,
    OnLeave, ParticlePointer, Turbulence, VelocityRange,
};

use crate::gpu::shadertoy::{Compiled, Language, UniformLayout};
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
    /// shader 효과 결과를 마스크로 섞는 패스 (`scene/effect_mask.wgsl`). shader 효과가 있을 때만.
    pub effect_mask_shader: Option<naga::Module>,
}

pub enum Layer {
    Image(Box<ImageLayer>),
    Particles(Box<ParticleLayer>),
    Shader(Box<ShaderLayer>),
    Live2d(Box<Live2dLayer>),
}

impl Layer {
    pub fn id(&self) -> &str {
        match self {
            Layer::Image(l) => &l.id,
            Layer::Particles(l) => &l.id,
            Layer::Shader(l) => &l.id,
            Layer::Live2d(l) => &l.id,
        }
    }

    pub fn texture(&self) -> Option<&Texture> {
        match self {
            Layer::Image(l) => Some(&l.texture),
            Layer::Particles(l) => l.texture.as_ref(),
            Layer::Shader(_) | Layer::Live2d(_) => None,
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
    /// `layers`의 인덱스 (이 레이어보다 아래).
    pub clip: Option<usize>,
    pub follow: Option<Follow>,
    pub effects: Vec<EffectSpec>,
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
    /// 채널마다 (길이 = 채널 수).
    pub inputs: Vec<Option<ChannelInput>>,
    pub sampling: Vec<ChannelDefaults>,
    /// 그리는 해상도 배율.
    pub render_scale: f64,
    pub animate: Animate,
    pub clip: Option<usize>,
    pub follow: Option<Follow>,
    /// HDR 버퍼에 직접 그린다: `opacity = 1`, `render_scale = 1`이고 다른 레이어의 입력이 아님 (docs/architecture.md#씬-렌더러).
    pub direct: bool,
}

/// 필드 설명은 docs/wallpaper-format.md#live2d.
pub struct Live2dLayer {
    pub id: String,
    pub blend: Blend,
    pub opacity: f64,
    pub parallax: f64,
    /// `[x, y, 폭, 높이]` (설계 px).
    pub rect: [f64; 4],
    /// 모델 캔버스 중심의 위치 (rect 안, 설계 px).
    pub position: [f64; 2],
    /// 캔버스 px → 설계 px.
    pub scale: f64,
    pub render_scale: f64,
    pub clip: Option<usize>,
    /// `.moc3` 내용 (출력마다 모델 인스턴스를 만든다).
    pub moc: Vec<u8>,
    /// `model3.json` 순서의 텍스처.
    pub textures: Vec<Texture>,
    /// 읽어 둔 모션, `idle`은 그중 대기 모션 번호.
    pub clips: Vec<crate::live2d::Clip>,
    pub idle: Vec<usize>,
    /// 눈 깜빡임 파라미터 (`model3.json` Groups `EyeBlink`).
    pub eye_ids: Vec<String>,
    /// 꺼져 있으면 `None`.
    pub blink: Option<Live2dBlink>,
    /// 꺼져 있으면 비어 있음.
    pub breath: Vec<(String, Live2dBreathParam)>,
    /// 꺼져 있거나 모델에 없으면 `None`. (파일, 중력, 바람)
    pub physics: Option<(crate::live2d::PhysicsFile, [f32; 2], [f32; 2])>,
    /// 모델에 없으면 `None`. (파일, 페이드 초)
    pub pose: Option<(crate::live2d::PoseFile, f32)>,
    pub max_step: f64,
    /// 꺼져 있으면 `None`.
    pub look_at: Option<LookAt>,
    /// 부위 규칙이 먼저, `Anywhere`가 마지막.
    pub on_click: Vec<ClickRule>,
    /// `live2d/draw.wgsl`, `live2d/mask.wgsl`.
    pub draw_shader: naga::Module,
    pub mask_shader: naga::Module,
}

/// live2d `look_at`: 방향 계산은 `follow`(최대 이동 `[1, 1]`)와 같고, 파라미터에 성분 × 최대값을 더한다.
pub struct LookAt {
    pub follow: Follow,
    /// (파라미터 ID, 최대값, 성분)
    pub params: Vec<(String, f32, Axis)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// 오른쪽 +.
    X,
    /// 위 +.
    Y,
}

/// live2d `on_click` 규칙 하나: 맞으면 `clips` 중 하나를 재생.
pub struct ClickRule {
    pub hit: Hit,
    pub clips: Vec<usize>,
}

pub enum Hit {
    /// 이 drawable 중 하나의 메시 안 (HitArea·`drawable:<ID>`). 불투명도와 관계없음.
    Drawables(Vec<String>),
    /// 보이는 메시 어디든.
    Anywhere,
}

/// docs/wallpaper-format.md#포인터-따라가기-follow. 좌표는 설계 px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Follow {
    pub anchor: [f64; 2],
    pub max_offset: [f64; 2],
    pub range: f64,
    pub smoothing: f64,
    pub on_leave: OnLeave,
}

/// docs/wallpaper-format.md#레이어-효과-effects.
pub struct EffectSpec {
    pub kind: EffectKind,
    /// 값 그대로의 RGBA8 (R = 효과 세기).
    pub mask: Option<Texture>,
}

pub enum EffectKind {
    /// `assets/shaders/effects/<name>.wgsl`. uniform 멤버를 `values`(이름 → 성분)로 채운다.
    Builtin {
        name: String,
        module: naga::Module,
        layout: UniformLayout,
        values: HashMap<String, Vec<f32>>,
        animate: Animate,
    },
    /// Shadertoy 형식. `channels[0]`은 비어 있고 효과를 적용할 이미지가 들어간다.
    Shader {
        compiled: Compiled,
        props: Vec<Property>,
        lang: Language,
        channels: Vec<Option<Texture>>,
        sampling: Vec<ChannelDefaults>,
        animate: Animate,
    },
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
