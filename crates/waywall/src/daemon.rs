//! 데몬: 이벤트 루프, 출력별 내용 결정, IPC 요청 처리.

use std::collections::HashMap;
use std::ffi::c_void;
use std::io::Read;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use calloop::channel::{Channel, Event as ChannelEvent, Sender, channel};
use calloop::generic::Generic;
use calloop::signals::{Signal, Signals};
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction};
use calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::compositor::CompositorState;
use smithay_client_toolkit::output::OutputState;
use smithay_client_toolkit::registry::RegistryState;
use smithay_client_toolkit::seat::SeatState;
use smithay_client_toolkit::seat::pointer::{PointerEventKind, ThemedPointer};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell};
use smithay_client_toolkit::shm::Shm;
use smithay_client_toolkit::subcompositor::SubcompositorState;
use toml::Table;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, QueueHandle};
use waywall_proto::config::{
    Config, Fill, FillOrPrevious, ImageOptions, LayerKind, PauseWhen, ShaderOptions, VideoOptions,
};
use waywall_proto::ipc::{
    ListData, OutputStatus, PauseReason, PlayState, Reason, ReasonSource, Request, Response,
    WallpaperInfo,
};
use waywall_proto::project::WallpaperKind;

use crate::assets::Assets;
use crate::backend::image::{self, Decoded};
use crate::backend::scene;
use crate::backend::shader::ShaderSpec;
use crate::backend::video::{
    self, Mode as VideoMode, VideoEvent, VideoMsg, VideoParams, VideoView,
};
use crate::backend::view::{Geometry, GpuSpec, GpuView, RendererSpec, ViewOpts};
use crate::config::{Layers, OutputConfig, expand_path};
use crate::config_edit::{self, Target};
use crate::gpu::Gpu;
use crate::gpu::shadertoy::{self, Language};
use crate::ipc::{self, Clients};
use crate::properties;
use crate::runtime::{self, Runtime};
use crate::visibility;
use crate::visibility::foreign_toplevel::Toplevels;
use crate::visibility::frame::Change;
use crate::visibility::niri::{self, NiriMsg};
use crate::wallpaper::{self, Source, Wallpaper};
use crate::wayland::dmabuf::DrmFormat;
use crate::wayland::globals::Globals;
use crate::wayland::output::{
    Content, Ctx, DesyncPlane, Output, Pending, Purpose, SCALE_DENOMINATOR,
};
use smithay_client_toolkit::compositor::FrameCallbackData;
use smithay_client_toolkit::dmabuf::DmabufState;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1;

pub struct State {
    pub qh: QueueHandle<State>,
    pub registry: RegistryState,
    pub output_state: OutputState,
    pub seat_state: SeatState,
    pub compositor: CompositorState,
    pub subcompositor: SubcompositorState,
    pub layer_shell: LayerShell,
    pub shm: Shm,
    pub globals: Globals,

    /// `daemon --config` 값. `reload`가 같은 파일을 다시 읽는다.
    config_arg: Option<PathBuf>,
    pub layers: Layers,
    pub config: Config,
    runtime: Runtime,
    pub outputs: Vec<Output>,
    pub pointers: Vec<ThemedPointer>,
    next_output_id: u64,
    images: Sender<ImageDone>,
    loop_signal: LoopSignal,

    clients: Clients,
    waiters: HashMap<u64, Waiter>,
    next_waiter: u64,
    /// 모든 출력이 끝나서 응답할 차례인 waiter. 디스패치가 끝날 때마다 비운다.
    finished: Vec<u64>,

    conn: Connection,
    handle: LoopHandle<'static, State>,
    /// shader/scene이 있을 때만 존재한다.
    gpu: Option<Gpu>,

    pub dmabuf: DmabufState,
    /// 기본 dmabuf feedback에서 받은 포맷 (선호 순).
    pub dmabuf_formats: Vec<DrmFormat>,
    /// feedback 이벤트를 계속 받으려면 살아 있어야 한다.
    _dmabuf_feedback: Option<ZwpLinuxDmabufFeedbackV1>,
    videos: Sender<VideoMsg>,
    next_view: u64,

    pub toplevels: Toplevels,
    /// niri IPC: 출력 → 활성 워크스페이스에 창이 있는지.
    niri: HashMap<String, bool>,
    niri_active: bool,
}

/// 디코딩 스레드 → 메인 루프.
pub struct ImageDone {
    output: u64,
    pending: Pending,
    request: image::Request,
    opts: ImageOptions,
    result: Result<Decoded>,
}

/// 배경이 표시될 때까지 응답을 미루는 `set` 요청.
struct Waiter {
    client: u64,
    remaining: usize,
    errors: Vec<String>,
}

enum Handled {
    Reply(Response),
    /// 응답은 waiter가 끝날 때.
    Wait,
}

/// 검사를 통과해 바로 로딩할 수 있는 배경.
enum Prepared {
    Image {
        wallpaper: Wallpaper,
        path: PathBuf,
        opts: ImageOptions,
    },
    /// 셰이더 또는 씬.
    Gpu {
        wallpaper: Wallpaper,
        spec: Box<GpuSpec>,
    },
    /// 영상 또는 움직이는 GIF.
    Video {
        wallpaper: Wallpaper,
        path: PathBuf,
        params: VideoParams,
    },
}

impl Prepared {
    fn wallpaper(&self) -> &Wallpaper {
        match self {
            Prepared::Image { wallpaper, .. }
            | Prepared::Gpu { wallpaper, .. }
            | Prepared::Video { wallpaper, .. } => wallpaper,
        }
    }
}

