//! 출력 하나 = layer surface 하나. 화면 구성:
//!
//! ```text
//! layer surface  ── 단색 버퍼(single-pixel 또는 1×1 shm), viewporter로 출력 크기까지 확대
//!   └ subsurface ── 이미지 버퍼(wl_shm), viewporter로 fit 모드 crop/scale
//! ```
//!
//! 크기 맞춤은 모두 컴포지터가 하므로, 한 번 커밋한 뒤에는 아무 비용이 없다.

use anyhow::{Context, Result};
use smithay_client_toolkit::compositor::{CompositorState, Region};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::LayerSurface;
use smithay_client_toolkit::shm::Shm;
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use smithay_client_toolkit::subcompositor::SubcompositorState;
use wayland_client::QueueHandle;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_shm;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use waywall_proto::Color;
use waywall_proto::config::{Fit, ImageOptions};
use waywall_proto::ipc::{RenderPath, WallpaperInfo};

use super::globals::{Globals, NoEvents, PixelBuffer};
use crate::backend::image::{Decoded, Request};
use crate::backend::video::VideoView;
use crate::backend::view::GpuView;
use crate::config::OutputConfig;
use crate::daemon::State;
use crate::fit::place;
use crate::visibility::frame::FrameWatch;

/// wp_fractional_scale_v1의 분모.
pub const SCALE_DENOMINATOR: u32 = 120;

/// 렌더링에 필요한 공유 객체.
pub struct Ctx<'a> {
    pub qh: &'a QueueHandle<State>,
    pub compositor: &'a CompositorState,
    pub subcompositor: &'a SubcompositorState,
    pub shm: &'a Shm,
    pub globals: &'a Globals,
}

/// 왜 이 내용을 보여주는지. 실패 시 어떤 대체 화면으로 갈지 결정한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// 사용자가 지정한 배경.
    Main,
    /// `appearance.empty`.
    Empty,
    /// `appearance.error`.
    Error,
    /// `appearance.loading`.
    Loading,
}

pub enum Content {
    Nothing,
    Color,
    Image(ShownImage),
    Gpu(Box<GpuView>),
    Video(Box<VideoView>),
}

#[derive(Debug, Clone)]
pub struct ShownImage {
    pub request: Request,
    pub opts: ImageOptions,
    /// 이 이미지가 실제로 디코딩된 크기 (downscale 결과).
    pub buffer_size: (u32, u32),
}

/// 진행 중인 로딩.
#[derive(Debug, Clone, Copy)]
pub struct Pending {
    pub generation: u64,
    pub purpose: Purpose,
}

pub struct Output {
    pub id: u64,
    pub wl_output: WlOutput,
    pub name: String,
    pub layer: LayerSurface,
    viewport: WpViewport,
    fractional: Option<WpFractionalScaleV1>,
    /// 첫 configure 전에는 `None`.
    pub logical: Option<(u32, u32)>,
    /// 스케일 × 120.
    pub scale120: u32,
    pub cfg: OutputConfig,
    pub content: Content,
    pub generation: u64,
    pub pending: Option<Pending>,
    /// `appearance.loading`이 배경 경로일 때의 로딩.
    pub loading: Option<Pending>,
    pub description: Option<String>,
    /// 설정된 배경 (로딩 중·실패 포함).
    pub wallpaper: Option<WallpaperInfo>,
    /// 마지막 배경 로딩 실패.
    pub error: Option<String>,
    /// `pause`로 멈춤 (CLI 사유).
    pub user_paused: bool,
    /// 다음 배경 로딩이 끝나면 응답할 `set` 요청들.
    pub waiters: Vec<u64>,
    /// 첫 프레임을 기다리는 영상. 첫 프레임이 오면 `content`로 바꾼다(그때까지 이전 화면 유지).
    pub incoming: Option<Box<VideoView>>,
    /// frame 콜백 감시 (Hidden 판정).
    pub watch: FrameWatch,
    /// 감시 타이머가 걸려 있다.
    pub watch_timer: bool,
    /// 정책상 덮였다 (wlr-foreign-toplevel / niri).
    pub covered_toplevel: bool,
    pub covered_niri: bool,
    /// 마지막으로 내용에 적용한 일시정지 상태.
    pub paused_applied: bool,
    solid: Option<SolidBuffer>,
    plane: Option<Plane>,
}

