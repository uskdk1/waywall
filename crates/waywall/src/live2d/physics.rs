//! `.physics3.json` 물리: 입력 파라미터 → 진자 사슬 시뮬레이션 → 출력 파라미터 (머리카락·옷 흔들림).
//!
//! 계산 순서와 상수는 Cubism 물리와 같은 결과를 내도록 맞춘 값이다(형식 호환, 조절 값이 아님).
//! 바람·중력처럼 조절할 값은 설정에서 받는다 (docs/wallpaper-format.md#live2d).

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

/// 진자가 중력 방향 변화를 따라가는 정도를 줄이는 값.
const AIR_RESISTANCE: f32 = 5.0;
/// 이보다 오래 멈췄다 깨어나면 밀린 시간을 버린다(초).
const MAXIMUM_DELTA_TIME: f32 = 5.0;
/// 위치 정규화 최대값에 곱해 x를 0으로 붙이는 문턱.
const MOVEMENT_THRESHOLD: f32 = 0.001;
/// 입력·출력 `Weight`의 100%.
const MAXIMUM_WEIGHT: f32 = 100.0;
/// 진자 `Delay`가 기준으로 하는 프레임 수(초당).
const DELAY_FPS: f32 = 30.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct V2 {
    x: f32,
    y: f32,
}

impl V2 {
    const ZERO: V2 = V2 { x: 0.0, y: 0.0 };
    fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    fn add(self, o: V2) -> V2 {
        V2::new(self.x + o.x, self.y + o.y)
    }
    fn sub(self, o: V2) -> V2 {
        V2::new(self.x - o.x, self.y - o.y)
    }
    fn mul(self, k: f32) -> V2 {
        V2::new(self.x * k, self.y * k)
    }
    fn normalized(self) -> V2 {
        let len = self.x.hypot(self.y);
        if len == 0.0 {
            self
        } else {
            self.mul(1.0 / len)
        }
    }
}

/// `from`에서 `to`로 도는 각(라디안, −π–π).
fn direction_to_radian(from: V2, to: V2) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut r = to.y.atan2(to.x) - from.y.atan2(from.x);
    while r < -PI {
        r += TAU;
    }
    while r > PI {
        r -= TAU;
    }
    r
}

fn radian_to_direction(r: f32) -> V2 {
    V2::new(r.sin(), r.cos())
}

// ── 파일 ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct File {
    #[serde(default)]
    meta: Meta,
    #[serde(default)]
    physics_settings: Vec<SettingFile>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
