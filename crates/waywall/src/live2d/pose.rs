//! `.pose3.json` 포즈: 그룹 안에서 한 파츠만 보이게 하고 바뀔 때 페이드한다(예: 팔 두 세트).
//!
//! 모션의 `PartOpacity` 곡선은 "어느 파츠를 보일지 고르는 값"이고, 실제 불투명도는 여기서 정한다.
//! 계산은 Cubism 포즈와 같은 결과를 내도록 맞춘 값이다(형식 호환).

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

/// 고르는 값이 이보다 크면 그 파츠를 보이는 것으로 본다.
const SELECT_EPSILON: f32 = 0.001;
/// 나가는 파츠의 불투명도 곡선이 꺾이는 점과, 겹쳐 보일 때 허용하는 뒤 파츠 비율.
const PHI: f32 = 0.5;
const BACK_OPACITY_THRESHOLD: f32 = 0.15;

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct File {
    fade_in_time: Option<f32>,
    #[serde(default)]
    groups: Vec<Vec<PartFile>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PartFile {
    id: String,
    #[serde(default)]
    link: Vec<String>,
}

pub struct PoseFile(File);

impl PoseFile {
    pub fn read(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        Ok(Self(
            serde_json::from_str(&text).with_context(|| format!("invalid {}", path.display()))?,
        ))
    }

    /// 파일이 정한 페이드 시간(초). 음수·없음은 `None`.
    pub fn fade(&self) -> Option<f32> {
        self.0.fade_in_time.filter(|s| *s >= 0.0)
    }
}

impl std::fmt::Debug for PoseFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PoseFile({} groups)", self.0.groups.len())
    }
}

struct Member {
    part: usize,
    links: Vec<usize>,
}

pub struct Pose {
    groups: Vec<Vec<Member>>,
    fade: f32,
    /// 파츠마다 페이드 중인 불투명도.
    opacity: Vec<f32>,
}

impl Pose {
    /// 모델 파츠 ID에 맞춘다(없는 파츠는 무시). 처음에는 그룹의 첫 파츠만 보인다.
    pub fn new(file: &PoseFile, part_ids: &[String], fade: f32) -> Self {
        let index: HashMap<&str, usize> = part_ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        let groups: Vec<Vec<Member>> = file
            .0
            .groups
            .iter()
            .map(|g| {
                g.iter()
                    .filter_map(|p| {
                        Some(Member {
                            part: *index.get(p.id.as_str())?,
                            links: p
                                .link
                                .iter()
                                .filter_map(|l| index.get(l.as_str()).copied())
                                .collect(),
                        })
                    })
                    .collect()
            })
            .collect();
        let mut opacity = vec![1.0; part_ids.len()];
        for g in &groups {
            for (k, m) in g.iter().enumerate() {
                opacity[m.part] = if k == 0 { 1.0 } else { 0.0 };
            }
        }
        Self {
            groups,
            fade,
            opacity,
        }
    }

    /// 포즈 그룹에 든 파츠 (고르는 값의 처음 상태: 첫 파츠 1, 나머지 0).
    pub fn initial_selection(&self, selection: &mut [f32]) {
        for g in &self.groups {
            for (k, m) in g.iter().enumerate() {
                selection[m.part] = if k == 0 { 1.0 } else { 0.0 };
            }
        }
    }

    /// `selection`(모션이 정한 고르는 값)으로 그룹 파츠의 불투명도를 `dt`초만큼 페이드해 `parts`에 쓴다.
    pub fn apply(&mut self, selection: &[f32], parts: &mut [f32], dt: f32) {
        let dt = dt.max(0.0);
        for g in &self.groups {
            let mut visible = None;
            let mut new_opacity = 1.0f32;
            for (k, m) in g.iter().enumerate() {
                if selection[m.part] <= SELECT_EPSILON {
                    continue;
                }
                visible = Some(k);
                new_opacity = if self.fade == 0.0 {
                    1.0
                } else {
                    (self.opacity[m.part] + dt / self.fade).min(1.0)
                };
                break;
            }
            let visible = visible.unwrap_or(0);
            for (k, m) in g.iter().enumerate() {
                let o = &mut self.opacity[m.part];
                if k == visible {
                    *o = new_opacity;
                    continue;
                }
                let mut target = if new_opacity < PHI {
                    new_opacity * (PHI - 1.0) / PHI + 1.0
                } else {
                    (1.0 - new_opacity) * PHI / (1.0 - PHI)
                };
                let back = (1.0 - target) * (1.0 - new_opacity);
                if back > BACK_OPACITY_THRESHOLD {
                    target = 1.0 - BACK_OPACITY_THRESHOLD / (1.0 - new_opacity);
                }
                if *o > target {
                    *o = target;
                }
            }
            for m in g {
                parts[m.part] = self.opacity[m.part];
                for &l in &m.links {
                    parts[l] = self.opacity[m.part];
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_with_fade() {
        let file = PoseFile(
            serde_json::from_str(r#"{"Groups":[[{"Id":"A","Link":["A2"]},{"Id":"B"}]]}"#).unwrap(),
        );
        let ids: Vec<String> = ["A", "B", "A2"].map(String::from).to_vec();
        let mut pose = Pose::new(&file, &ids, 0.5);
        let mut sel = vec![0.0; 3];
        pose.initial_selection(&mut sel);
        let mut parts = vec![1.0; 3];
        pose.apply(&sel, &mut parts, 0.1);
        assert_eq!(parts, vec![1.0, 0.0, 1.0]);
        // B를 고르면 B가 페이드 시간에 걸쳐 들어오고 A(와 링크 A2)는 나간다.
        sel = vec![0.0, 1.0, 0.0];
        pose.apply(&sel, &mut parts, 0.1);
        assert!(parts[1] > 0.0 && parts[1] < 1.0, "{parts:?}");
        for _ in 0..10 {
            pose.apply(&sel, &mut parts, 0.1);
        }
        assert_eq!(parts[1], 1.0);
        assert_eq!(parts[0], 0.0);
        assert_eq!(parts[2], parts[0]);
    }
}
