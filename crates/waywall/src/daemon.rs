//! 데몬: 이벤트 루프와 출력별 내용 결정.

use anyhow::{Context, Result, anyhow};
use calloop::channel::{Channel, Event as ChannelEvent, Sender, channel};
use calloop::signals::{Signal, Signals};
use calloop::{EventLoop, LoopSignal};
use calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::compositor::CompositorState;
use smithay_client_toolkit::output::OutputState;
use smithay_client_toolkit::registry::RegistryState;
use smithay_client_toolkit::seat::SeatState;
use smithay_client_toolkit::seat::pointer::ThemedPointer;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell};
use smithay_client_toolkit::shm::Shm;
use smithay_client_toolkit::subcompositor::SubcompositorState;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, QueueHandle};
use waywall_proto::config::{Config, Fill, FillOrPrevious, ImageOptions, LayerKind};
use waywall_proto::project::WallpaperKind;

use crate::backend::image::{self, Decoded};
use crate::config::Layers;
use crate::wallpaper::{self, Source};
use crate::wayland::globals::Globals;
use crate::wayland::output::{Content, Ctx, Output, Pending, Purpose, SCALE_DENOMINATOR};

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

    pub layers: Layers,
    pub config: Config,
    pub outputs: Vec<Output>,
    pub pointers: Vec<ThemedPointer>,
    next_output_id: u64,
    images: Sender<ImageDone>,
    loop_signal: LoopSignal,
}

/// 디코딩 스레드 → 메인 루프.
pub struct ImageDone {
    output: u64,
    pending: Pending,
    request: image::Request,
    opts: ImageOptions,
    result: Result<Decoded>,
}

pub fn run(layers: Layers) -> Result<()> {
    let config = layers.global()?;
    let conn =
        Connection::connect_to_env().context("failed to connect to the Wayland compositor")?;
    let (globals, queue) = registry_queue_init::<State>(&conn).context("wl_registry")?;
    let qh = queue.handle();

    let mut event_loop = EventLoop::<State>::try_new()?;
    let handle = event_loop.handle();
    WaylandSource::new(conn.clone(), queue)
        .insert(handle.clone())
        .map_err(|e| anyhow!("failed to insert Wayland source: {e}"))?;

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
        layers,
        config,
        outputs: Vec::new(),
        pointers: Vec::new(),
        next_output_id: 0,
        images,
        loop_signal: event_loop.get_signal(),
    };

    tracing::info!(config = ?state.layers.config_path(), "waywall daemon started");
    event_loop.run(None, &mut state, |_| {})?;
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

    pub fn add_output(&mut self, wl_output: WlOutput) {
        let Some(info) = self.output_state.info(&wl_output) else {
            return;
        };
        let name = info
            .name
            .clone()
            .unwrap_or_else(|| format!("output-{}", info.id));
        let cfg = match self.layers.output(&name) {
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
        let output = Output::new(&ctx!(self), id, wl_output, name, layer, scale120, cfg);
        self.outputs.push(output);
    }

    pub fn remove_output(&mut self, wl_output: &WlOutput) {
        self.outputs.retain(|o| {
            let keep = &o.wl_output != wl_output;
            if !keep {
                tracing::info!(output = %o.name, "output removed");
            }
            keep
        });
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

    /// 설정에 따라 출력에 보일 내용을 정한다.
    fn show_configured(&mut self, idx: usize) {
        match self.outputs[idx].cfg.wallpaper.clone() {
            Some(spec) => self.load(idx, &spec, Purpose::Main),
            None => {
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

    fn try_load(&mut self, idx: usize, spec: &str, purpose: Purpose) -> Result<()> {
        let base = self.layers.base_dir();
        let wp = wallpaper::resolve(spec, &self.config.detect, &base)?;
        let out = &self.outputs[idx];
        tracing::info!(output = %out.name, wallpaper = %wp.name, kind = ?wp.kind, ?purpose, "loading");
        match (wp.kind, &wp.source) {
            (WallpaperKind::Image, Source::File(path)) => {
                let opts: ImageOptions =
                    self.layers
                        .type_options(wp.kind, wp.options.as_ref(), &out.cfg)?;
                let request = image::Request {
                    path: path.clone(),
                    target: out.physical().context("output not configured yet")?,
                    fit: opts.fit,
                    downscale: opts.downscale.then_some(opts.downscale_filter),
                };
                self.spawn_image(idx, request, opts, purpose);
                Ok(())
            }
            (kind, _) => Err(anyhow!("{} wallpapers are not implemented yet", kind.key())),
        }
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
        match done.result {
            Ok(decoded) => {
                let ctx = ctx!(self);
                let out = &mut self.outputs[idx];
                if let Err(e) = out.present_image(&ctx, &decoded, done.request, done.opts) {
                    self.failed(idx, done.pending.purpose, e);
                }
            }
            Err(e) => self.failed(idx, done.pending.purpose, e),
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
        let out = &self.outputs[idx];
        tracing::error!(output = %out.name, ?purpose, "{err:#}");
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
}

fn layer_kind(k: LayerKind) -> Layer {
    match k {
        LayerKind::Background => Layer::Background,
        LayerKind::Bottom => Layer::Bottom,
        LayerKind::Top => Layer::Top,
        LayerKind::Overlay => Layer::Overlay,
    }
}