struct Meta {
    #[serde(default)]
    fps: f32,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SettingFile {
    #[serde(default)]
    input: Vec<InputFile>,
    #[serde(default)]
    output: Vec<OutputFile>,
    #[serde(default)]
    vertices: Vec<VertexFile>,
    normalization: NormalizationFile,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Source {
    id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
enum Kind {
    X,
    Y,
    Angle,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InputFile {
    source: Source,
    weight: f32,
    #[serde(rename = "Type")]
    kind: Kind,
    #[serde(default)]
    reflect: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct OutputFile {
    destination: Source,
    vertex_index: usize,
    scale: f32,
    weight: f32,
    #[serde(rename = "Type")]
    kind: Kind,
    #[serde(default)]
    reflect: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct VertexFile {
    mobility: f32,
    delay: f32,
    acceleration: f32,
    radius: f32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Range {
    minimum: f32,
    default: f32,
    maximum: f32,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct NormalizationFile {
    position: Range,
    angle: Range,
}

/// 읽어 둔 `.physics3.json` (모델과 짝짓기 전).
pub struct PhysicsFile(File);

impl PhysicsFile {
    pub fn read(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("invalid {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        Ok(Self(serde_json::from_str(text)?))
    }
}

impl std::fmt::Debug for PhysicsFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PhysicsFile({} settings)", self.0.physics_settings.len())
    }
}

// ── 실행 ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct Particle {
    position: V2,
    last_position: V2,
    velocity: V2,
    last_gravity: V2,
    mobility: f32,
    delay: f32,
    acceleration: f32,
    radius: f32,
}

struct Input {
    param: usize,
    kind: Kind,
    weight: f32,
    reflect: bool,
}

struct Output {
    param: Option<usize>,
    vertex: usize,
    scale: f32,
    weight: f32,
    kind: Kind,
    reflect: bool,
}

struct Setting {
    inputs: Vec<Input>,
    outputs: Vec<Output>,
    particles: Vec<Particle>,
    position: Range,
    angle: Range,
    current: Vec<f32>,
    previous: Vec<f32>,
}

/// 파라미터 하나의 범위 (Core 값).
#[derive(Debug, Clone, Copy)]
pub struct ParamRange {
    pub minimum: f32,
    pub maximum: f32,
}

pub struct Physics {
    settings: Vec<Setting>,
    /// 물리 한 단계(초). 파일 Fps가 0이면 프레임 dt를 그대로 쓴다.
    step: Option<f32>,
    remain: f32,
    gravity: V2,
    wind: V2,
    /// 고정 단계 사이를 잇는 입력 값.
    input_cache: Vec<f32>,
    scratch: Vec<f32>,
}

/// 파라미터 값을 `[minimum, maximum]` → 정규화 범위로 옮긴다(가운데 기준 양쪽 따로).
fn normalize(value: f32, p: ParamRange, n: Range, reflect: bool) -> f32 {
    let max = p.maximum.max(p.minimum);
    let min = p.maximum.min(p.minimum);
    let v = value.clamp(min, max);
    let (nmin, nmax) = (n.minimum.min(n.maximum), n.minimum.max(n.maximum));
    let middle = min + (max - min).abs() / 2.0;
    let d = v - middle;
    let r = if d > 0.0 {
        let len = max - middle;
        if len == 0.0 {
            0.0
        } else {
            d * ((nmax - n.default) / len) + n.default
        }
    } else if d < 0.0 {
        let len = min - middle;
        if len == 0.0 {
            0.0
        } else {
            d * ((nmin - n.default) / len) + n.default
        }
    } else {
        n.default
    };
    if reflect { r } else { -r }
}

fn initial_particles(v: &[VertexFile]) -> Vec<Particle> {
    let mut position = V2::ZERO;
    v.iter()
        .enumerate()
        .map(|(i, v)| {
            if i != 0 {
                position = position.add(V2::new(0.0, v.radius));
            }
            Particle {
                position,
                last_position: position,
                velocity: V2::ZERO,
                last_gravity: V2::new(0.0, 1.0),
                mobility: v.mobility,
                delay: v.delay,
                acceleration: v.acceleration,
                radius: v.radius,
            }
        })
        .collect()
}

impl Physics {
    /// 모델 파라미터 ID에 맞춘다(없는 입력·출력 파라미터는 무시). `gravity`·`wind`는 사슬 좌표(y 위로 +).
    pub fn new(
        file: &PhysicsFile,
        parameter_ids: &[String],
        gravity: [f32; 2],
        wind: [f32; 2],
    ) -> Self {
        let index: HashMap<&str, usize> = parameter_ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        let settings = file
            .0
            .physics_settings
            .iter()
            .map(|s| Setting {
                inputs: s
                    .input
                    .iter()
                    .filter_map(|i| {
                        Some(Input {
                            param: *index.get(i.source.id.as_str())?,
                            kind: i.kind,
                            weight: i.weight,
                            reflect: i.reflect,
                        })
                    })
                    .collect(),
                outputs: s
                    .output
                    .iter()
                    .map(|o| Output {
                        param: index.get(o.destination.id.as_str()).copied(),
                        vertex: o.vertex_index,
                        scale: o.scale,
                        weight: o.weight,
                        kind: o.kind,
                        reflect: o.reflect,
                    })
                    .collect(),
                particles: initial_particles(&s.vertices),
                position: s.normalization.position,
                angle: s.normalization.angle,
                current: vec![0.0; s.output.len()],
                previous: vec![0.0; s.output.len()],
            })
            .collect();
        let fps = file.0.meta.fps;
        Self {
            settings,
            step: (fps > 0.0 && fps.is_finite()).then(|| 1.0 / fps),
            remain: 0.0,
            gravity: V2::new(gravity[0], gravity[1]),
            wind: V2::new(wind[0], wind[1]),
            input_cache: Vec::new(),
            scratch: Vec::new(),
        }
    }

    /// 지금 입력에서 사슬을 정지 상태로 놓는다(처음 한 번).
    pub fn stabilize(&mut self, params: &mut [f32], ranges: &[ParamRange]) {
        let (gravity, wind) = (self.gravity, self.wind);
        for s in &mut self.settings {
            let (translation, angle) = s.input(params, ranges);
            let threshold = MOVEMENT_THRESHOLD * s.position.maximum;
            let g = radian_to_direction(angle.to_radians()).normalized();
            let mut prev = translation;
            if let Some(first) = s.particles.first_mut() {
                first.position = translation;
            }
            for p in s.particles.iter_mut().skip(1) {
                let force = g.mul(p.acceleration).add(wind);
                p.last_position = p.position;
                p.velocity = V2::ZERO;
                p.position = prev.add(force.normalized().mul(p.radius));
                if p.position.x.abs() < threshold {
                    p.position.x = 0.0;
                }
                p.last_gravity = g;
                prev = p.position;
            }
            for (k, o) in s.outputs.iter().enumerate() {
                if let Some(v) = s.output_value(o, gravity) {
                    s.current[k] = v;
                    s.previous[k] = v;
                    if let Some(i) = o.param {
                        apply_output(&mut params[i], ranges[i], v, o);
                    }
                }
            }
        }
        self.input_cache = params.to_vec();
    }

    /// `dt`초 진행하고 출력 파라미터를 쓴다.
    pub fn evaluate(&mut self, params: &mut [f32], ranges: &[ParamRange], dt: f32) {
        if dt <= 0.0 || !dt.is_finite() {
            return;
        }
        if self.input_cache.len() != params.len() {
            self.input_cache = params.to_vec();
        }
        self.remain += dt;
        if self.remain > MAXIMUM_DELTA_TIME {
            self.remain = 0.0;
        }
        let step = self.step.unwrap_or(dt);
        let (gravity, wind) = (self.gravity, self.wind);
        self.scratch.resize(params.len(), 0.0);
        while self.remain >= step {
            // 고정 단계 사이의 입력은 지난 입력과 이번 입력을 이어서 쓴다.
            let w = step / self.remain;
            for (i, &v) in params.iter().enumerate() {
                let c = self.input_cache[i] * (1.0 - w) + v * w;
                self.scratch[i] = c;
                self.input_cache[i] = c;
            }
            for s in &mut self.settings {
                s.previous.copy_from_slice(&s.current);
                s.simulate(&mut self.scratch, ranges, gravity, wind, step);
            }
            self.remain -= step;
        }
        // 마지막 두 단계 사이를 남은 시간만큼 보간한다.
        let alpha = self.remain / step;
        for s in &self.settings {
            for ((o, &prev), &cur) in s.outputs.iter().zip(&s.previous).zip(&s.current) {
                if let Some(i) = o.param {
                    apply_output(
                        &mut params[i],
                        ranges[i],
                        prev * (1.0 - alpha) + cur * alpha,
                        o,
                    );
                }
            }
        }
    }
}

fn apply_output(param: &mut f32, r: ParamRange, value: f32, o: &Output) {
    let v = (value * o.scale).clamp(r.minimum, r.maximum);
    let w = o.weight / MAXIMUM_WEIGHT;
    *param = if w >= 1.0 {
        v
    } else {
        *param * (1.0 - w) + v * w
    };
}

impl Setting {
    /// 입력을 모은 (이동, 각도°).
    fn input(&self, params: &[f32], ranges: &[ParamRange]) -> (V2, f32) {
        let (mut tx, mut ty, mut angle) = (0.0, 0.0, 0.0);
        for i in &self.inputs {
            let n = match i.kind {
                Kind::X | Kind::Y => self.position,
                Kind::Angle => self.angle,
            };
            let v = normalize(params[i.param], ranges[i.param], n, i.reflect)
                * (i.weight / MAXIMUM_WEIGHT);
            match i.kind {
                Kind::X => tx += v,
                Kind::Y => ty += v,
                Kind::Angle => angle += v,
            }
        }
        // 이동을 각도만큼 되돌려 돌린다(Cubism과 같이 x를 먼저 바꾼 값으로 y를 계산).
        let r = (-angle).to_radians();
        let x = tx * r.cos() - ty * r.sin();
        let y = x * r.sin() + ty * r.cos();
        (V2::new(x, y), angle)
    }

    fn output_value(&self, o: &Output, gravity: V2) -> Option<f32> {
        let cur = self.particles.get(o.vertex)?;
        let prev = self.particles.get(o.vertex.checked_sub(1)?)?;
        let t = cur.position.sub(prev.position);
        let flip = |v: f32| if o.reflect { -v } else { v };
        Some(match o.kind {
            Kind::X => flip(t.x),
            Kind::Y => flip(t.y),
            Kind::Angle => {
                let parent = if o.vertex >= 2 {
                    let a = self.particles.get(o.vertex - 1)?;
                    let b = self.particles.get(o.vertex - 2)?;
                    a.position.sub(b.position)
                } else {
                    V2::new(-gravity.x, -gravity.y)
                };
                flip(direction_to_radian(parent, t))
            }
        })
    }

    fn simulate(
        &mut self,
        params: &mut [f32],
        ranges: &[ParamRange],
        gravity: V2,
        wind: V2,
        dt: f32,
    ) {
        let (translation, angle) = self.input(params, ranges);
        let threshold = MOVEMENT_THRESHOLD * self.position.maximum;
        let g = radian_to_direction(angle.to_radians()).normalized();
        if let Some(first) = self.particles.first_mut() {
            first.position = translation;
        }
        let mut prev = translation;
        for p in self.particles.iter_mut().skip(1) {
            let force = g.mul(p.acceleration).add(wind);
            p.last_position = p.position;
            let delay = p.delay * dt * DELAY_FPS;
            let r = direction_to_radian(p.last_gravity, g) / AIR_RESISTANCE;
            let mut d = p.position.sub(prev);
            // Cubism과 같이 x를 먼저 바꾼 값으로 y를 계산한다.
            d.x = r.cos() * d.x - d.y * r.sin();
            d.y = r.sin() * d.x + d.y * r.cos();
            p.position = prev
                .add(d)
                .add(p.velocity.mul(delay))
                .add(force.mul(delay * delay));
            let dir = p.position.sub(prev).normalized();
            p.position = prev.add(dir.mul(p.radius));
            if p.position.x.abs() < threshold {
                p.position.x = 0.0;
            }
            if delay != 0.0 {
                p.velocity = p.position.sub(p.last_position).mul(p.mobility / delay);
            }
            p.last_gravity = g;
            prev = p.position;
        }
        for k in 0..self.outputs.len() {
            let Some(v) = self.output_value(&self.outputs[k], gravity) else {
                continue;
            };
            self.current[k] = v;
            // 같은 단계 안에서 뒤 설정이 이 출력을 입력으로 쓸 수 있도록 단계 값에도 쓴다.
            if let Some(i) = self.outputs[k].param {
                apply_output(&mut params[i], ranges[i], v, &self.outputs[k]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 머리 각도(입력) → 머리카락 각도(출력) 하나짜리 사슬.
    const SAMPLE: &str = r#"{
        "Meta": { "Fps": 60 },
        "PhysicsSettings": [{
            "Input": [{ "Source": { "Target": "Parameter", "Id": "Head" }, "Weight": 100, "Type": "X", "Reflect": false }],
            "Output": [{ "Destination": { "Target": "Parameter", "Id": "Hair" }, "VertexIndex": 1, "Scale": 1, "Weight": 100, "Type": "Angle", "Reflect": false }],
            "Vertices": [
                { "Position": { "X": 0, "Y": 0 }, "Mobility": 1, "Delay": 1, "Acceleration": 1, "Radius": 0 },
                { "Position": { "X": 0, "Y": 10 }, "Mobility": 0.95, "Delay": 0.9, "Acceleration": 1.5, "Radius": 10 }
            ],
            "Normalization": { "Position": { "Minimum": -10, "Default": 0, "Maximum": 10 }, "Angle": { "Minimum": -10, "Default": 0, "Maximum": 10 } }
        }]
    }"#;

    fn ranges() -> Vec<ParamRange> {
        vec![
            ParamRange {
                minimum: -30.0,
                maximum: 30.0
            };
            2
        ]
    }

    #[test]
    fn hair_lags_then_settles() {
        let file = PhysicsFile::parse(SAMPLE).unwrap();
        let ids = ["Head".to_owned(), "Hair".to_owned()];
        let mut ph = Physics::new(&file, &ids, [0.0, -1.0], [0.0, 0.0]);
        let r = ranges();
        let mut p = vec![0.0, 0.0];
        ph.stabilize(&mut p, &r);
        assert!(p[1].abs() < 1e-3, "at rest: {}", p[1]);
        // 머리를 갑자기 옮기면 머리카락이 흔들렸다가 멈춘다.
        let mut seen = Vec::new();
        for _ in 0..240 {
            let mut q = vec![30.0, 0.0];
            ph.evaluate(&mut q, &r, 1.0 / 60.0);
            seen.push(q[1]);
        }
        let peak = seen.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.05, "hair did not move: {peak}");
        let tail = seen[200..]
            .iter()
            .fold(0.0f32, |m, v| m.max((v - seen[239]).abs()));
        assert!(
            tail < peak * 0.2,
            "hair did not settle: tail {tail}, peak {peak}"
        );
    }

    #[test]
    fn unknown_parameters_are_ignored() {
        let file = PhysicsFile::parse(SAMPLE).unwrap();
        let mut ph = Physics::new(&file, &["Other".to_owned()], [0.0, -1.0], [0.0, 0.0]);
        let r = vec![ParamRange {
            minimum: -1.0,
            maximum: 1.0,
        }];
        let mut p = vec![0.5];
        ph.stabilize(&mut p, &r);
        ph.evaluate(&mut p, &r, 0.1);
        assert_eq!(p, vec![0.5]);
    }
}