impl Output {
    pub fn new(
        ctx: &Ctx<'_>,
        id: u64,
        wl_output: WlOutput,
        name: String,
        layer: LayerSurface,
        scale120: u32,
        cfg: OutputConfig,
    ) -> Self {
        let surface = layer.wl_surface();
        let viewport = ctx
            .globals
            .viewporter
            .get_viewport(surface, ctx.qh, NoEvents);
        let fractional = ctx.globals.fractional.as_ref().map(|m| {
            m.get_fractional_scale(
                surface,
                ctx.qh,
                super::globals::FractionalScale(surface.clone()),
            )
        });
        Self {
            id,
            wl_output,
            name,
            layer,
            viewport,
            fractional,
            logical: None,
            scale120,
            cfg,
            content: Content::Nothing,
            generation: 0,
            pending: None,
            loading: None,
            description: None,
            wallpaper: None,
            error: None,
            user_paused: false,
            waiters: Vec::new(),
            incoming: None,
            watch: FrameWatch::default(),
            watch_timer: false,
            covered_toplevel: false,
            covered_niri: false,
            paused_applied: false,
            solid: None,
            plane: None,
        }
    }

    pub fn has_fractional_scale(&self) -> bool {
        self.fractional.is_some()
    }

    pub fn surface(&self) -> &WlSurface {
        self.layer.wl_surface()
    }

    /// 출력의 물리 픽셀 크기.
    pub fn physical(&self) -> Option<(u32, u32)> {
        let (w, h) = self.logical?;
        let scale = |v: u32| (v * self.scale120).div_ceil(SCALE_DENOMINATOR);
        Some((scale(w), scale(h)))
    }

    /// `list`에 표시할 렌더 경로.
    pub fn render_path(&self) -> Option<RenderPath> {
        match &self.content {
            Content::Nothing => None,
            Content::Color => Some(RenderPath::Solid),
            Content::Image(_) => Some(RenderPath::Shm),
            Content::Gpu(_) => Some(RenderPath::Gpu),
            Content::Video(v) => Some(v.mode.render_path()),
        }
    }

    /// 새 내용은 지금의 일시정지 상태로 만들어진다. frame 감시는 새 surface 기준으로 다시 시작.
    fn content_changed(&mut self) {
        self.watch.reset();
        self.paused_applied = self.is_paused();
    }

    /// 사유가 하나라도 있으면 멈춤 (docs/architecture.md#일시정지-사유).
    pub fn is_paused(&self) -> bool {
        self.user_paused || self.watch.hidden() || self.covered_toplevel || self.covered_niri
    }

    /// 계속 갱신되는 내용(셰이더, 영상)의 서브서피스. frame 콜백 감시 대상.
    pub fn live_surface(&self) -> Option<&WlSurface> {
        match &self.content {
            Content::Gpu(v) => Some(&v.plane.surface),
            Content::Video(v) => Some(&v.plane.surface),
            _ => None,
        }
    }

    /// 기본 배경을 로딩 중인지.
    pub fn loading_main(&self) -> bool {
        self.pending.is_some_and(|p| p.purpose == Purpose::Main)
    }

