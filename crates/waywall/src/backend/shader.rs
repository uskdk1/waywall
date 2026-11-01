//! 셰이더 배경: 출력 하나의 wgpu surface + 파이프라인 + 입력 상태
//! (docs/wallpaper-format.md#셰이더-배경, docs/architecture.md#셰이더씬-wgpu).
//!
//! 페이싱은 바깥(daemon)이 frame 콜백과 타이머로 하고, 여기서는 "지금 한 장 그리기"만 한다.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::{Datelike, Local, Timelike};
use smithay_client_toolkit::compositor::FrameCallbackData;
use smithay_client_toolkit::seat::pointer::PointerEventKind;
use wayland_client::QueueHandle;
use waywall_proto::config::{PresentMode, Render, ShaderOptions};

use crate::daemon::State;
use crate::gpu::shadertoy::{Compiled, Inputs, UniformLayout};
use crate::gpu::{FULLSCREEN_ENTRY, Gpu};
use crate::properties::Property;
use crate::wayland::output::DesyncPlane;

/// 리눅스 input-event-codes.h의 버튼 코드 → `iPointer.w`/`iClick.w` 비트.
const BUTTONS: [(u32, u32); 3] = [(0x110, 1), (0x111, 2), (0x112, 4)];
/// 셰이더 출력값이 그대로 화면 값이 되는 포맷 (선호 순).
const UNORM_FORMATS: [wgpu::TextureFormat; 2] = [
    wgpu::TextureFormat::Bgra8Unorm,
    wgpu::TextureFormat::Rgba8Unorm,
];
/// Shadertoy의 `iMouse`는 왼쪽 버튼만 따른다.
const MOUSE_BUTTON_BIT: u32 = 1;

pub struct ShaderSpec {
    pub compiled: Compiled,
    pub props: Vec<Property>,
    pub opts: ShaderOptions,
    pub name: String,
}

/// 렌더 크기 계산에 필요한 출력 정보.
#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    pub logical: (u32, u32),
    pub physical: (u32, u32),
}

pub struct ShaderView {
    // 필드 순서 = drop 순서: wgpu surface가 wl_surface(plane)보다 먼저 사라져야 한다.
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    layout: UniformLayout,
    props: Vec<Property>,
    pub opts: ShaderOptions,
    pub name: String,
    /// 이 view를 구분하는 값 (오래된 타이머 무시용).
    pub id: u64,
    /// `None`이면 frame 콜백마다 그린다.
    interval: Option<Duration>,
    render_scale: f64,
    geometry: Geometry,
    clock: Clock,
    /// 셰이더 시계 기준 직전 프레임 시각.
    last_time: Option<f32>,
    /// 벽시계 기준 직전 프레임 시각 (fps 제한).
    last_draw: Option<Instant>,
    frame: i32,
    pointer: Pointer,
    /// frame 콜백을 요청해 두었고 아직 받지 않음.
    pub waiting_frame: bool,
    pub timer_armed: bool,
    /// suboptimal 때문에 이미 한 번 다시 구성했다. 그래도 계속되면 매 프레임 재구성하지 않는다.
    suboptimal_handled: bool,
    /// `list`의 fps 측정: 마지막 측정 이후 그린 프레임 수와 그 시작 시각.
    stats_frames: u32,
    stats_since: Instant,
    pub plane: DesyncPlane,
}

