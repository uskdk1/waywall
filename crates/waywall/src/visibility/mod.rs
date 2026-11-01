//! 가림 감지: 신호원과 정책 (docs/protocols.md#가림-감지, docs/architecture.md#일시정지-사유).
//!
//! - `Hidden` (frame 콜백) — 항상 pause 사유
//! - `Covered` (wlr-foreign-toplevel, niri IPC) — `pause_when` 정책에 해당할 때만

pub mod foreign_toplevel;
pub mod frame;
pub mod niri;

use waywall_proto::config::PauseWhen;

/// toplevel 하나의 상태 (`done`으로 확정된 값).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToplevelState {
    /// 창이 걸친 출력 이름.
    pub outputs: Vec<String>,
    pub maximized: bool,
    pub minimized: bool,
    pub activated: bool,
    pub fullscreen: bool,
}

/// niri 신호원이 없을 때 `any-window`를 대신할 정책 (docs/config.md).
pub fn effective_policy(policy: PauseWhen, niri: bool) -> PauseWhen {
    match policy {
        PauseWhen::AnyWindow if !niri => PauseWhen::Maximized,
        p => p,
    }
}

/// 이 출력에 포커스된 전체화면(정책에 따라 최대화 포함) 창이 있는지.
pub fn covered_by_toplevels<'a>(
    policy: PauseWhen,
    output: &str,
    toplevels: impl IntoIterator<Item = &'a ToplevelState>,
) -> bool {
    let wants_maximized = match policy {
        PauseWhen::Never => return false,
        PauseWhen::Fullscreen => false,
        // any-window는 niri가 판정하지만, 전체화면/최대화도 당연히 덮는다.
        PauseWhen::Maximized | PauseWhen::AnyWindow => true,
    };
    toplevels.into_iter().any(|t| {
        t.activated
            && !t.minimized
            && (t.fullscreen || (wants_maximized && t.maximized))
            && t.outputs.iter().any(|o| o == output)
    })
}

/// niri: 출력의 활성 워크스페이스에 창이 있으면 덮인 것 (`any-window`에서만).
pub fn covered_by_niri(policy: PauseWhen, has_windows: bool) -> bool {
    policy == PauseWhen::AnyWindow && has_windows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tl(output: &str, f: impl FnOnce(&mut ToplevelState)) -> ToplevelState {
        let mut t = ToplevelState {
            outputs: vec![output.into()],
            activated: true,
            ..Default::default()
        };
        f(&mut t);
        t
    }

    #[test]
    fn policies() {
        let full = tl("DP-1", |t| t.fullscreen = true);
        let max = tl("DP-1", |t| t.maximized = true);
        let unfocused = tl("DP-1", |t| {
            t.fullscreen = true;
            t.activated = false;
        });
        let other = tl("eDP-1", |t| t.fullscreen = true);

        assert!(covered_by_toplevels(PauseWhen::Fullscreen, "DP-1", [&full]));
        assert!(!covered_by_toplevels(PauseWhen::Fullscreen, "DP-1", [&max]));
        assert!(covered_by_toplevels(PauseWhen::Maximized, "DP-1", [&max]));
        assert!(!covered_by_toplevels(PauseWhen::Never, "DP-1", [&full]));
        assert!(!covered_by_toplevels(
            PauseWhen::Fullscreen,
            "DP-1",
            [&unfocused]
        ));
        assert!(!covered_by_toplevels(
            PauseWhen::Fullscreen,
            "DP-1",
            [&other]
        ));
    }

    #[test]
    fn any_window_needs_niri() {
        assert_eq!(
            effective_policy(PauseWhen::AnyWindow, false),
            PauseWhen::Maximized
        );
        assert_eq!(
            effective_policy(PauseWhen::AnyWindow, true),
            PauseWhen::AnyWindow
        );
        assert!(covered_by_niri(PauseWhen::AnyWindow, true));
        assert!(!covered_by_niri(PauseWhen::Fullscreen, true));
    }
}