    pub fn next_generation(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    pub fn present_color(&mut self, ctx: &Ctx<'_>, color: Color) -> Result<()> {
        let old_solid = self.set_solid(ctx, color)?;
        let old_plane = self.plane.take();
        set_input(ctx, self.surface(), false)?;
        self.surface().commit();
        drop((old_solid, old_plane));
        self.content = Content::Color;
        self.content_changed();
        Ok(())
    }

    pub fn present_image(
        &mut self,
        ctx: &Ctx<'_>,
        decoded: &Decoded,
        request: Request,
        opts: ImageOptions,
    ) -> Result<()> {
        let old_solid = self.set_solid(ctx, opts.background)?;
        let plane = match self.plane.take() {
            Some(p) => p,
            None => Plane::new(ctx, self.surface())?,
        };
        let plane = self.plane.insert(plane);
        let old_image = plane.set_image(ctx, decoded)?;
        self.place_plane(opts.fit);
        set_input(ctx, self.surface(), opts.interactive)?;
        self.surface().commit();
        drop((old_solid, old_image));
        self.content = Content::Image(ShownImage {
            request,
            opts,
            buffer_size: (decoded.width, decoded.height),
        });
        self.content_changed();
        Ok(())
    }

    /// 첫 프레임을 붙인 영상으로 바꾼다. 영상 서브서피스는 이 부모 커밋과 함께 나타난다.
    pub fn present_video(&mut self, ctx: &Ctx<'_>, view: Box<VideoView>) -> Result<()> {
        let old_solid = self.set_solid(ctx, view.params.background)?;
        let old_plane = self.plane.take();
        set_input(ctx, self.surface(), view.params.interactive)?;
        self.surface().commit();
        drop((old_solid, old_plane));
        self.content = Content::Video(view);
        self.content_changed();
        Ok(())
    }

    /// GPU 서브서피스가 준비된 셰이더로 바꾼다. 첫 프레임 전에는 부모의 `background` 색이 보인다.
    pub fn present_gpu(&mut self, ctx: &Ctx<'_>, view: GpuView) -> Result<()> {
        let old_solid = self.set_solid(ctx, view.opts.background)?;
        let old_plane = self.plane.take();
        set_input(ctx, self.surface(), view.opts.interactive)?;
        self.surface().commit();
        drop((old_solid, old_plane));
        self.content = Content::Gpu(Box::new(view));
        self.content_changed();
        Ok(())
    }

    /// 출력 크기가 바뀌었을 때 버퍼를 다시 만들지 않고 배치만 갱신한다.
    pub fn relayout(&mut self) {
        let Some((w, h)) = self.logical else { return };
        if self.solid.is_none() {
            return;
        }
        self.viewport.set_destination(w as i32, h as i32);
        if let Content::Image(img) = &self.content {
            let fit = img.opts.fit;
            self.place_plane(fit);
        }
        if let Content::Video(v) = &mut self.content {
            v.relayout((w, h));
        }
        if let Some(v) = &mut self.incoming {
            v.relayout((w, h));
        }
        self.surface().commit();
    }

    fn place_plane(&self, fit: Fit) {
        let (Some(plane), Some(logical)) = (&self.plane, self.logical) else {
            return;
        };
        let p = place(fit, plane.size, logical);
        let (x, y, w, h) = p.source;
        plane.viewport.set_source(x, y, w, h);
        plane.viewport.set_destination(p.size.0, p.size.1);
        plane.subsurface.set_position(p.position.0, p.position.1);
        plane.surface.commit();
    }

    /// 부모 surface에 단색 버퍼를 붙인다. 이전 버퍼는 커밋 후 버리도록 돌려준다.
    fn set_solid(&mut self, ctx: &Ctx<'_>, color: Color) -> Result<Option<SolidBuffer>> {
        let (w, h) = self.logical.context("output not configured yet")?;
        let buffer = SolidBuffer::new(ctx, color)?;
        buffer.attach(self.surface())?;
        self.surface().damage_buffer(0, 0, 1, 1);
        self.viewport.set_destination(w as i32, h as i32);
        Ok(self.solid.replace(buffer))
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        // 서브서피스(이미지, GPU)를 부모 layer surface보다 먼저 정리한다.
        self.incoming = None;
        self.content = Content::Nothing;
        self.plane = None;
        self.viewport.destroy();
        if let Some(f) = &self.fractional {
            f.destroy();
        }
    }
}

fn set_input(ctx: &Ctx<'_>, surface: &WlSurface, interactive: bool) -> Result<()> {
    if interactive {
        surface.set_input_region(None);
    } else {
        let empty = Region::new(ctx.compositor).context("wl_compositor missing")?;
        surface.set_input_region(Some(empty.wl_region()));
    }
    Ok(())
}

/// 이미지를 담는 서브서피스.
struct Plane {
    surface: WlSurface,
    subsurface: WlSubsurface,
    viewport: WpViewport,
    image: Option<ShmImage>,
    size: (u32, u32),
}

struct ShmImage {
    _pool: SlotPool,
    _buffer: Buffer,
}

impl Plane {
    fn new(ctx: &Ctx<'_>, parent: &WlSurface) -> Result<Self> {
        let (subsurface, surface) = ctx.subcompositor.create_subsurface(parent.clone(), ctx.qh);
        let viewport = ctx
            .globals
            .viewporter
            .get_viewport(&surface, ctx.qh, NoEvents);
        // 입력은 항상 부모(layer surface)가 받는다.
        set_input(ctx, &surface, false)?;
        Ok(Self {
            surface,
            subsurface,
            viewport,
            image: None,
            size: (0, 0),
        })
    }

