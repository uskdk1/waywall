//! 씬 배경 (docs/wallpaper-format.md#씬-배경, docs/architecture.md#씬-렌더러).
//!
//! `load`가 `scene.toml`을 검사하고 텍스처를 디코딩해 `SceneSpec`을 만들고(GPU 불필요),
//! `render`가 출력마다 GPU 자원을 만들어 그린다.

mod load;
mod render;

pub use load::load;
pub use render::SceneRenderer;
use waywall_proto::Color;
use waywall_proto::config::{Blend, Fit};

/// 검사를 마친 씬. 출력 하나의 `SceneRenderer`가 소비한다.
pub struct SceneSpec {
    /// 설계 해상도 `[w, h]`.
    pub size: [f64; 2],
    pub fit: Fit,
    pub clear: Color,
    pub exposure: f64,
    pub pointer_smoothing: f64,
    pub settle_px: f64,
    /// 보이는 레이어만, 아래에서 위 순서.
    pub layers: Vec<ImageLayer>,
    pub image_shader: naga::Module,
    pub post_shader: naga::Module,
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
}

/// 알파를 선형 공간에서 곱해 둔 sRGB RGBA8.
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}
