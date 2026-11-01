//! `zwlr_foreign_toplevel_management_v1` (de-facto) 신호원
//! (docs/protocols.md#보조-zwlr_foreign_toplevel_management_v1-de-facto).

use std::collections::HashMap;
use std::sync::Arc;

use smithay_client_toolkit::dispatch2::Dispatch2;
use wayland_client::backend::{ObjectData, ObjectId};
use wayland_client::globals::GlobalList;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::{Connection, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

use super::ToplevelState;
use crate::daemon::State;

pub struct ManagerData;
pub struct HandleData;

#[derive(Debug, Clone, Default)]
struct Raw {
    outputs: Vec<WlOutput>,
    maximized: bool,
    minimized: bool,
    activated: bool,
    fullscreen: bool,
}

struct Toplevel {
    pending: Raw,
    current: Raw,
}

#[derive(Default)]
pub struct Toplevels {
    manager: Option<ZwlrForeignToplevelManagerV1>,
    map: HashMap<ObjectId, Toplevel>,
}

impl Toplevels {
    /// 설정 `visibility.foreign_toplevel`이 켜져 있고 컴포지터가 광고할 때만 바인딩한다.
    pub fn bind(globals: &GlobalList, qh: &QueueHandle<State>, enabled: bool) -> Self {
        let manager = enabled
            .then(|| globals.bind(qh, 1..=3, ManagerData).ok())
            .flatten();
        tracing::info!(foreign_toplevel = manager.is_some(), "visibility sources");
        Self {
            manager,
            map: HashMap::new(),
        }
    }
}

impl State {
    /// 확정된 toplevel 상태들 (출력은 이름으로).
    pub fn toplevel_states(&self) -> Vec<ToplevelState> {
        self.toplevels
            .map
            .values()
            .map(|t| {
                let c = &t.current;
                ToplevelState {
                    outputs: c
                        .outputs
                        .iter()
                        .filter_map(|o| self.output_state.info(o)?.name)
                        .collect(),
                    maximized: c.maximized,
                    minimized: c.minimized,
                    activated: c.activated,
                    fullscreen: c.fullscreen,
                }
            })
            .collect()
    }
}

impl Dispatch2<ZwlrForeignToplevelManagerV1, State> for ManagerData {
    fn event(
        &self,
        state: &mut State,
        _: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        match event {
            zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } => {
                state.toplevels.map.insert(
                    toplevel.id(),
                    Toplevel {
                        pending: Raw::default(),
                        current: Raw::default(),
                    },
                );
            }
            zwlr_foreign_toplevel_manager_v1::Event::Finished => {
                tracing::warn!("compositor stopped foreign-toplevel events");
                state.toplevels.manager = None;
            }
            _ => {}
        }
    }

    fn event_created_child(opcode: u16, qh: &QueueHandle<State>) -> Arc<dyn ObjectData> {
        match opcode {
            zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => {
                qh.make_data::<ZwlrForeignToplevelHandleV1, HandleData>(HandleData)
            }
            _ => panic!(
                "unexpected child object for opcode {opcode} of zwlr_foreign_toplevel_manager_v1"
            ),
        }
    }
}

impl Dispatch2<ZwlrForeignToplevelHandleV1, State> for HandleData {
    fn event(
        &self,
        state: &mut State,
        handle: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Event;
        if let Event::Closed = event {
            state.toplevels.map.remove(&handle.id());
            handle.destroy();
            state.visibility_changed();
            return;
        }
        let Some(t) = state.toplevels.map.get_mut(&handle.id()) else {
            return;
        };
        let p = &mut t.pending;
        match event {
            Event::OutputEnter { output } => {
                if !p.outputs.contains(&output) {
                    p.outputs.push(output);
                }
            }
            Event::OutputLeave { output } => p.outputs.retain(|o| o != &output),
            Event::State { state: bytes } => {
                use zwlr_foreign_toplevel_handle_v1::State as S;
                p.maximized = false;
                p.minimized = false;
                p.activated = false;
                p.fullscreen = false;
                for chunk in bytes.as_chunks::<4>().0 {
                    let v = u32::from_ne_bytes(*chunk);
                    match S::try_from(v).map(WEnum::Value) {
                        Ok(WEnum::Value(S::Maximized)) => p.maximized = true,
                        Ok(WEnum::Value(S::Minimized)) => p.minimized = true,
                        Ok(WEnum::Value(S::Activated)) => p.activated = true,
                        Ok(WEnum::Value(S::Fullscreen)) => p.fullscreen = true,
                        _ => {}
                    }
                }
            }
            Event::Done => {
                if t.current.fullscreen != t.pending.fullscreen
                    || t.current.maximized != t.pending.maximized
                    || t.current.activated != t.pending.activated
                {
                    let p = &t.pending;
                    tracing::debug!(
                        fullscreen = p.fullscreen,
                        maximized = p.maximized,
                        activated = p.activated,
                        outputs = p.outputs.len(),
                        "toplevel state"
                    );
                }
                t.current = t.pending.clone();
                state.visibility_changed();
            }
            _ => {}
        }
    }
}