pub fn run(config_arg: Option<PathBuf>, layers: Layers) -> Result<()> {
    let config = layers.global()?;
    let (listener, _socket) = ipc::bind()?;
    let conn =
        Connection::connect_to_env().context("failed to connect to the Wayland compositor")?;
    let (globals, queue) = registry_queue_init::<State>(&conn).context("wl_registry")?;
    let qh = queue.handle();

    let mut event_loop = EventLoop::<State>::try_new()?;
    let handle = event_loop.handle();
    WaylandSource::new(conn.clone(), queue)
        .insert(handle.clone())
        .map_err(|e| anyhow!("failed to insert Wayland source: {e}"))?;

    let (videos, video_rx): (Sender<VideoMsg>, Channel<VideoMsg>) = channel();
    handle
        .insert_source(video_rx, |ev, _, state| {
            if let ChannelEvent::Msg(msg) = ev {
                state.on_video(msg);
            }
        })
        .map_err(|e| anyhow!("failed to insert video channel: {e}"))?;

    let (images, image_rx): (Sender<ImageDone>, Channel<ImageDone>) = channel();
    handle
        .insert_source(image_rx, |ev, _, state| {
            if let ChannelEvent::Msg(done) = ev {
                state.on_image(done);
            }
        })
        .map_err(|e| anyhow!("failed to insert image channel: {e}"))?;

    let signals = Signals::new(&[Signal::SIGINT, Signal::SIGTERM])?;
    handle
        .insert_source(signals, |ev, _, state| {
            tracing::info!(signal = ?ev.signal(), "exiting");
            state.loop_signal.stop();
        })
        .map_err(|e| anyhow!("failed to insert signal source: {e}"))?;

    insert_listener(&handle, listener)?;

    // v4 기본 feedback으로 영상 버퍼 포맷을 정한다. 없으면 v3 modifier 이벤트 목록, 그것도 없으면 shm.
    let dmabuf = DmabufState::new(&globals, &qh);
    let dmabuf_feedback = dmabuf.get_default_feedback(&qh).ok();
    let toplevels = Toplevels::bind(&globals, &qh, config.visibility.foreign_toplevel);
    let niri_active = config.visibility.niri && start_niri(&handle);
    if config.pause_when == PauseWhen::AnyWindow && !niri_active {
        tracing::warn!(
            "pause_when = \"any-window\" needs visibility.niri = true under niri; acting like \"maximized\""
        );
    }
    let compositor = CompositorState::bind(&globals, &qh).context("wl_compositor is required")?;
    let subcompositor = SubcompositorState::bind(compositor.wl_compositor().clone(), &globals, &qh)
        .context("wl_subcompositor is required")?;
    let mut state = State {
        registry: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        layer_shell: LayerShell::bind(&globals, &qh)
            .context("zwlr_layer_shell_v1 is required (GNOME is not supported)")?,
        shm: Shm::bind(&globals, &qh).context("wl_shm is required")?,
        globals: Globals::bind(&globals, &qh)?,
        compositor,
        subcompositor,

        qh,
        config_arg,
        layers,
        config,
        runtime: Runtime::default(),
        outputs: Vec::new(),
        pointers: Vec::new(),
        next_output_id: 0,
        images,
        loop_signal: event_loop.get_signal(),
        clients: Clients::default(),
        waiters: HashMap::new(),
        next_waiter: 0,
        finished: Vec::new(),
        conn,
        handle,
        gpu: None,
        _dmabuf_feedback: dmabuf_feedback,
        dmabuf,
        dmabuf_formats: Vec::new(),
        videos,
        next_view: 0,
        toplevels,
        niri: HashMap::new(),
        niri_active,
    };

    tracing::info!(config = ?state.layers.config_path(), "waywall daemon started");
    event_loop.run(None, &mut state, State::after_dispatch)?;
    Ok(())
}

fn insert_listener(handle: &LoopHandle<'static, State>, listener: UnixListener) -> Result<()> {
    let h = handle.clone();
    handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            move |_, listener, state| {
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if let Err(e) = accept(&h, state, stream) {
                                tracing::warn!("failed to accept IPC client: {e:#}");
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(e) => {
                            tracing::warn!("IPC accept failed: {e}");
                            break;
                        }
                    }
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| anyhow!("failed to insert IPC listener: {e}"))?;
    Ok(())
}

