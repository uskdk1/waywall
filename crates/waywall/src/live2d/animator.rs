//! 매 프레임 파라미터 계산: 모션(페이드·크로스페이드) → 모델 곡선(눈) → 자동 눈 깜빡임 → 숨쉬기
//! (docs/wallpaper-format.md#live2d). Core에 넣기 전의 값만 다루고 GPU와는 관계없다.

use std::collections::HashMap;
use std::f32::consts::{PI, TAU};

use waywall_proto::config::{Live2dBlink, Live2dBreathParam};

use super::motion::{Motion, Target};
use super::physics::{ParamRange, Physics};
use super::pose::Pose;

/// 페이드 시간이 정해진 모션 하나.
#[derive(Debug, Clone)]
pub struct Clip {
    pub motion: Motion,
    pub fade_in: f32,
    pub fade_out: f32,
}

/// 모델 곡선 `EyeBlink`(Cubism 관례).
const EYE_BLINK_CURVE: &str = "EyeBlink";

/// 곡선마다 미리 찾아 둔 대상 번호.
#[derive(Debug, Clone, Copy)]
enum Slot {
    Parameter(usize),
    Part(usize),
    EyeBlink,
    None,
}

#[derive(Debug, Clone, Copy)]
struct Playing {
    clip: usize,
    start: f32,
    /// 나가기 시작했으면 다 나가는 시각.
    end: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum BlinkPhase {
    Open { until: f32 },
    Closing { from: f32 },
    Closed { from: f32 },
    Opening { from: f32 },
}

pub struct Animator {
    parameter_defaults: Vec<f32>,
    parameter_ranges: Vec<ParamRange>,
    /// 파츠 고르는 값의 처음 상태 (포즈 그룹은 첫 파츠만 1).
    selection_defaults: Vec<f32>,
    selection: Vec<f32>,
    clips: Vec<Clip>,
    slots: Vec<Vec<Slot>>,
    idle: Vec<usize>,
    playing: Vec<Playing>,
    time: f32,
    max_step: f32,
    eyes: Vec<usize>,
    blink: Option<(Live2dBlink, BlinkPhase)>,
    breath: Vec<(usize, Live2dBreathParam)>,
    physics: Option<Physics>,
    pose: Option<Pose>,
    rng: u64,
}

/// 0–1에서 사인 곡선으로 들어가고 나온다(Cubism의 페이드 곡선).
fn ease(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    0.5 - 0.5 * (PI * x).cos()
}

/// `elapsed`/`span`의 페이드 가중치. 시간이 0이면 바로 1.
fn fade(elapsed: f32, span: f32) -> f32 {
    if span <= 0.0 {
        1.0
    } else {
        ease(elapsed / span)
    }
}

pub struct AnimatorSpec<'a> {
    pub parameter_ids: &'a [String],
    pub parameter_defaults: &'a [f32],
    pub parameter_ranges: &'a [ParamRange],
    pub part_ids: &'a [String],
    pub part_defaults: &'a [f32],
    /// `clips`의 번호.
    pub idle: Vec<usize>,
    pub clips: Vec<Clip>,
    pub eye_ids: &'a [String],
    pub blink: Option<Live2dBlink>,
    pub breath: Vec<(String, Live2dBreathParam)>,
    pub physics: Option<Physics>,
    pub pose: Option<Pose>,
    pub max_step: f32,
    pub seed: u64,
}

impl Animator {
    pub fn new(s: AnimatorSpec<'_>) -> Self {
        let param: HashMap<&str, usize> = s
            .parameter_ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        let part: HashMap<&str, usize> = s
            .part_ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        let slots = s
            .clips
            .iter()
            .map(|c| {
                c.motion
                    .curves
                    .iter()
                    .map(|curve| match curve.target {
                        Target::Parameter => param
                            .get(curve.id.as_str())
                            .map_or(Slot::None, |&i| Slot::Parameter(i)),
                        Target::PartOpacity => part
                            .get(curve.id.as_str())
                            .map_or(Slot::None, |&i| Slot::Part(i)),
                        Target::Model if curve.id == EYE_BLINK_CURVE => Slot::EyeBlink,
                        Target::Model => Slot::None,
                    })
                    .collect()
            })
            .collect();
        let eyes = s
            .eye_ids
            .iter()
            .filter_map(|id| param.get(id.as_str()).copied())
            .collect();
        let breath = s
            .breath
            .into_iter()
            .filter_map(|(id, b)| param.get(id.as_str()).map(|&i| (i, b)))
            .collect();
        let mut selection_defaults = s.part_defaults.to_vec();
        if let Some(p) = &s.pose {
            p.initial_selection(&mut selection_defaults);
        }
        let mut physics = s.physics;
        if let Some(ph) = &mut physics {
            let mut rest = s.parameter_defaults.to_vec();
            ph.stabilize(&mut rest, s.parameter_ranges);
        }
        let mut a = Self {
            parameter_defaults: s.parameter_defaults.to_vec(),
            parameter_ranges: s.parameter_ranges.to_vec(),
            selection: selection_defaults.clone(),
            selection_defaults,
            clips: s.clips,
            slots,
            idle: s.idle,
            playing: Vec::new(),
            time: 0.0,
            max_step: s.max_step,
            eyes,
            blink: None,
            breath,
            physics,
            pose: s.pose,
            rng: s.seed | 1,
        };
        a.blink = s.blink.map(|b| {
            let until = a.random(b.interval);
            (b, BlinkPhase::Open { until })
        });
        a
    }

