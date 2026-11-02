//! 디렉터리형 배경의 `project.toml` 스키마 (docs/wallpaper-format.md).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub wallpaper: ProjectInfo,
    /// 타입별 옵션. 키 검증은 타입 옵션으로 병합할 때 한다.
    pub options: Option<toml::Table>,
    pub properties: Option<toml::Table>,
    /// shader 배경의 텍스처 채널 (docs/wallpaper-format.md#텍스처-입력-ichannel0ichannel3).
    pub inputs: Option<toml::Table>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: WallpaperKind,
    pub entry: String,
    pub author: Option<String>,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WallpaperKind {
    Image,
    Video,
    Shader,
    Scene,
    Web,
}

impl WallpaperKind {
    /// 설정의 `[types.<이름>]` 키.
    pub fn key(self) -> &'static str {
        match self {
            WallpaperKind::Image => "image",
            WallpaperKind::Video => "video",
            WallpaperKind::Shader => "shader",
            WallpaperKind::Scene => "scene",
            WallpaperKind::Web => "web",
        }
    }

    pub const ALL: [WallpaperKind; 5] = [
        WallpaperKind::Image,
        WallpaperKind::Video,
        WallpaperKind::Shader,
        WallpaperKind::Scene,
        WallpaperKind::Web,
    ];
}