fn accept(
    handle: &LoopHandle<'static, State>,
    state: &mut State,
    stream: std::os::unix::net::UnixStream,
) -> Result<()> {
    stream.set_nonblocking(false)?;
    let id = state.clients.add(stream.try_clone()?);
    tracing::debug!(client = id, "IPC client connected");
    handle
        .insert_source(
            Generic::new(stream, Interest::READ, Mode::Level),
            move |_, stream, state| {
                let mut chunk = [0u8; ipc::READ_CHUNK];
                let action = match (&**stream).read(&mut chunk) {
                    Ok(0) => {
                        state.clients.eof(id);
                        PostAction::Remove
                    }
                    Ok(n) => {
                        state.clients.feed(id, &chunk[..n]);
                        PostAction::Continue
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => PostAction::Continue,
                    Err(e) => {
                        tracing::debug!(client = id, "IPC read failed: {e}");
                        state.clients.eof(id);
                        PostAction::Remove
                    }
                };
                state.pump(id);
                Ok(action)
            },
        )
        .map_err(|e| anyhow!("failed to insert IPC client: {e}"))?;
    Ok(())
}

/// `self.outputs`를 가변으로 빌리는 동안에도 쓸 수 있도록 필드를 따로 빌린다.
macro_rules! ctx {
    ($s:expr) => {
        Ctx {
            qh: &$s.qh,
            compositor: &$s.compositor,
            subcompositor: &$s.subcompositor,
            shm: &$s.shm,
            globals: &$s.globals,
        }
    };
}

impl State {
    fn index_by_id(&self, id: u64) -> Option<usize> {
        self.outputs.iter().position(|o| o.id == id)
    }

    pub fn index_by_surface(&self, surface: &WlSurface) -> Option<usize> {
        self.outputs.iter().position(|o| o.surface() == surface)
    }

    /// 설정 파일 + CLI 계층.
    fn output_config(&mut self, name: &str) -> Result<OutputConfig> {
        let mut cfg = self.layers.output(name)?;
        self.runtime.apply(name, &mut cfg);
        Ok(cfg)
    }

    pub fn add_output(&mut self, wl_output: WlOutput) {
        let Some(info) = self.output_state.info(&wl_output) else {
            return;
        };
        let name = info
            .name
            .clone()
            .unwrap_or_else(|| format!("output-{}", info.id));
        let cfg = match self.output_config(&name) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(output = %name, "{e:#}");
                return;
            }
        };

        let surface = self.compositor.create_surface(&self.qh);
        let layer = self.layer_shell.create_layer_surface(
            &self.qh,
            surface,
            layer_kind(self.config.surface.layer),
            Some(self.config.surface.namespace.clone()),
            Some(&wl_output),
        );
        layer.set_anchor(Anchor::all());
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(0, 0);
        layer.commit();

        let id = self.next_output_id;
        self.next_output_id += 1;
        let scale120 = info.scale_factor.max(1) as u32 * SCALE_DENOMINATOR;
        tracing::info!(output = %name, description = ?info.description, "output added");
        let user_paused = self.runtime.is_paused(&name);
        let mut output = Output::new(&ctx!(self), id, wl_output, name, layer, scale120, cfg);
        output.description = info.description.clone();
        output.user_paused = user_paused;
        self.outputs.push(output);
        self.visibility_changed();
    }

    pub fn remove_output(&mut self, wl_output: &WlOutput) {
        if let Some(idx) = self.outputs.iter().position(|o| &o.wl_output == wl_output) {
            tracing::info!(output = %self.outputs[idx].name, "output removed");
            self.drop_output(idx);
        }
    }

    pub fn drop_output(&mut self, idx: usize) {
        let name = self.outputs[idx].name.clone();
        self.settle(idx, Err(format!("{name}: output removed")));
        self.outputs.remove(idx);
    }

    pub fn configured(&mut self, idx: usize, size: (u32, u32)) {
        let out = &mut self.outputs[idx];
        let first = out.logical.is_none();
        if out.logical == Some(size) {
            return;
        }
        out.logical = Some(size);
        tracing::debug!(output = %out.name, ?size, "configured");
        if first {
            self.show_configured(idx);
        } else {
            self.geometry_changed(idx);
        }
    }

    /// `scale120`: 스케일 × 120 (fractional-scale 단위).
    pub fn scale_changed(&mut self, surface: &WlSurface, scale120: u32) {
        let Some(idx) = self.index_by_surface(surface) else {
            return;
        };
        let out = &mut self.outputs[idx];
        if out.scale120 == scale120 {
            return;
        }
        tracing::debug!(output = %out.name, scale = scale120 as f64 / SCALE_DENOMINATOR as f64, "scale changed");
        out.scale120 = scale120;
        if out.logical.is_some() {
            self.geometry_changed(idx);
        }
    }

    /// 크기/스케일 변경: 배치를 즉시 갱신하고, 미리 줄인 이미지가 이제 부족하면 다시 읽는다.
    fn geometry_changed(&mut self, idx: usize) {
        self.outputs[idx].relayout();
        if matches!(self.outputs[idx].content, Content::Gpu(_)) {
            self.gpu_resized(idx);
            return;
        }
        let out = &self.outputs[idx];
        let Content::Image(img) = &out.content else {
            return;
        };
        let Some(target) = out.physical() else { return };
        if img.opts.downscale && (img.buffer_size.0 < target.0 && img.buffer_size.1 < target.1) {
            let mut request = img.request.clone();
            request.target = target;
            let opts = img.opts.clone();
            let purpose = Purpose::Main;
            self.spawn_image(idx, request, opts, purpose);
        }
    }

    /// 설정이 바뀐 출력을 다시 그린다. configure 전이면 configure 때 그린다.
    fn refresh(&mut self, idx: usize) {
        if self.outputs[idx].logical.is_none() {
            return;
        }
        self.show_configured(idx);
        // 동기적으로 끝났으면(단색, 즉시 실패 등) 기다리던 요청에 바로 응답한다.
        if !self.outputs[idx].loading_main() {
            self.settle(idx, Ok(()));
        }
    }

    /// 설정에 따라 출력에 보일 내용을 정한다.
    fn show_configured(&mut self, idx: usize) {
        self.outputs[idx].error = None;
        match self.outputs[idx].cfg.wallpaper.clone() {
            Some(spec) => self.load(idx, &spec, Purpose::Main),
            None => {
                self.outputs[idx].wallpaper = None;
                let fill = self.outputs[idx].cfg.appearance.empty.clone();
                self.apply_fill(idx, fill, Purpose::Empty);
            }
        }
    }

    fn apply_fill(&mut self, idx: usize, fill: Fill, purpose: Purpose) {
        match fill {
            Fill::Color(c) => {
                let ctx = ctx!(self);
                let out = &mut self.outputs[idx];
                if purpose != Purpose::Loading {
                    out.next_generation();
                    out.pending = None;
                    out.incoming = None;
                }
                if let Err(e) = out.present_color(&ctx, c) {
                    tracing::error!(output = %out.name, "{e:#}");
                }
            }
            Fill::Wallpaper(spec) => self.load(idx, &spec, purpose),
        }
    }

    fn load(&mut self, idx: usize, spec: &str, purpose: Purpose) {
        if purpose == Purpose::Main {
            self.show_loading(idx);
        }
        if let Err(e) = self.try_load(idx, spec, purpose) {
            self.failed(idx, purpose, e);
        }
    }

    /// 경로 해석, 타입 판별, 옵션 검사. 출력 상태는 바꾸지 않는다.
    fn prepare(&self, cfg: &OutputConfig, spec: &str) -> Result<Prepared> {
        let wp = wallpaper::resolve(spec, &self.config.detect, &self.layers.base_dir())?;
        match (wp.kind, &wp.source) {
            (WallpaperKind::Image, Source::File(path)) => {
                let opts: ImageOptions =
                    self.layers
                        .type_options(wp.kind, wp.options.as_ref(), cfg)?;
                if is_gif(path) && video::gif_is_animated(path)? {
                    return Ok(Prepared::Video {
                        path: path.clone(),
                        params: VideoParams::gif(&opts),
                        wallpaper: wp,
                    });
                }
                Ok(Prepared::Image {
                    path: path.clone(),
                    wallpaper: wp,
                    opts,
                })
            }
            (WallpaperKind::Shader, Source::File(path)) => {
                let opts: ShaderOptions =
                    self.layers
                        .type_options(wp.kind, wp.options.as_ref(), cfg)?;
                let props = properties::resolve(wp.properties.as_ref(), &cfg.properties)?;
                let source = std::fs::read_to_string(path)
                    .with_context(|| format!("failed to read {}", path.display()))?;
                let compiled = shadertoy::compile(
                    &self.assets(),
                    Language::from_path(path),
                    &source,
                    &props,
                    &wp.name,
                )?;
                Ok(Prepared::Gpu {
                    spec: Box::new(GpuSpec {
                        renderer: RendererSpec::Shader(Box::new(ShaderSpec { compiled, props })),
                        opts: ViewOpts {
                            fps: opts.fps,
                            render_scale: opts.render_scale,
                            interactive: opts.interactive,
                            background: opts.background,
                        },
                        name: wp.name.clone(),
                    }),
                    wallpaper: wp,
                })
            }
            (WallpaperKind::Scene, Source::File(path)) => {
                let (scene, opts) = scene::load(path, &wp, &self.layers, cfg, &self.assets())?;
                Ok(Prepared::Gpu {
                    spec: Box::new(GpuSpec {
                        renderer: RendererSpec::Scene(Box::new(scene)),
                        opts: ViewOpts {
                            fps: opts.fps,
                            render_scale: opts.render_scale,
                            interactive: opts.interactive,
                            background: opts.clear,
                        },
                        name: wp.name.clone(),
                    }),
                    wallpaper: wp,
                })
            }
            (WallpaperKind::Video, Source::File(path)) => {
                let opts: VideoOptions =
                    self.layers
                        .type_options(wp.kind, wp.options.as_ref(), cfg)?;
                Ok(Prepared::Video {
                    path: path.clone(),
                    params: VideoParams::video(&opts, cfg.volume, cfg.mute),
                    wallpaper: wp,
                })
            }
            (kind, _) => Err(anyhow!("{} wallpapers are not implemented yet", kind.key())),
        }
    }

    fn try_load(&mut self, idx: usize, spec: &str, purpose: Purpose) -> Result<()> {
        if purpose == Purpose::Main {
            // 실패해도 `list`에 무엇을 시도했는지 보이도록 먼저 기록한다.
            let resolved = wallpaper::resolve(spec, &self.config.detect, &self.layers.base_dir());
            self.outputs[idx].wallpaper = Some(match resolved {
                Ok(wp) => info(&wp),
                Err(_) => WallpaperInfo {
                    path: spec.to_owned(),
                    kind: None,
                    name: spec.to_owned(),
                },
            });
        }
        let prepared = self.prepare(&self.outputs[idx].cfg, spec)?;
        let wp = prepared.wallpaper();
        tracing::info!(output = %self.outputs[idx].name, wallpaper = %wp.name, kind = ?wp.kind, ?purpose, "loading");
        let (path, opts) = match prepared {
            Prepared::Image { path, opts, .. } => (path, opts),
            Prepared::Gpu { spec, .. } => return self.show_gpu(idx, *spec, purpose),
            Prepared::Video {
                wallpaper,
                path,
                params,
            } => return self.show_video(idx, &path, wallpaper.name, params, purpose),
        };
        let out = &self.outputs[idx];
        let request = image::Request {
            path,
            target: out.physical().context("output not configured yet")?,
            fit: opts.fit,
            downscale: opts.downscale.then_some(opts.downscale_filter),
        };
        self.spawn_image(idx, request, opts, purpose);
        Ok(())
    }

    fn spawn_image(
        &mut self,
        idx: usize,
        request: image::Request,
        opts: ImageOptions,
        purpose: Purpose,
    ) {
        let out = &mut self.outputs[idx];
        let pending = Pending {
            generation: out.next_generation(),
            purpose,
        };
        if purpose == Purpose::Loading {
            out.loading = Some(pending);
        } else {
            out.pending = Some(pending);
            out.incoming = None;
        }
        let (output, tx) = (out.id, self.images.clone());
        let req = request.clone();
        image::spawn(request, move |result| {
            let _ = tx.send(ImageDone {
                output,
                pending,
                request: req,
                opts,
                result,
            });
        });
    }

    fn on_image(&mut self, done: ImageDone) {
        let Some(idx) = self.index_by_id(done.output) else {
            return;
        };
        let out = &mut self.outputs[idx];
        let current = match done.pending.purpose {
            // 로딩 화면은 본 배경이 아직 준비 중일 때만 의미가 있다.
            Purpose::Loading => {
                out.loading
                    .is_some_and(|p| p.generation == done.pending.generation)
                    && out.pending.is_some()
            }
            _ => out
                .pending
                .is_some_and(|p| p.generation == done.pending.generation),
        };
        if !current {
            return;
        }
        if done.pending.purpose == Purpose::Loading {
            out.loading = None;
        } else {
            out.pending = None;
            out.loading = None;
        }
        let purpose = done.pending.purpose;
        let shown = done.result.and_then(|decoded| {
            let ctx = ctx!(self);
            self.outputs[idx].present_image(&ctx, &decoded, done.request, done.opts)
        });
        match shown {
            Ok(()) if purpose == Purpose::Main => self.settle(idx, Ok(())),
            Ok(()) => {}
            Err(e) => self.failed(idx, purpose, e),
        }
    }

    fn show_loading(&mut self, idx: usize) {
        match self.outputs[idx].cfg.appearance.loading.clone() {
            FillOrPrevious::Previous => {
                if matches!(self.outputs[idx].content, Content::Nothing) {
                    // 이전 내용이 없으면 빈 화면 설정이 색일 때만 바로 보여준다.
                    if let Fill::Color(c) = self.outputs[idx].cfg.appearance.empty {
                        self.apply_fill(idx, Fill::Color(c), Purpose::Loading);
                    }
                }
            }
            FillOrPrevious::Fill(fill) => self.apply_fill(idx, fill, Purpose::Loading),
        }
    }

    /// 실패 시 대체 화면: Main → appearance.error → appearance.empty → 아무것도 안 함.
    fn failed(&mut self, idx: usize, purpose: Purpose, err: anyhow::Error) {
        let out = &mut self.outputs[idx];
        tracing::error!(output = %out.name, ?purpose, "{err:#}");
        if purpose == Purpose::Main {
            let msg = format!("{}: {err:#}", out.name);
            out.error = Some(format!("{err:#}"));
            self.settle(idx, Err(msg));
        }
        let out = &self.outputs[idx];
        let appearance = out.cfg.appearance.clone();
        match purpose {
            Purpose::Main => match appearance.error {
                FillOrPrevious::Previous if !matches!(out.content, Content::Nothing) => {}
                FillOrPrevious::Previous => self.apply_fill(idx, appearance.empty, Purpose::Empty),
                FillOrPrevious::Fill(fill) => self.apply_fill(idx, fill, Purpose::Error),
            },
            Purpose::Error => self.apply_fill(idx, appearance.empty, Purpose::Empty),
            Purpose::Empty | Purpose::Loading => {}
        }
    }

    // ── IPC ──────────────────────────────────────────────────────────────

    /// 클라이언트 버퍼에 쌓인 요청을 순서대로 처리한다.
    fn pump(&mut self, client: u64) {
        while let Some(line) = self.clients.next_line(client) {
            tracing::debug!(client, request = %line, "IPC request");
            let handled = match serde_json::from_str::<Request>(&line) {
                Ok(req) => self.handle(client, req),
                Err(e) => Handled::Reply(Response::error(format!("invalid request: {e}"))),
            };
            match handled {
                Handled::Reply(resp) => self.clients.reply(client, &resp),
                Handled::Wait => {
                    self.clients.set_busy(client, true);
                    break;
                }
            }
        }
        self.clients.reap(client);
    }

    fn handle(&mut self, client: u64, req: Request) -> Handled {
        let result = match req {
            Request::Set { output, path, save } => {
                match self.cmd_set(client, output.as_deref(), path, save) {
                    Ok(()) => return Handled::Wait,
                    Err(e) => Err(e),
                }
            }
            Request::Pause { output } => self.cmd_pause(output.as_deref(), true),
            Request::Resume { output } => self.cmd_pause(output.as_deref(), false),
            Request::Volume {
                output,
                volume,
                mute,
                save,
            } => self.cmd_volume(output.as_deref(), volume, mute, save),
            Request::List => self.cmd_list(),
            Request::Reload => self.cmd_reload(),
            Request::Quit => {
                tracing::info!("quit requested");
                self.loop_signal.stop();
                Ok(Response::ok())
            }
        };
        Handled::Reply(result.unwrap_or_else(|e| Response::error(format!("{e:#}"))))
    }

    fn targets(&self, output: Option<&str>) -> Result<Vec<usize>> {
        match output {
            None => Ok((0..self.outputs.len()).collect()),
            Some(name) => match self.outputs.iter().position(|o| o.name == name) {
                Some(i) => Ok(vec![i]),
                None => bail!("unknown output: {name}"),
            },
        }
    }

    fn save(&self, target: Target<'_>, values: &[(&str, toml_edit::Value)]) -> Result<()> {
        let path = self
            .layers
            .config_path()
            .context("no config file location (set XDG_CONFIG_HOME or HOME, or use --config)")?;
        config_edit::save(path, target, values)?;
        tracing::info!(path = %path.display(), "config saved");
        Ok(())
    }

    fn cmd_set(
        &mut self,
        client: u64,
        output: Option<&str>,
        path: String,
        save: bool,
    ) -> Result<()> {
        let targets = self.targets(output)?;
        // 하나라도 안 되면 아무것도 바꾸지 않는다.
        for &idx in &targets {
            let out = &self.outputs[idx];
            self.prepare(&out.cfg, &path)
                .with_context(|| out.name.clone())?;
        }
        if save {
            let target = output.map_or(Target::Default, Target::Output);
            self.save(target, &[("wallpaper", path.as_str().into())])?;
        }
        self.runtime.set(output, runtime::wallpaper, path);

        let waiter = self.next_waiter;
        self.next_waiter += 1;
        self.waiters.insert(
            waiter,
            Waiter {
                client,
                remaining: targets.len(),
                errors: Vec::new(),
            },
        );
        if targets.is_empty() {
            self.finished.push(waiter);
        }
        for idx in targets {
            let name = self.outputs[idx].name.clone();
            self.outputs[idx].cfg = self.output_config(&name)?;
            self.outputs[idx].waiters.push(waiter);
            self.refresh(idx);
        }
        Ok(())
    }

    /// 출력의 배경 로딩이 끝났다: 기다리던 요청에 결과를 알린다.
    fn settle(&mut self, idx: usize, result: Result<(), String>) {
        for id in std::mem::take(&mut self.outputs[idx].waiters) {
            let Some(w) = self.waiters.get_mut(&id) else {
                continue;
            };
            if let Err(e) = &result {
                w.errors.push(e.clone());
            }
            w.remaining -= 1;
            if w.remaining == 0 {
                self.finished.push(id);
            }
        }
    }

    /// 디스패치마다 호출: 끝난 `set`에 응답하고, 그 클라이언트의 다음 요청을 처리한다.
    fn flush_waiters(&mut self) {
        while let Some(id) = self.finished.pop() {
            let Some(w) = self.waiters.remove(&id) else {
                continue;
            };
            let resp = if w.errors.is_empty() {
                Response::ok()
            } else {
                Response::error(w.errors.join("; "))
            };
            self.clients.reply(w.client, &resp);
            self.clients.set_busy(w.client, false);
            self.pump(w.client);
        }
    }

    fn cmd_pause(&mut self, output: Option<&str>, paused: bool) -> Result<Response> {
        let targets = self.targets(output)?;
        self.runtime.set(output, runtime::paused, paused);
        for idx in targets {
            let out = &mut self.outputs[idx];
            out.user_paused = paused;
            tracing::info!(output = %out.name, paused, "user pause");
            self.apply_pause(idx);
        }
        Ok(Response::ok())
    }

    // ── 영상 ──────────────────────────────────────────────────────────────

    fn show_video(
        &mut self,
        idx: usize,
        path: &std::path::Path,
        name: String,
        params: VideoParams,
        purpose: Purpose,
    ) -> Result<()> {
        if purpose == Purpose::Loading {
            bail!("a video cannot be used as appearance.loading");
        }
        let formats = self.dmabuf_formats();
        // hwdec이면 dmabuf부터 시도하고, 컴포지터가 dmabuf를 안 받으면 바로 shm 폴백.
        let mode = match (params.hwdec, formats.is_empty()) {
            (true, false) => VideoMode::Dmabuf,
            (true, true) => VideoMode::Shm { fallback: true },
            (false, _) => VideoMode::Shm { fallback: false },
        };
        self.start_video(idx, path, name, params, mode, purpose)
    }

    fn start_video(
        &mut self,
        idx: usize,
        path: &std::path::Path,
        name: String,
        params: VideoParams,
        mode: VideoMode,
        purpose: Purpose,
    ) -> Result<()> {
        let out = &self.outputs[idx];
        let logical = out.logical.context("output not configured yet")?;
        let plane = DesyncPlane::new(&ctx!(self), out.surface())?;
        let id = self.next_view;
        self.next_view += 1;
        let view = VideoView::new(
            path,
            name,
            params,
            mode,
            &self.dmabuf_formats(),
            plane,
            logical,
            self.videos.clone(),
            out.id,
            id,
        )?;
        let out = &mut self.outputs[idx];
        let generation = out.next_generation();
        out.pending = Some(Pending {
            generation,
            purpose,
        });
        out.incoming = Some(Box::new(view));
        Ok(())
    }

    fn on_video(&mut self, msg: VideoMsg) {
        let Some(idx) = self.index_by_id(msg.output) else {
            return;
        };
        let out = &mut self.outputs[idx];
        let paused = out.is_paused();
        // 감시용 frame 콜백은 화면에 붙은 영상에만, 기다리는 것이 없을 때 하나씩.
        let request_frame = !paused && !out.watch.pending();
        let incoming = out.incoming.as_ref().is_some_and(|v| v.id == msg.view);
        let frame_requested =
            request_frame && !incoming && matches!(msg.event, VideoEvent::Frame(_));
        let view = match (&mut out.content, &mut out.incoming) {
            (_, Some(v)) if v.id == msg.view => v,
            (Content::Video(v), _) if v.id == msg.view => v,
            _ => return,
        };
        let result = match msg.event {
            VideoEvent::Frame(sample) => {
                let ctx = ctx!(self);
                view.present(&ctx, &self.dmabuf, sample, request_frame && !incoming)
            }
            VideoEvent::AsyncDone => view.on_async_done(paused),
            VideoEvent::SegmentDone => view.on_segment_done(),
            VideoEvent::Eos => {
                tracing::debug!(video = %view.name, "end of stream");
                Ok(())
            }
            VideoEvent::Error {
                message,
                not_negotiated,
            } => {
                if not_negotiated && view.mode == VideoMode::Dmabuf {
                    // 하드웨어 디코더가 없거나 컴포지터와 포맷이 안 맞음: 복사 경로로 다시.
                    tracing::warn!(video = %view.name, "dmabuf path failed ({message}), falling back to shm");
                    let (path, name, params) = (
                        view.path().to_owned(),
                        view.name.clone(),
                        view.params.clone(),
                    );
                    let purpose = out.pending.map_or(Purpose::Main, |p| p.purpose);
                    let r = self.start_video(
                        idx,
                        &path,
                        name,
                        params,
                        VideoMode::Shm { fallback: true },
                        purpose,
                    );
                    if let Err(e) = r {
                        self.video_failed(idx, incoming, e);
                    }
                    return;
                }
                Err(anyhow!("{message}"))
            }
        };
        if let Err(e) = result {
            self.video_failed(idx, incoming, e);
            return;
        }
        if frame_requested {
            self.outputs[idx].watch.requested(Instant::now());
            self.arm_watch(idx);
        }
        // 첫 프레임이 붙었으면 이전 화면과 바꾼다.
        let out = &mut self.outputs[idx];
        if incoming && out.incoming.as_ref().is_some_and(|v| v.shown_first) {
            let view = out.incoming.take().expect("checked above");
            let purpose = out.pending.take().map_or(Purpose::Main, |p| p.purpose);
            out.loading = None;
            let ctx = ctx!(self);
            if let Err(e) = out.present_video(&ctx, view) {
                self.failed(idx, purpose, e);
                return;
            }
            tracing::info!(output = %out.name, path = ?out.render_path(), "video playing");
            if purpose == Purpose::Main {
                self.settle(idx, Ok(()));
            }
        }
    }

    fn video_failed(&mut self, idx: usize, incoming: bool, err: anyhow::Error) {
        let out = &mut self.outputs[idx];
        if incoming {
            out.incoming = None;
            let purpose = out.pending.take().map_or(Purpose::Main, |p| p.purpose);
            self.failed(idx, purpose, err);
        } else {
            // 재생 중 오류: 마지막 프레임을 그대로 두고 상태만 알린다.
            tracing::error!(output = %out.name, "{err:#}");
            out.error = Some(format!("{err:#}"));
        }
    }

    /// 컴포지터가 놓은 dmabuf 버퍼의 샘플을 GStreamer에 돌려준다.
    pub fn video_buffer_released(&mut self, buffer: &WlBuffer) {
        for out in &mut self.outputs {
            let views = [
                match &mut out.content {
                    Content::Video(v) => Some(v),
                    _ => None,
                },
                out.incoming.as_mut(),
            ];
            for v in views.into_iter().flatten() {
                if v.released(buffer) {
                    return;
                }
            }
        }
    }

    // ── 셰이더·씬 ─────────────────────────────────────────────────────────

    fn assets(&self) -> Assets {
        Assets::new(expand_path(
            &self.config.assets.dir,
            &self.layers.base_dir(),
        ))
    }

    fn show_gpu(&mut self, idx: usize, spec: GpuSpec, purpose: Purpose) -> Result<()> {
        let out = &self.outputs[idx];
        let geometry = Geometry {
            logical: out.logical.context("output not configured yet")?,
            physical: out.physical().context("output not configured yet")?,
        };
        let plane = DesyncPlane::new(&ctx!(self), out.surface())?;
        let surface = match &self.gpu {
            Some(gpu) => gpu.create_surface(&plane.surface)?,
            None => {
                let display = NonNull::new(self.conn.backend().display_ptr().cast::<c_void>())
                    .context("no wl_display pointer (wayland-backend without client_system?)")?;
                let (gpu, surface) =
                    Gpu::new(&self.config.render, &self.assets(), display, &plane.surface)?;
                self.gpu = Some(gpu);
                surface
            }
        };
        let gpu = self.gpu.as_ref().context("GPU disappeared")?;
        let id = self.next_view;
        self.next_view += 1;
        let paused = self.outputs[idx].is_paused();
        let view = GpuView::new(
            gpu,
            surface,
            plane,
            spec,
            &self.config.render,
            geometry,
            id,
            paused,
        )?;
        let ctx = ctx!(self);
        let out = &mut self.outputs[idx];
        if purpose != Purpose::Loading {
            out.next_generation();
            out.pending = None;
            out.loading = None;
        }
        out.present_gpu(&ctx, view)?;
        // 멈춘 상태여도 첫 장은 그린다(시간 0에 고정된 화면).
        self.draw_gpu(idx);
        if purpose == Purpose::Main {
            self.settle(idx, Ok(()));
        }
        Ok(())
    }

    fn draw_gpu(&mut self, idx: usize) {
        let Some(gpu) = &self.gpu else { return };
        let out = &mut self.outputs[idx];
        let Content::Gpu(view) = &mut out.content else {
            return;
        };
        match view.draw(gpu, &self.qh) {
            // draw는 항상 frame 콜백을 요청한다.
            Ok(()) => {
                out.watch.requested(Instant::now());
                self.arm_watch(idx);
            }
            Err(e) => {
                tracing::error!(output = %out.name, "{e:#}");
                out.error = Some(format!("{e:#}"));
            }
        }
    }

    fn gpu_resized(&mut self, idx: usize) {
        let Some(gpu) = &self.gpu else { return };
        let out = &mut self.outputs[idx];
        let (Some(logical), Some(physical)) = (out.logical, out.physical()) else {
            return;
        };
        let Content::Gpu(view) = &mut out.content else {
            return;
        };
        if let Err(e) = view.resize(gpu, Geometry { logical, physical }) {
            tracing::error!(output = %out.name, "{e:#}");
            return;
        }
        // 새 크기는 다음 present에 반영된다. 루프가 멈춰 있으면 지금 한 장 그린다.
        if !view.waiting_frame && !view.timer_armed {
            self.draw_gpu(idx);
        }
    }

    /// GPU/영상 서브서피스의 frame 콜백: 가림 판정을 갱신하고, GPU 배경은 fps 제한을 지키며 다음 프레임을 그린다.
    pub fn on_frame(&mut self, surface: &WlSurface) {
        let Some(idx) = self
            .outputs
            .iter()
            .position(|o| o.live_surface() == Some(surface))
        else {
            return;
        };
        let now = Instant::now();
        let vis = &self.config.visibility;
        let timeout = Duration::from_millis(vis.frame_timeout_ms);
        let out = &mut self.outputs[idx];
        let change = out.watch.on_callback(now, timeout, vis.slow_frame_count);
        if change != Change::None {
            tracing::info!(output = %out.name, ?change, "frame callbacks");
            self.apply_pause(idx);
        }
        let out = &mut self.outputs[idx];
        if out.watch.hidden() {
            // 아직 가려져 있다: 다시 보이는지 알기 위해 빈 커밋 + 콜백 하나만 걸어 둔다.
            surface.frame(&self.qh, FrameCallbackData(surface.clone()));
            surface.commit();
            out.watch.requested(now);
        }
        let paused = out.is_paused();
        let Content::Gpu(view) = &mut out.content else {
            return;
        };
        view.waiting_frame = false;
        // 다 움직인 씬은 여기서 잠든다. 포인터·크기 변경·resume이 다시 깨운다.
        if paused || view.timer_armed || !view.wants_frames() {
            return;
        }
        let Some(at) = view.due(Instant::now()) else {
            self.draw_gpu(idx);
            return;
        };
        view.timer_armed = true;
        let (output, view_id) = (out.id, view.id);
        let timer = self
            .handle
            .insert_source(Timer::from_deadline(at), move |_, _, state| {
                state.on_timer(output, view_id);
                TimeoutAction::Drop
            });
        if let Err(e) = timer {
            tracing::error!("failed to arm frame timer: {e}");
            view.timer_armed = false;
        }
    }

    fn on_timer(&mut self, output: u64, view_id: u64) {
        let Some(idx) = self.index_by_id(output) else {
            return;
        };
        let out = &mut self.outputs[idx];
        let paused = out.is_paused();
        let Content::Gpu(view) = &mut out.content else {
            return;
        };
        if view.id != view_id {
            return;
        }
        view.timer_armed = false;
        if !paused {
            self.draw_gpu(idx);
        }
    }

    // ── 가림 감지 ─────────────────────────────────────────────────────────

    /// 사유가 바뀌었을 수 있다: 내용의 재생 상태를 맞춘다.
    fn apply_pause(&mut self, idx: usize) {
        let out = &mut self.outputs[idx];
        let paused = out.is_paused();
        if paused == out.paused_applied {
            return;
        }
        out.paused_applied = paused;
        // 다른 사유로 멈추면 콜백을 요청하지 않으니 Hidden 판정도 처음부터 다시 한다.
        if out.user_paused || out.covered_toplevel || out.covered_niri {
            out.watch.reset();
        }
        tracing::info!(
            output = %out.name, paused, user = out.user_paused, hidden = out.watch.hidden(),
            covered = out.covered_toplevel || out.covered_niri, "pause state"
        );
        if let Content::Video(view) = &mut out.content {
            view.set_paused(paused);
        }
        if let Some(view) = &mut out.incoming {
            view.set_paused(paused);
        }
        if let Content::Gpu(view) = &mut out.content {
            view.set_paused(paused);
            // 멈춰 있던 렌더 루프는 한 장 그려서 다시 돌린다.
            if !paused && !view.waiting_frame && !view.timer_armed {
                self.draw_gpu(idx);
            }
        }
    }

    /// toplevel/niri 상태나 정책이 바뀌었다: 모든 출력의 Covered를 다시 계산한다.
    pub fn visibility_changed(&mut self) {
        let policy = visibility::effective_policy(self.config.pause_when, self.niri_active);
        let toplevels = self.toplevel_states();
        for idx in 0..self.outputs.len() {
            let out = &mut self.outputs[idx];
            out.covered_toplevel = visibility::covered_by_toplevels(policy, &out.name, &toplevels);
            out.covered_niri = self
                .niri
                .get(&out.name)
                .is_some_and(|&busy| visibility::covered_by_niri(policy, busy));
            self.apply_pause(idx);
        }
    }

    /// 요청한 frame 콜백이 제때 오는지 확인할 타이머.
    fn arm_watch(&mut self, idx: usize) {
        let timeout = Duration::from_millis(self.config.visibility.frame_timeout_ms);
        let out = &mut self.outputs[idx];
        if out.watch_timer {
            return;
        }
        let Some(at) = out.watch.deadline(timeout) else {
            return;
        };
        let id = out.id;
        let timer = self
            .handle
            .insert_source(Timer::from_deadline(at), move |_, _, state| {
                state.on_watch_timer(id);
                TimeoutAction::Drop
            });
        match timer {
            Ok(_) => out.watch_timer = true,
            Err(e) => tracing::error!("failed to arm visibility timer: {e}"),
        }
    }

    fn on_watch_timer(&mut self, output: u64) {
        let Some(idx) = self.index_by_id(output) else {
            return;
        };
        let timeout = Duration::from_millis(self.config.visibility.frame_timeout_ms);
        let out = &mut self.outputs[idx];
        out.watch_timer = false;
        if out.watch.on_timer(Instant::now(), timeout) == Change::Hidden {
            tracing::info!(output = %out.name, "no frame callbacks: hidden");
            self.apply_pause(idx);
        } else {
            // 그 사이 새로 요청한 콜백이 있으면 그 기한으로 다시 건다.
            self.arm_watch(idx);
        }
    }

    fn on_niri(&mut self, msg: NiriMsg) {
        match msg {
            NiriMsg::Outputs(map) => {
                tracing::debug!(?map, "niri outputs");
                self.niri = map;
            }
            NiriMsg::Gone(why) => {
                tracing::warn!("niri IPC disconnected ({why}); niri visibility source off");
                self.niri.clear();
                self.niri_active = false;
            }
        }
        self.visibility_changed();
    }

    /// layer surface 기준 포인터 이벤트.
    pub fn pointer_event(&mut self, surface: &WlSurface, kind: &PointerEventKind, pos: (f64, f64)) {
        let Some(idx) = self.index_by_surface(surface) else {
            return;
        };
        let out = &mut self.outputs[idx];
        let paused = out.is_paused();
        if let Content::Gpu(view) = &mut out.content {
            let redraw = view.pointer(kind, pos);
            // 렌더 루프가 멈춰 있으면(잠든 씬) 한 장 그려서 다시 돌린다.
            if redraw && !paused && !view.waiting_frame && !view.timer_armed {
                self.draw_gpu(idx);
            }
        }
    }

    /// 디스패치마다: 끝난 요청에 응답하고, 쓰는 곳이 없어진 GPU를 해제한다.
    fn after_dispatch(&mut self) {
        self.flush_waiters();
        if self.gpu.is_some()
            && !self
                .outputs
                .iter()
                .any(|o| matches!(o.content, Content::Gpu(_)))
        {
            self.gpu = None;
            tracing::info!("GPU released (no shader or scene outputs)");
        }
    }

    fn cmd_volume(
        &mut self,
        output: Option<&str>,
        volume: Option<u8>,
        mute: Option<bool>,
        save: bool,
    ) -> Result<Response> {
        if volume.is_none() && mute.is_none() {
            bail!("nothing to change: give a volume and/or mute");
        }
        if let Some(v) = volume
            && v > 100
        {
            bail!("volume must be 0-100, got {v}");
        }
        let targets = self.targets(output)?;
        if save {
            let mut values: Vec<(&str, toml_edit::Value)> = Vec::new();
            if let Some(v) = volume {
                values.push(("volume", i64::from(v).into()));
            }
            if let Some(m) = mute {
                values.push(("mute", m.into()));
            }
            let target = output.map_or(Target::TopLevel, Target::Output);
            self.save(target, &values)?;
        }
        if let Some(v) = volume {
            self.runtime.set(output, runtime::volume, v);
        }
        if let Some(m) = mute {
            self.runtime.set(output, runtime::mute, m);
        }
        for idx in targets {
            let name = self.outputs[idx].name.clone();
            let cfg = self.output_config(&name)?;
            let out = &mut self.outputs[idx];
            out.cfg.volume = cfg.volume;
            out.cfg.mute = cfg.mute;
            if let Content::Video(v) = &mut out.content {
                v.set_volume(cfg.volume, cfg.mute);
            }
            if let Some(v) = &mut out.incoming {
                v.set_volume(cfg.volume, cfg.mute);
            }
            tracing::info!(output = %name, volume = cfg.volume, mute = cfg.mute, "volume");
        }
        Ok(Response::ok())
    }

    fn cmd_list(&mut self) -> Result<Response> {
        let outputs = self.outputs.iter_mut().map(status).collect();
        Ok(Response::data(serde_json::to_value(ListData { outputs })?))
    }

    fn cmd_reload(&mut self) -> Result<Response> {
        let layers = Layers::load(self.config_arg.as_deref())?;
        let config = layers.global()?;
        let content_changed = Table::try_from(&config.detect)?
            != Table::try_from(&self.config.detect)?
            || Table::try_from(&config.types)? != Table::try_from(&self.config.types)?;
        let surface_changed = config.surface != self.config.surface;
        if config.log.level != self.config.log.level
            || config.cursor.theme != self.config.cursor.theme
            || config.cursor.size != self.config.cursor.size
        {
            tracing::warn!("[log] and [cursor].theme/size take effect after a daemon restart");
        }
        if config.visibility.foreign_toplevel != self.config.visibility.foreign_toplevel
            || config.visibility.niri != self.config.visibility.niri
        {
            tracing::warn!("[visibility].foreign_toplevel/niri take effect after a daemon restart");
        }
        self.layers = layers;
        self.config = config;
        self.runtime.clear_settings();
        tracing::info!(config = ?self.layers.config_path(), "reloaded");
        // pause_when이 바뀌었을 수 있다.
        self.visibility_changed();

        if surface_changed {
            // layer/namespace는 surface 생성 시에만 정할 수 있다.
            let wl_outputs: Vec<WlOutput> =
                self.outputs.iter().map(|o| o.wl_output.clone()).collect();
            while !self.outputs.is_empty() {
                self.drop_output(0);
            }
            for o in wl_outputs {
                self.add_output(o);
            }
            return Ok(Response::ok());
        }
        for idx in 0..self.outputs.len() {
            let name = self.outputs[idx].name.clone();
            let cfg = self.output_config(&name)?;
            let changed = content_changed || cfg != self.outputs[idx].cfg;
            self.outputs[idx].cfg = cfg;
            if changed {
                self.refresh(idx);
            }
        }
        Ok(Response::ok())
    }
}

