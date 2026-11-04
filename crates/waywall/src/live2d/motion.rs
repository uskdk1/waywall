//! `.motion3.json` (Cubism 모션). 곡선을 시각마다 계산한다.
//!
//! 세그먼트 배열: 첫 점 `(t, v)` 뒤에 `종류, …`가 이어진다. 0 = 선형 `(t, v)`, 1 = 베지어 `(c1t, c1v, c2t, c2v, t, v)`,
//! 2 = 계단 `(t, v)`(끝에서 바뀜), 3 = 역계단 `(t, v)`(시작에서 바뀜).

use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

/// 곡선이 바꾸는 것.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Parameter,
    PartOpacity,
    /// 모델 전체 곡선 (`EyeBlink` 등, `id`로 구분).
    Model,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Segment {
    Linear([f32; 2]),
    Bezier([[f32; 2]; 3]),
    Stepped([f32; 2]),
    InverseStepped([f32; 2]),
}

impl Segment {
    fn end(&self) -> [f32; 2] {
        match *self {
            Segment::Linear(p) | Segment::Stepped(p) | Segment::InverseStepped(p) => p,
            Segment::Bezier([_, _, p]) => p,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Curve {
    pub target: Target,
    pub id: String,
    /// 이 곡선만의 페이드 시간(초). 없으면 모션의 값.
    pub fade_in: Option<f32>,
    pub fade_out: Option<f32>,
    first: [f32; 2],
    segments: Vec<Segment>,
}

#[derive(Debug, Clone)]
pub struct Motion {
    pub duration: f32,
    /// 모션 파일이 정한 페이드 시간(초).
    pub fade_in: Option<f32>,
    pub fade_out: Option<f32>,
    /// 베지어 제어점이 구간 안에 있다고 보고 시간 비율을 그대로 매개변수로 쓴다.
    beziers_restricted: bool,
    pub curves: Vec<Curve>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct File {
    meta: Meta,
    curves: Vec<CurveFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Meta {
    duration: f32,
    #[serde(default)]
    are_beziers_restricted: bool,
    fade_in_time: Option<f32>,
    fade_out_time: Option<f32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CurveFile {
    target: String,
    id: String,
    fade_in_time: Option<f32>,
    fade_out_time: Option<f32>,
    segments: Vec<f32>,
}

/// 음수는 "정하지 않음"(Cubism 관례).
fn seconds(v: Option<f32>) -> Option<f32> {
    v.filter(|s| *s >= 0.0)
}

impl Motion {
    pub fn read(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("invalid {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let f: File = serde_json::from_str(text)?;
        ensure!(f.meta.duration >= 0.0, "Meta.Duration must be >= 0");
        let curves = f
            .curves
            .into_iter()
            .map(|c| {
                let target = match c.target.as_str() {
                    "Parameter" => Target::Parameter,
                    "PartOpacity" => Target::PartOpacity,
                    "Model" => Target::Model,
                    t => bail!("curve `{}`: unknown target `{t}`", c.id),
                };
                let (first, segments) =
                    parse_segments(&c.segments).with_context(|| format!("curve `{}`", c.id))?;
                Ok(Curve {
                    target,
                    id: c.id,
                    fade_in: seconds(c.fade_in_time),
                    fade_out: seconds(c.fade_out_time),
                    first,
                    segments,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            duration: f.meta.duration,
            fade_in: seconds(f.meta.fade_in_time),
            fade_out: seconds(f.meta.fade_out_time),
            beziers_restricted: f.meta.are_beziers_restricted,
            curves,
        })
    }

    /// `curve`의 시각 `t`(초) 값. 마지막 점 뒤는 마지막 값.
    pub fn sample(&self, curve: &Curve, t: f32) -> f32 {
        let mut prev = curve.first;
        for s in &curve.segments {
            let end = s.end();
            if t < end[0] {
                let span = end[0] - prev[0];
                let k = if span > 0.0 {
                    ((t - prev[0]) / span).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                return match *s {
                    Segment::Linear(p) => prev[1] + (p[1] - prev[1]) * k,
                    Segment::Stepped(_) => prev[1],
                    Segment::InverseStepped(p) => p[1],
                    Segment::Bezier([c1, c2, p]) => {
                        let u = if self.beziers_restricted {
                            k
                        } else {
                            bezier_param(prev[0], c1[0], c2[0], p[0], t)
                        };
                        bezier(prev[1], c1[1], c2[1], p[1], u)
                    }
                };
            }
            prev = end;
        }
        prev[1]
    }
}

fn parse_segments(raw: &[f32]) -> Result<([f32; 2], Vec<Segment>)> {
    ensure!(raw.len() >= 2, "segments need a first point");
    let first = [raw[0], raw[1]];
    let mut out = Vec::new();
    let mut i = 2;
    while i < raw.len() {
        let kind = raw[i];
        let need = |n: usize| -> Result<&[f32]> {
            raw.get(i + 1..i + 1 + n)
                .with_context(|| format!("segment at {i} is cut short"))
        };
        let (seg, n) = match kind as i32 {
            0 => {
                let p = need(2)?;
                (Segment::Linear([p[0], p[1]]), 2)
            }
            1 => {
                let p = need(6)?;
                (
                    Segment::Bezier([[p[0], p[1]], [p[2], p[3]], [p[4], p[5]]]),
                    6,
                )
            }
            2 => {
                let p = need(2)?;
                (Segment::Stepped([p[0], p[1]]), 2)
            }
            3 => {
                let p = need(2)?;
                (Segment::InverseStepped([p[0], p[1]]), 2)
            }
            _ => bail!("unknown segment type {kind} at {i}"),
        };
        out.push(seg);
        i += 1 + n;
    }
    Ok((first, out))
}

fn bezier(p0: f32, p1: f32, p2: f32, p3: f32, u: f32) -> f32 {
    let v = 1.0 - u;
    v * v * v * p0 + 3.0 * v * v * u * p1 + 3.0 * v * u * u * p2 + u * u * u * p3
}

/// 시간 축 베지어 `x(u) = t`의 `u` (이분법, x는 u에 대해 단조 증가로 본다).
fn bezier_param(x0: f32, x1: f32, x2: f32, x3: f32, t: f32) -> f32 {
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if bezier(x0, x1, x2, x3, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "Version": 3,
        "Meta": { "Duration": 3.0, "Fps": 30.0, "Loop": true, "AreBeziersRestricted": true, "FadeInTime": 0.5 },
        "Curves": [
            { "Target": "Parameter", "Id": "A", "Segments": [0, 0, 0, 1, 10, 2, 2, 20, 3, 3, 30] },
            { "Target": "Parameter", "Id": "B", "FadeOutTime": 0.25, "Segments": [0, 0, 1, 0.333, 0, 0.667, 1, 1, 1] },
            { "Target": "PartOpacity", "Id": "P", "Segments": [0, 1] }
        ]
    }"#;

    #[test]
    fn segments_and_fades() {
        let m = Motion::parse(SAMPLE).unwrap();
        assert_eq!(m.duration, 3.0);
        assert_eq!((m.fade_in, m.fade_out), (Some(0.5), None));
        let a = &m.curves[0];
        // 선형 0→10 (0–1초), 계단 10→20 (1–2초: 끝에서 바뀜), 역계단 20→30 (2–3초: 시작에서 바뀜).
        assert!((m.sample(a, 0.5) - 5.0).abs() < 1e-5);
        assert_eq!(m.sample(a, 1.5), 10.0);
        assert_eq!(m.sample(a, 2.5), 30.0);
        assert_eq!(m.sample(a, 9.0), 30.0);
        // 대칭 베지어(ease-in-out)의 가운데는 0.5.
        let b = &m.curves[1];
        assert!((m.sample(b, 0.5) - 0.5).abs() < 1e-3);
        assert!(m.sample(b, 0.1) < 0.1);
        assert_eq!(b.fade_out, Some(0.25));
        assert_eq!(m.curves[2].target, Target::PartOpacity);
        assert_eq!(m.sample(&m.curves[2], 1.0), 1.0);
    }

    #[test]
    fn unrestricted_bezier_solves_time() {
        // 시간 제어점이 한쪽으로 몰린 곡선: 시간 비율을 그대로 쓰면 틀리고, x(u) = t를 풀어야 한다.
        let text = r#"{"Meta":{"Duration":1},"Curves":[{"Target":"Parameter","Id":"A","Segments":[0,0,1,0.9,0,0.95,1,1,1]}]}"#;
        let m = Motion::parse(text).unwrap();
        let c = &m.curves[0];
        let u = bezier_param(0.0, 0.9, 0.95, 1.0, 0.5);
        assert!((bezier(0.0, 0.9, 0.95, 1.0, u) - 0.5).abs() < 1e-4);
        assert!((m.sample(c, 0.5) - bezier(0.0, 0.0, 1.0, 1.0, u)).abs() < 1e-4);
    }

    #[test]
    fn rejects_bad_segments() {
        for bad in [
            r#"{"Meta":{"Duration":1},"Curves":[{"Target":"Parameter","Id":"A","Segments":[0]}]}"#,
            r#"{"Meta":{"Duration":1},"Curves":[{"Target":"Parameter","Id":"A","Segments":[0,0,1,0.5]}]}"#,
            r#"{"Meta":{"Duration":1},"Curves":[{"Target":"Parameter","Id":"A","Segments":[0,0,7,1,1]}]}"#,
            r#"{"Meta":{"Duration":1},"Curves":[{"Target":"Nope","Id":"A","Segments":[0,0]}]}"#,
        ] {
            assert!(Motion::parse(bad).is_err(), "{bad}");
        }
    }
}
