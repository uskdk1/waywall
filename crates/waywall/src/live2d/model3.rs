//! `.model3.json` (Cubism 모델 설정 파일). 외부 형식이라 모르는 필드는 무시한다.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Model3 {
    pub file_references: FileReferences,
    #[serde(default)]
    pub groups: Vec<Group>,
    #[serde(default)]
    pub hit_areas: Vec<HitArea>,
}

/// 클릭 판정 영역: 이름 → drawable ID.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct HitArea {
    pub id: String,
    pub name: String,
}

/// 파라미터 묶음 (`EyeBlink`, `LipSync`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Group {
    pub target: String,
    pub name: String,
    #[serde(default)]
    pub ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MotionRef {
    /// `.motion3.json` (이 파일 기준 경로).
    pub file: String,
    pub fade_in_time: Option<f32>,
    pub fade_out_time: Option<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct FileReferences {
    /// `.moc3` (이 파일 기준 경로).
    pub moc: String,
    /// 텍스처 (이 파일 기준 경로). drawable의 텍스처 번호가 이 순서.
    pub textures: Vec<String>,
    /// 그룹 이름 → 모션.
    #[serde(default)]
    pub motions: BTreeMap<String, Vec<MotionRef>>,
    /// `.physics3.json`, `.pose3.json` (이 파일 기준 경로).
    pub physics: Option<String>,
    pub pose: Option<String>,
}

/// 눈 깜빡임 파라미터 그룹 이름 (Cubism 관례).
pub const EYE_BLINK_GROUP: &str = "EyeBlink";
const PARAMETER_TARGET: &str = "Parameter";

impl Model3 {
    /// `name` 그룹의 파라미터 ID.
    pub fn group(&self, name: &str) -> Vec<String> {
        self.groups
            .iter()
            .filter(|g| g.name == name && g.target == PARAMETER_TARGET)
            .flat_map(|g| g.ids.iter().cloned())
            .collect()
    }

    pub fn read(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("invalid {}", path.display()))
    }
}