fn info(wp: &Wallpaper) -> WallpaperInfo {
    WallpaperInfo {
        path: match &wp.source {
            Source::File(p) => p.display().to_string(),
            Source::Url(u) => u.clone(),
        },
        kind: Some(wp.kind),
        name: wp.name.clone(),
    }
}

fn status(out: &mut Output) -> OutputStatus {
    let (width, height) = out.physical().unwrap_or_default();
    let pause_reasons: Vec<PauseReason> = [
        (out.user_paused, Reason::User, ReasonSource::Cli),
        (out.watch.hidden(), Reason::Hidden, ReasonSource::Frame),
        (
            out.covered_toplevel,
            Reason::Covered,
            ReasonSource::ForeignToplevel,
        ),
        (out.covered_niri, Reason::Covered, ReasonSource::Niri),
    ]
    .into_iter()
    .filter(|(on, ..)| *on)
    .map(|(_, reason, source)| PauseReason { reason, source })
    .collect();
    let state = if out.error.is_some() {
        PlayState::Error
    } else if out.cfg.wallpaper.is_none() {
        PlayState::Idle
    } else if out.loading_main() {
        PlayState::Loading
    } else if !pause_reasons.is_empty() {
        PlayState::Paused
    } else {
        PlayState::Playing
    };
    OutputStatus {
        name: out.name.clone(),
        description: out.description.clone(),
        width,
        height,
        scale: out.scale120 as f64 / SCALE_DENOMINATOR as f64,
        wallpaper: out.wallpaper.clone(),
        render_path: out.render_path(),
        fps: match &mut out.content {
            Content::Gpu(view) => Some(view.take_fps()),
            Content::Video(view) => Some(view.take_fps()),
            _ => None,
        },
        volume: out.cfg.volume,
        mute: out.cfg.mute,
        state,
        pause_reasons,
        error: out.error.clone(),
    }
}

fn layer_kind(k: LayerKind) -> Layer {
    match k {
        LayerKind::Background => Layer::Background,
        LayerKind::Bottom => Layer::Bottom,
        LayerKind::Top => Layer::Top,
        LayerKind::Overlay => Layer::Overlay,
    }
}

fn is_gif(path: &std::path::Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gif"))
}

/// niri IPC 신호원을 연결한다. 실패하면 경고만 남기고 `false`.
fn start_niri(handle: &LoopHandle<'static, State>) -> bool {
    let (tx, rx): (Sender<NiriMsg>, Channel<NiriMsg>) = channel();
    let inserted = handle.insert_source(rx, |ev, _, state| {
        if let ChannelEvent::Msg(msg) = ev {
            state.on_niri(msg);
        }
    });
    if let Err(e) = inserted {
        tracing::warn!("failed to insert niri channel: {e}");
        return false;
    }
    match niri::spawn(tx) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("niri visibility source unavailable: {e:#}");
            false
        }
    }
}