impl ShaderView {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        gpu: &Gpu,
        surface: wgpu::Surface<'static>,
        plane: DesyncPlane,
        spec: ShaderSpec,
        render: &Render,
        geometry: Geometry,
        id: u64,
        paused: bool,
    ) -> Result<Self> {
        let caps = surface.get_capabilities(&gpu.adapter);
        // Shadertoy처럼 셰이더 출력값을 그대로 화면에 쓴다: 8비트 UNORM(sRGB 인코딩 없음, 선형 float 아님).
        let format = UNORM_FORMATS
            .iter()
            .copied()
            .find(|f| caps.formats.contains(f))
            .or_else(|| caps.formats.iter().copied().find(|f| !f.is_srgb()))
            .or_else(|| caps.formats.first().copied())
            .context("the GPU cannot present to this surface")?;
        let wanted = match render.present_mode {
            PresentMode::Mailbox => wgpu::PresentMode::Mailbox,
            PresentMode::Fifo => wgpu::PresentMode::Fifo,
            PresentMode::Immediate => wgpu::PresentMode::Immediate,
        };
        let present_mode = if caps.present_modes.contains(&wanted) {
            wanted
        } else {
            tracing::debug!(?wanted, "present mode not supported, using fifo");
            wgpu::PresentMode::Fifo
        };
        let alpha_mode = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            wgpu::CompositeAlphaMode::Opaque
        } else {
            caps.alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto)
        };
        let render_scale = render.render_scale * spec.opts.render_scale;
        let (width, height) = render_size(geometry, render_scale);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width,
            height,
            present_mode,
            desired_maximum_frame_latency: render.frame_latency,
            alpha_mode,
            view_formats: Vec::new(),
        };
        gpu.scoped(|d| surface.configure(d, &config))
            .context("failed to configure the Vulkan surface")?;

        let layout = spec.compiled.layout.clone();
        let entry = spec.compiled.entry;
        let fragment = gpu.create_module(&spec.name, spec.compiled.module)?;
        let device = &gpu.device;
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("waywall shader uniforms"),
            size: layout.buffer_size(),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (pipeline, bind_group) = gpu
            .scoped(|d| {
                let bgl = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("waywall shader globals"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    }],
                });
                let pl = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("waywall shader"),
                    bind_group_layouts: &[Some(&bgl)],
                    immediate_size: 0,
                });
                let pipeline = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(&spec.name),
                    layout: Some(&pl),
                    vertex: wgpu::VertexState {
                        module: &gpu.fullscreen,
                        entry_point: Some(FULLSCREEN_ENTRY),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &fragment,
                        entry_point: Some(entry),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
                let bind_group = d.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("waywall shader globals"),
                    layout: &bgl,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniforms.as_entire_binding(),
                    }],
                });
                (pipeline, bind_group)
            })
            .with_context(|| format!("{}: failed to create the render pipeline", spec.name))?;

        plane
            .viewport
            .set_destination(geometry.logical.0 as i32, geometry.logical.1 as i32);
        let fps = [spec.opts.fps, render.max_fps]
            .into_iter()
            .filter(|&f| f > 0)
            .min();
        let mut clock = Clock::new(Instant::now());
        clock.set_paused(paused, Instant::now());
        tracing::info!(
            shader = %spec.name, ?format, ?present_mode, size = ?(width, height), ?fps,
            "shader ready"
        );
        Ok(Self {
            surface,
            config,
            pipeline,
            uniforms,
            bind_group,
            layout,
            props: spec.props,
            opts: spec.opts,
            name: spec.name,
            id,
            interval: fps.map(|f| Duration::from_secs_f64(1.0 / f64::from(f))),
            render_scale,
            geometry,
            clock,
            last_time: None,
            last_draw: None,
            frame: 0,
            pointer: Pointer::default(),
            waiting_frame: false,
            timer_armed: false,
            suboptimal_handled: false,
            stats_frames: 0,
            stats_since: Instant::now(),
            plane,
        })
    }

    /// 다음 프레임을 그릴 시각. `None`이면 지금.
    pub fn due(&self, now: Instant) -> Option<Instant> {
        let next = self.last_draw? + self.interval?;
        (next > now).then_some(next)
    }

    /// 마지막 호출 이후 평균 fps를 돌려주고 측정을 새로 시작한다.
    pub fn take_fps(&mut self) -> f32 {
        let now = Instant::now();
        let secs = now.duration_since(self.stats_since).as_secs_f32();
        let fps = if secs > 0.0 {
            self.stats_frames as f32 / secs
        } else {
            0.0
        };
        self.stats_frames = 0;
        self.stats_since = now;
        fps
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.clock.set_paused(paused, Instant::now());
    }

    pub fn resize(&mut self, gpu: &Gpu, geometry: Geometry) -> Result<()> {
        self.geometry = geometry;
        let (w, h) = render_size(geometry, self.render_scale);
        self.plane
            .viewport
            .set_destination(geometry.logical.0 as i32, geometry.logical.1 as i32);
        if (w, h) != (self.config.width, self.config.height) {
            self.config.width = w;
            self.config.height = h;
            self.configure(gpu)?;
            self.suboptimal_handled = false;
        }
        Ok(())
    }

    fn configure(&self, gpu: &Gpu) -> Result<()> {
        gpu.scoped(|d| self.surface.configure(d, &self.config))
            .context("failed to reconfigure the Vulkan surface")
    }

    /// 한 장 그리고, present 전에 frame 콜백을 요청해 둔다(present의 커밋에 함께 실린다).
    pub fn draw(&mut self, gpu: &Gpu, qh: &QueueHandle<State>) -> Result<()> {
        let mut suboptimal = false;
        let texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                suboptimal = !self.suboptimal_handled;
                t
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                // 그릴 수 없으니 콜백만 받아서 나중에 다시 시도한다.
                self.request_frame(qh);
                self.plane.surface.commit();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.configure(gpu)?;
                match self.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(t)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
                    other => bail!("{}: cannot acquire a frame ({other:?})", self.name),
                }
            }
            other => bail!("{}: cannot acquire a frame ({other:?})", self.name),
        };

        let now = Instant::now();
        let time = self.clock.now(now);
        let time_delta = self.last_time.map_or(0.0, |l| (time - l).max(0.0));
        self.last_time = Some(time);
        self.last_draw = Some(now);
        let date = Local::now();
        let midnight =
            f64::from(date.num_seconds_from_midnight()) + f64::from(date.nanosecond()) / 1e9;
        let (w, h) = (self.config.width as f32, self.config.height as f32);
        let inputs = Inputs {
            resolution: [w, h, 1.0],
            time,
            time_delta,
            frame: self.frame,
            frame_rate: if time_delta > 0.0 {
                1.0 / time_delta
            } else {
                0.0
            },
            date: [
                date.year() as f32,
                date.month0() as f32,
                date.day() as f32,
                midnight as f32,
            ],
            mouse: self.pointer.mouse,
            pointer: [
                self.pointer.pos[0],
                self.pointer.pos[1],
                f32::from(u8::from(self.pointer.inside)),
                self.pointer.buttons as f32,
            ],
            click: self.pointer.click,
        };
        gpu.queue
            .write_buffer(&self.uniforms, 0, &self.layout.write(&inputs, &self.props));

        let view = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(&self.name),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // 삼각형이 전부 덮으므로 이전 내용은 필요 없다.
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        gpu.queue.submit([encoder.finish()]);
        self.request_frame(qh);
        gpu.queue.present(texture);
        if suboptimal {
            // 컴포지터가 다른 포맷/모디파이어를 권한다(dmabuf feedback). present 후에 다시 구성한다.
            tracing::debug!(shader = %self.name, "suboptimal surface, reconfiguring");
            self.suboptimal_handled = true;
            self.configure(gpu)?;
        }

        tracing::trace!(shader = %self.name, frame = self.frame, time, "frame");
        self.frame = self.frame.wrapping_add(1);
        self.stats_frames = self.stats_frames.saturating_add(1);
        self.pointer.after_frame();
        Ok(())
    }

    fn request_frame(&mut self, qh: &QueueHandle<State>) {
        self.plane
            .surface
            .frame(qh, FrameCallbackData(self.plane.surface.clone()));
        self.waiting_frame = true;
    }

    /// 부모 layer surface 기준 포인터 이벤트(논리 좌표).
    pub fn pointer(&mut self, kind: &PointerEventKind, position: (f64, f64)) {
        let (lw, lh) = self.geometry.logical;
        let (rw, rh) = (self.config.width as f64, self.config.height as f64);
        // 렌더 픽셀, 왼쪽 아래 원점.
        let x = (position.0 * rw / f64::from(lw.max(1))) as f32;
        let y = (rh - position.1 * rh / f64::from(lh.max(1))) as f32;
        let time = self.clock.now(Instant::now());
        let p = &mut self.pointer;
        match kind {
            PointerEventKind::Enter { .. } => {
                p.inside = true;
                p.motion(x, y);
            }
            PointerEventKind::Leave { .. } => {
                p.inside = false;
                p.buttons = 0;
            }
            PointerEventKind::Motion { .. } => p.motion(x, y),
            PointerEventKind::Press { button, .. } => {
                if let Some(bit) = button_bit(*button) {
                    p.press(bit, x, y, time);
                }
            }
            PointerEventKind::Release { button, .. } => {
                if let Some(bit) = button_bit(*button) {
                    p.release(bit);
                }
            }
            PointerEventKind::Axis { .. } => {}
        }
    }
}

