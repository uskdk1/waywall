//! SCTK가 감싸지 않는 프로토콜 전역 객체와 이벤트 처리 (docs/protocols.md).

use anyhow::{Context, Result};
use smithay_client_toolkit::dispatch2::Dispatch2;
use wayland_client::globals::GlobalList;
use wayland_client::protocol::{wl_buffer, wl_surface::WlSurface};
use wayland_client::{Connection, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    wp_fractional_scale_v1::{self, WpFractionalScaleV1},
};
use wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1;
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};

use crate::daemon::State;

pub struct Globals {
    /// stable, 필수.
    pub viewporter: WpViewporter,
    /// staging, 없으면 정수 스케일로 폴백.
    pub fractional: Option<WpFractionalScaleManagerV1>,
    /// staging, 없으면 1×1 wl_shm 버퍼로 폴백.
    pub single_pixel: Option<WpSinglePixelBufferManagerV1>,
}

impl Globals {
    pub fn bind(globals: &GlobalList, qh: &QueueHandle<State>) -> Result<Self> {
        let viewporter = globals
            .bind(qh, 1..=1, NoEvents)
            .context("wp_viewporter is required")?;
        let fractional = globals.bind(qh, 1..=1, NoEvents).ok();
        let single_pixel = globals.bind(qh, 1..=1, NoEvents).ok();
        tracing::info!(
            fractional_scale = fractional.is_some(),
            single_pixel_buffer = single_pixel.is_some(),
            "optional protocols"
        );
        Ok(Self {
            viewporter,
            fractional,
            single_pixel,
        })
    }
}

/// 이벤트가 없는 인터페이스용 user data.
pub struct NoEvents;

/// wp_fractional_scale_v1: 어느 surface의 스케일인지 기억한다.
pub struct FractionalScale(pub WlSurface);

/// wp_single_pixel_buffer_v1로 만든 wl_buffer. release는 무시한다(내용이 바뀌지 않음).
pub struct PixelBuffer;

impl Dispatch2<WpViewporter, State> for NoEvents {
    fn event(
        &self,
        _: &mut State,
        _: &WpViewporter,
        ev: <WpViewporter as wayland_client::Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        let _ = ev;
    }
}

impl Dispatch2<WpViewport, State> for NoEvents {
    fn event(
        &self,
        _: &mut State,
        _: &WpViewport,
        ev: <WpViewport as wayland_client::Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        let _ = ev;
    }
}

impl Dispatch2<WpFractionalScaleManagerV1, State> for NoEvents {
    fn event(
        &self,
        _: &mut State,
        _: &WpFractionalScaleManagerV1,
        ev: <WpFractionalScaleManagerV1 as wayland_client::Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        let _ = ev;
    }
}

impl Dispatch2<WpSinglePixelBufferManagerV1, State> for NoEvents {
    fn event(
        &self,
        _: &mut State,
        _: &WpSinglePixelBufferManagerV1,
        ev: <WpSinglePixelBufferManagerV1 as wayland_client::Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        let _ = ev;
    }
}

impl Dispatch2<WpFractionalScaleV1, State> for FractionalScale {
    fn event(
        &self,
        state: &mut State,
        _: &WpFractionalScaleV1,
        ev: wp_fractional_scale_v1::Event,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = ev {
            state.scale_changed(&self.0, scale);
        }
    }
}

impl Dispatch2<wl_buffer::WlBuffer, State> for PixelBuffer {
    fn event(
        &self,
        _: &mut State,
        _: &wl_buffer::WlBuffer,
        _: wl_buffer::Event,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
    }
}
