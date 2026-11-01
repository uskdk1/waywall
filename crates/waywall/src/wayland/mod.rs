//! SCTK 핸들러 구현.

pub mod dmabuf;
pub mod globals;
pub mod output;

use smithay_client_toolkit::compositor::CompositorHandler;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::pointer::{
    PointerEvent, PointerEventKind, PointerHandler, ThemeSpec,
};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::wlr_layer::{
    LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_registry, registry_handlers};
use wayland_client::protocol::{wl_output, wl_pointer, wl_seat, wl_surface};
use wayland_client::{Connection, Proxy, QueueHandle};

use crate::cursor::{CursorShape, parse_shape};
use crate::daemon::State;
use output::SCALE_DENOMINATOR;

impl CompositorHandler for State {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        // fractional-scale이 있으면 그쪽 값이 정확하다.
        let fractional = self
            .index_by_surface(surface)
            .is_some_and(|i| self.outputs[i].has_fractional_scale());
        if !fractional {
            self.scale_changed(surface, factor.max(1) as u32 * SCALE_DENOMINATOR);
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        self.on_frame(surface);
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.add_output(output);
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.remove_output(&output);
    }
}

impl LayerShellHandler for State {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if let Some(i) = self.outputs.iter().position(|o| &o.layer == layer) {
            tracing::warn!(output = %self.outputs[i].name, "layer surface closed by compositor");
            self.drop_output(i);
        }
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(idx) = self.outputs.iter().position(|o| &o.layer == layer) else {
            return;
        };
        let (w, h) = configure.new_size;
        if w == 0 || h == 0 {
            tracing::warn!(output = %self.outputs[idx].name, "compositor sent a zero-sized configure");
            return;
        }
        self.configured(idx, (w, h));
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability != Capability::Pointer {
            return;
        }
        let surface = self.compositor.create_surface(qh);
        let cursor = &self.config.cursor;
        let env_theme = std::env::var("XCURSOR_THEME").ok();
        let name = Some(cursor.theme.as_str())
            .filter(|t| !t.is_empty())
            .or(env_theme.as_deref());
        let theme = match name {
            Some(name) => ThemeSpec::Named {
                name,
                size: cursor.size,
            },
            None => ThemeSpec::System,
        };
        match self.seat_state.get_pointer_with_theme::<_, ()>(
            qh,
            &seat,
            self.shm.wl_shm(),
            surface,
            theme,
        ) {
            Ok(p) => self.pointers.push(p),
            Err(e) => tracing::warn!("failed to get pointer: {e}"),
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            self.pointers.retain(|p| {
                p.pointer()
                    .data::<smithay_client_toolkit::seat::pointer::PointerData<()>>()
                    .is_none_or(|d| d.seat() != &seat)
            });
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for State {
    fn pointer_frame(
        &mut self,
        conn: &Connection,
        _: &QueueHandle<Self>,
        pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            self.pointer_event(&event.surface, &event.kind, event.position);
            let PointerEventKind::Enter { .. } = event.kind else {
                continue;
            };
            let Some(idx) = self.index_by_surface(&event.surface) else {
                continue;
            };
            let Some(themed) = self.pointers.iter().find(|p| p.pointer() == pointer) else {
                continue;
            };
            let result = match parse_shape(&self.outputs[idx].cfg.cursor_shape) {
                Ok(CursorShape::Hidden) => themed.hide_cursor().map_err(anyhow::Error::from),
                Ok(CursorShape::Icon(icon)) => {
                    themed.set_cursor(conn, icon).map_err(anyhow::Error::from)
                }
                Err(e) => Err(e),
            };
            if let Err(e) = result {
                tracing::warn!("failed to set cursor: {e:#}");
            }
        }
    }
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    registry_handlers![OutputState, SeatState];
}

delegate_registry!(State);
smithay_client_toolkit::delegate_dispatch2!(State);