    /// xorshift64 → `[lo, hi]` 사이 값.
    fn random(&mut self, [lo, hi]: [f64; 2]) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        let u = (self.rng >> 11) as f64 / (1u64 << 53) as f64;
        (lo + (hi - lo) * u) as f32
    }

    /// 시간 축 위에서 계속 바뀌는 것이 있는지(모션, 눈 깜빡임, 숨쉬기).
    pub fn animating(&self) -> bool {
        !self.playing.is_empty()
            || !self.idle.is_empty()
            || self.blink.is_some() && !self.eyes.is_empty()
            || !self.breath.is_empty()
            || self.physics.is_some()
    }

    /// 다른 모션을 내보내며 `clip`을 들인다.
    pub fn play(&mut self, clip: usize) {
        let now = self.time;
        for p in &mut self.playing {
            if p.end.is_none() {
                p.end = Some(now + self.clips[p.clip].fade_out);
            }
        }
        self.playing.push(Playing {
            clip,
            start: now,
            end: None,
        });
    }

    /// `clips` 중 하나를 무작위로 들인다.
    pub fn play_random(&mut self, clips: &[usize]) {
        if clips.is_empty() {
            return;
        }
        let k = (self.random([0.0, clips.len() as f64]) as usize).min(clips.len() - 1);
        self.play(clips[k]);
    }

    fn next_idle(&mut self) {
        let idle = std::mem::take(&mut self.idle);
        self.play_random(&idle);
        self.idle = idle;
    }

    /// `dt`초 진행하고 파라미터·파츠 값을 쓴다(기본값에서 시작). `add`: 모션 뒤에 더할 값 (파라미터 번호, 값) — 시선.
    pub fn step(&mut self, dt: f32, params: &mut [f32], parts: &mut [f32], add: &[(usize, f32)]) {
        // 들어와 있는 모션이 끝나 가면(나가기 시작할 때) 다음 대기 모션을 겹쳐 들인다. 지난 프레임 시각 기준이라
        // 처음 들인 모션도 이번 dt만큼 진행된다.
        let current = self.playing.iter().rev().find(|p| p.end.is_none()).copied();
        match current {
            None => self.next_idle(),
            Some(p) => {
                let c = &self.clips[p.clip];
                if self.time >= p.start + (c.motion.duration - c.fade_out).max(0.0) {
                    self.next_idle();
                }
            }
        }
        let dt = dt.clamp(0.0, self.max_step);
        self.time += dt;
        let now = self.time;
        self.playing.retain(|p| p.end.is_none_or(|e| e > now));

        params.copy_from_slice(&self.parameter_defaults);
        self.selection.copy_from_slice(&self.selection_defaults);
        let mut eye_scale = 1.0f32;
        for p in &self.playing {
            let clip = &self.clips[p.clip];
            let m = &clip.motion;
            let local = (now - p.start).min(m.duration);
            let end = p.end.unwrap_or(p.start + m.duration);
            for (curve, slot) in m.curves.iter().zip(&self.slots[p.clip]) {
                let w = fade(now - p.start, curve.fade_in.unwrap_or(clip.fade_in))
                    * fade(end - now, curve.fade_out.unwrap_or(clip.fade_out));
                let v = m.sample(curve, local);
                match *slot {
                    Slot::Parameter(i) => params[i] += (v - params[i]) * w,
                    Slot::Part(i) => self.selection[i] += (v - self.selection[i]) * w,
                    Slot::EyeBlink => eye_scale *= 1.0 + (v - 1.0) * w,
                    Slot::None => {}
                }
            }
        }

        for &(i, v) in add {
            params[i] += v;
        }
        let blink = self.blink_value(now);
        for &i in &self.eyes {
            params[i] *= eye_scale * blink;
        }
        for &(i, b) in &self.breath {
            let s = (TAU * now / b.cycle as f32).sin();
            params[i] += (b.offset as f32 + b.peak as f32 * s) * b.weight as f32;
        }
        if let Some(ph) = &mut self.physics {
            ph.evaluate(params, &self.parameter_ranges, dt);
        }
        parts.copy_from_slice(&self.selection);
        if let Some(pose) = &mut self.pose {
            pose.apply(&self.selection, parts, dt);
        }
    }

    /// 눈 뜬 정도 1(뜸)–0(감음).
    fn blink_value(&mut self, now: f32) -> f32 {
        let Some((cfg, phase)) = self.blink.clone() else {
            return 1.0;
        };
        let (closing, closed, opening) =
            (cfg.closing as f32, cfg.closed as f32, cfg.opening as f32);
        let (next, value) = match phase {
            BlinkPhase::Open { until } if now >= until => (BlinkPhase::Closing { from: now }, 1.0),
            BlinkPhase::Open { .. } => (phase, 1.0),
            BlinkPhase::Closing { from } if now - from >= closing => {
                (BlinkPhase::Closed { from: now }, 0.0)
            }
            BlinkPhase::Closing { from } => (phase, 1.0 - (now - from) / closing),
            BlinkPhase::Closed { from } if now - from >= closed => {
                (BlinkPhase::Opening { from: now }, 0.0)
            }
            BlinkPhase::Closed { .. } => (phase, 0.0),
            BlinkPhase::Opening { from } if now - from >= opening => {
                let until = now + self.random(cfg.interval);
                (BlinkPhase::Open { until }, 1.0)
            }
            BlinkPhase::Opening { from } => (phase, (now - from) / opening),
        };
        self.blink = Some((cfg, next));
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn motion(duration: f32, id: &str, value: f32) -> Motion {
        Motion::parse(&format!(
            r#"{{"Meta":{{"Duration":{duration}}},"Curves":[{{"Target":"Parameter","Id":"{id}","Segments":[0,{value},0,{duration},{value}]}}]}}"#
        ))
        .unwrap()
    }

    fn animator(clips: Vec<Clip>, idle: Vec<usize>, blink: Option<Live2dBlink>) -> Animator {
        let ids = ["A".to_owned(), "Eye".to_owned(), "Breath".to_owned()];
        let ranges = [ParamRange {
            minimum: -1.0,
            maximum: 1.0,
        }; 3];
        Animator::new(AnimatorSpec {
            parameter_ids: &ids,
            parameter_defaults: &[0.0, 1.0, 0.0],
            parameter_ranges: &ranges,
            part_ids: &[],
            part_defaults: &[],
            idle,
            clips,
            eye_ids: &["Eye".to_owned()],
            blink,
            breath: Vec::new(),
            physics: None,
            pose: None,
            max_step: 1.0,
            seed: 7,
        })
    }

    #[test]
    fn idle_fades_in_and_crossfades() {
        let clip = |v| Clip {
            motion: motion(2.0, "A", v),
            fade_in: 0.5,
            fade_out: 0.5,
        };
        let mut a = animator(vec![clip(10.0)], vec![0], None);
        let (mut p, mut parts) = (vec![0.0; 3], vec![]);
        a.step(0.25, &mut p, &mut parts, &[]);
        // 들어오는 중: 0.25 / 0.5 → ease = 0.5.
        assert!((p[0] - 5.0).abs() < 1e-4, "{}", p[0]);
        a.step(0.5, &mut p, &mut parts, &[]);
        assert!((p[0] - 10.0).abs() < 1e-4);
        // 1.5초부터(길이 − 나가기) 다음 대기 모션이 겹쳐 들어와도 값이 끊기지 않는다(같은 값).
        for _ in 0..10 {
            a.step(0.2, &mut p, &mut parts, &[]);
            assert!(p[0] > 5.0, "{}", p[0]);
        }
        assert!(a.playing.len() <= 2);
    }

    #[test]
    fn blink_closes_and_reopens() {
        let blink = Live2dBlink {
            enabled: true,
            interval: [1.0, 1.0],
            closing: 0.1,
            closed: 0.1,
            opening: 0.1,
        };
        let mut a = animator(vec![], vec![], Some(blink));
        let (mut p, mut parts) = (vec![0.0; 3], vec![]);
        let mut seen = Vec::new();
        for _ in 0..80 {
            a.step(0.02, &mut p, &mut parts, &[]);
            seen.push(p[1]);
        }
        assert_eq!(seen[0], 1.0);
        assert!(seen.contains(&0.0), "eye never closed");
        assert_eq!(*seen.last().unwrap(), 1.0, "eye did not reopen");
        assert!(a.animating());
    }

    #[test]
    fn nothing_to_animate_sleeps() {
        let a = animator(vec![], vec![], None);
        assert!(!a.animating());
    }
}