fn button_bit(code: u32) -> Option<u32> {
    BUTTONS.iter().find(|(c, _)| *c == code).map(|(_, b)| *b)
}

fn render_size(g: Geometry, scale: f64) -> (u32, u32) {
    let f = |v: u32| ((f64::from(v) * scale).round() as u32).max(1);
    (f(g.physical.0), f(g.physical.1))
}

/// pause 동안 멈추는 셰이더 시계.
#[derive(Debug, Clone, Copy)]
struct Clock {
    start: Instant,
    paused_total: Duration,
    paused_at: Option<Instant>,
}

impl Clock {
    fn new(now: Instant) -> Self {
        Self {
            start: now,
            paused_total: Duration::ZERO,
            paused_at: None,
        }
    }

    fn set_paused(&mut self, paused: bool, now: Instant) {
        match (paused, self.paused_at) {
            (true, None) => self.paused_at = Some(now),
            (false, Some(at)) => {
                self.paused_total += now - at;
                self.paused_at = None;
            }
            _ => {}
        }
    }

    fn now(&self, now: Instant) -> f32 {
        let t = self.paused_at.unwrap_or(now);
        t.saturating_duration_since(self.start)
            .saturating_sub(self.paused_total)
            .as_secs_f32()
    }
}

/// `iMouse`(Shadertoy 규칙), `iPointer`, `iClick` 상태.
#[derive(Debug, Clone, Copy, Default)]
struct Pointer {
    pos: [f32; 2],
    inside: bool,
    buttons: u32,
    mouse: [f32; 4],
    click: [f32; 4],
}

