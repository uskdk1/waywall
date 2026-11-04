//! Live2D 모델 (docs/wallpaper-format.md#live2d, docs/architecture.md#씬-렌더러).
//!
//! 런타임은 정적 링크한 PurismCore(Cubism Core C ABI v6 호환, MIT) 하나다. Core는 파라미터 → 메시 변형만 하고,
//! 모션·물리·포즈 같은 프레임워크 기능은 waywall이 한다.

mod animator;
mod core;
mod model3;
mod motion;
mod physics;
mod pose;

pub use animator::{Animator, AnimatorSpec, Clip};
pub use core::{BLEND_ADDITIVE, BLEND_MULTIPLICATIVE, Canvas, INVERTED_MASK, Model, Vec2};
pub use model3::EYE_BLINK_GROUP;
pub use model3::{Model3, MotionRef};
pub use motion::Motion;
pub use physics::{Physics, PhysicsFile};
pub use pose::{Pose, PoseFile};