    fn set_image(&mut self, ctx: &Ctx<'_>, decoded: &Decoded) -> Result<Option<ShmImage>> {
        let (w, h) = (decoded.width, decoded.height);
        let stride = w * 4;
        let mut pool = SlotPool::new((stride * h) as usize, ctx.shm).context("wl_shm pool")?;
        let (buffer, canvas) = pool
            .create_buffer(w as i32, h as i32, stride as i32, wl_shm::Format::Argb8888)
            .context("wl_shm buffer")?;
        // SlotPool은 슬롯 크기를 올림하므로 canvas가 더 길 수 있다.
        canvas[..decoded.argb.len()].copy_from_slice(&decoded.argb);
        buffer
            .attach_to(&self.surface)
            .context("attach image buffer")?;
        self.surface.damage_buffer(0, 0, w as i32, h as i32);
        self.size = (w, h);
        Ok(self.image.replace(ShmImage {
            _pool: pool,
            _buffer: buffer,
        }))
    }
}

impl Drop for Plane {
    fn drop(&mut self) {
        self.viewport.destroy();
        self.subsurface.destroy();
        self.surface.destroy();
    }
}

enum SolidBuffer {
    /// wp_single_pixel_buffer_v1 (staging).
    Pixel(WlBuffer),
    /// 폴백: 1×1 wl_shm.
    Shm { _pool: SlotPool, buffer: Buffer },
}

impl SolidBuffer {
    fn new(ctx: &Ctx<'_>, color: Color) -> Result<Self> {
        if let Some(m) = &ctx.globals.single_pixel {
            let [r, g, b, a] = color.to_u32_premultiplied();
            return Ok(Self::Pixel(m.create_u32_rgba_buffer(
                r,
                g,
                b,
                a,
                ctx.qh,
                PixelBuffer,
            )));
        }
        let mut pool = SlotPool::new(4, ctx.shm).context("wl_shm pool")?;
        let (buffer, canvas) = pool
            .create_buffer(1, 1, 4, wl_shm::Format::Argb8888)
            .context("wl_shm buffer")?;
        canvas[..4].copy_from_slice(&color.to_argb8888_premultiplied());
        Ok(Self::Shm {
            _pool: pool,
            buffer,
        })
    }

    fn attach(&self, surface: &WlSurface) -> Result<()> {
        match self {
            Self::Pixel(b) => surface.attach(Some(b), 0, 0),
            Self::Shm { buffer, .. } => buffer.attach_to(surface).context("attach solid buffer")?,
        }
        Ok(())
    }
}

impl Drop for SolidBuffer {
    fn drop(&mut self) {
        if let Self::Pixel(b) = self {
            b.destroy();
        }
    }
}

/// 셰이더(Vulkan present)·영상(프레임마다 attach)용 서브서피스. desync라서 자기 커밋이 부모 커밋 없이 바로 반영된다.
pub struct DesyncPlane {
    pub surface: WlSurface,
    subsurface: WlSubsurface,
    pub viewport: WpViewport,
}

impl DesyncPlane {
    pub fn subsurface(&self) -> &WlSubsurface {
        &self.subsurface
    }

    pub fn new(ctx: &Ctx<'_>, parent: &WlSurface) -> Result<Self> {
        let (subsurface, surface) = ctx.subcompositor.create_subsurface(parent.clone(), ctx.qh);
        subsurface.set_desync();
        let viewport = ctx
            .globals
            .viewporter
            .get_viewport(&surface, ctx.qh, NoEvents);
        // 입력은 항상 부모(layer surface)가 받는다.
        set_input(ctx, &surface, false)?;
        Ok(Self {
            surface,
            subsurface,
            viewport,
        })
    }
}

impl Drop for DesyncPlane {
    fn drop(&mut self) {
        self.viewport.destroy();
        self.subsurface.destroy();
        self.surface.destroy();
    }
}