impl Pointer {
    fn motion(&mut self, x: f32, y: f32) {
        self.pos = [x, y];
        if self.buttons & MOUSE_BUTTON_BIT != 0 {
            self.mouse[0] = x;
            self.mouse[1] = y;
        }
    }

    fn press(&mut self, bit: u32, x: f32, y: f32, time: f32) {
        self.buttons |= bit;
        self.pos = [x, y];
        self.click = [x, y, time, bit as f32];
        if bit == MOUSE_BUTTON_BIT {
            // z>0: 누르는 중, w>0: 누른 첫 프레임.
            self.mouse = [x, y, x, y];
        }
    }

    fn release(&mut self, bit: u32) {
        self.buttons &= !bit;
        if bit == MOUSE_BUTTON_BIT {
            self.mouse[2] = -self.mouse[2].abs();
        }
    }

    fn after_frame(&mut self) {
        self.mouse[3] = -self.mouse[3].abs();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadertoy_mouse_rules() {
        let mut p = Pointer::default();
        p.motion(10.0, 20.0);
        assert_eq!(p.mouse, [0.0; 4], "hover does not move iMouse");
        p.press(MOUSE_BUTTON_BIT, 10.0, 20.0, 1.5);
        assert_eq!(p.mouse, [10.0, 20.0, 10.0, 20.0]);
        assert_eq!(p.click, [10.0, 20.0, 1.5, 1.0]);
        p.after_frame();
        assert!(p.mouse[3] < 0.0, "w is positive only on the click frame");
        p.motion(30.0, 40.0);
        assert_eq!(&p.mouse[..3], &[30.0, 40.0, 10.0]);
        p.release(MOUSE_BUTTON_BIT);
        assert!(p.mouse[2] < 0.0, "z<0 after release");
        p.motion(50.0, 60.0);
        assert_eq!(&p.mouse[..2], &[30.0, 40.0], "released: iMouse stays");
        assert_eq!(p.pos, [50.0, 60.0]);
        p.press(2, 1.0, 2.0, 3.0);
        assert_eq!(p.buttons, 2);
        assert_eq!(p.click[3], 2.0);
    }

    #[test]
    fn clock_stops_while_paused() {
        let t0 = Instant::now();
        let mut c = Clock::new(t0);
        let s = |n: u64| t0 + Duration::from_secs(n);
        assert_eq!(c.now(s(2)), 2.0);
        c.set_paused(true, s(2));
        assert_eq!(c.now(s(10)), 2.0);
        c.set_paused(false, s(10));
        assert_eq!(c.now(s(11)), 3.0);
    }

    #[test]
    fn render_size_rounds() {
        let g = Geometry {
            logical: (1536, 960),
            physical: (1920, 1200),
        };
        assert_eq!(render_size(g, 1.0), (1920, 1200));
        assert_eq!(render_size(g, 0.5), (960, 600));
        assert_eq!(render_size(g, 0.0), (1, 1));
    }
}
