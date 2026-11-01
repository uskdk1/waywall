//! 영상(과 움직이는 GIF): GStreamer 재생기 + 출력별 서브서피스
//! (docs/architecture.md#영상-복사-없는-dmabuf-경로, docs/wallpaper-format.md#동영상-배경).
//!
//! GStreamer 스레드는 샘플과 버스 메시지를 calloop 채널로 넘기기만 하고, Wayland 객체는 전부 메인 스레드에서 다룬다.

use std::collections::HashMap;
use std::os::fd::BorrowedFd;
use std::path::Path;
use std::sync::Once;
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use calloop::channel::Sender;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_allocators as gst_alloc;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use smithay_client_toolkit::dmabuf::DmabufState;
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use wayland_client::Proxy;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_shm;
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::Flags;
use waywall_proto::Color;
use waywall_proto::config::{Fit, ImageOptions, VideoOptions};
use waywall_proto::ipc::RenderPath;

use crate::fit::place;
use crate::wayland::dmabuf::DrmFormat;
use crate::wayland::output::{Ctx, DesyncPlane};

// playbin의 GstPlayFlags (인터페이스 값).
const FLAG_VIDEO: u32 = 0x1;
const FLAG_AUDIO: u32 = 0x2;
const FLAG_SOFT_VOLUME: u32 = 0x10;
const FLAG_NATIVE_VIDEO: u32 = 0x40;
const FLAG_FORCE_SW_DECODERS: u32 = 0x1000;
/// wl_shm 폴백 경로의 픽셀 포맷: 메모리 순서 BGRx = wl_shm XRGB8888 (little-endian).
const SHM_CAPS: &str = "video/x-raw, format=BGRx";
const BYTES_PER_PIXEL: u32 = 4;

/// 영상 배경과 GIF가 공유하는 재생 설정.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoParams {
    pub fit: Fit,
    pub background: Color,
    pub interactive: bool,
    pub looping: bool,
    pub audio: bool,
    pub speed: f64,
    pub hwdec: bool,
    pub volume: u8,
    pub mute: bool,
    /// 반복을 비flush segment seek로 이어 붙일지. libav GIF 디먹서는 이를 처리하지 못해 flush seek로 반복한다.
    pub gapless: bool,
}

impl VideoParams {
    pub fn video(o: &VideoOptions, volume: u8, mute: bool) -> Self {
        Self {
            fit: o.fit,
            background: o.background,
            interactive: o.interactive,
            looping: o.looping,
            audio: o.audio,
            speed: o.speed,
            hwdec: o.hwdec,
            volume,
            mute,
            gapless: true,
        }
    }

    /// 움직이는 GIF: 이미지 옵션 + 항상 반복·무음·소프트웨어 디코딩 (docs/architecture.md#gif).
    pub fn gif(o: &ImageOptions) -> Self {
        Self {
            fit: o.fit,
            background: o.background,
            interactive: o.interactive,
            looping: true,
            audio: false,
            speed: 1.0,
            hwdec: false,
            volume: 0,
            mute: true,
            gapless: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 복사 없이 dmabuf → wl_buffer.
    Dmabuf,
    /// videoconvert → BGRx → wl_shm. `fallback`: dmabuf를 시도했다가 실패해서 온 것.
    Shm { fallback: bool },
}

impl Mode {
    pub fn render_path(self) -> RenderPath {
        match self {
            Mode::Dmabuf => RenderPath::Dmabuf,
            Mode::Shm { fallback: true } => RenderPath::ShmFallback,
            Mode::Shm { fallback: false } => RenderPath::Shm,
        }
    }
}

/// GStreamer 스레드 → 메인 루프.
pub struct VideoMsg {
    pub output: u64,
    pub view: u64,
    pub event: VideoEvent,
}

pub enum VideoEvent {
    Frame(gst::Sample),
    /// 첫 PAUSED 준비 완료 또는 seek 완료.
    AsyncDone,
    SegmentDone,
    Eos,
    Error {
        message: String,
        not_negotiated: bool,
    },
}

static GST_INIT: Once = Once::new();

/// GStreamer는 영상이 처음 필요할 때 초기화한다 (플러그인 레지스트리 로딩 비용).
fn ensure_gst() -> Result<()> {
    let mut result = Ok(());
    GST_INIT.call_once(|| {
        result = gst::init().map_err(|e| anyhow!("failed to initialize GStreamer: {e}"));
        if result.is_ok() {
            tracing::info!(version = %gst::version_string(), "GStreamer initialized");
        }
    });
    result
}

/// playbin 하나.
struct Player {
    pipeline: gst::Element,
    looping: bool,
    gapless: bool,
    speed: f64,
    started: bool,
}

impl Player {
    fn new(
        path: &Path,
        params: &VideoParams,
        mode: Mode,
        formats: &[DrmFormat],
        tx: Sender<VideoMsg>,
        output: u64,
        view: u64,
    ) -> Result<Self> {
        ensure_gst()?;
        let uri = glib_uri(path)?;
        // playbin3는 force-sw-decoders를 무시하므로 소프트웨어 강제일 때만 playbin.
        let factory = if params.hwdec { "playbin3" } else { "playbin" };
        let pipeline = gst::ElementFactory::make(factory)
            .property("uri", &uri)
            .build()
            .with_context(|| format!("GStreamer element {factory} is missing"))?;

        let mut flags = FLAG_VIDEO | FLAG_SOFT_VOLUME | FLAG_NATIVE_VIDEO;
        if params.audio {
            flags |= FLAG_AUDIO;
        }
        if !params.hwdec {
            flags |= FLAG_FORCE_SW_DECODERS;
        }
        pipeline.set_property_from_str("flags", &flags.to_string());

        let caps = match mode {
            Mode::Dmabuf => dmabuf_caps(formats)?,
            Mode::Shm { .. } => SHM_CAPS.parse().map_err(|e| anyhow!("{SHM_CAPS}: {e}"))?,
        };
        let sink = gst_app::AppSink::builder().caps(&caps).sync(true).build();
        // VA 디코더는 받는 쪽이 VideoMeta(plane별 offset/stride)를 읽을 수 있어야 dmabuf를 내보낸다.
        // appsink는 이를 알리지 않으므로 allocation 쿼리에 직접 추가한다.
        let pad = sink.static_pad("sink").context("appsink has no sink pad")?;
        pad.add_probe(gst::PadProbeType::QUERY_DOWNSTREAM, |_, info| {
            if let Some(gst::PadProbeData::Query(q)) = &mut info.data
                && let gst::QueryViewMut::Allocation(a) = q.view_mut()
            {
                a.add_allocation_meta::<gst_video::VideoMeta>(None);
            }
            gst::PadProbeReturn::Ok
        });
        let frame_tx = tx.clone();
        let preroll_tx = tx.clone();
        let send = move |tx: &Sender<VideoMsg>, s: &gst_app::AppSink, preroll: bool| {
            let sample = if preroll {
                s.pull_preroll()
            } else {
                s.pull_sample()
            };
            let sample = sample.map_err(|_| gst::FlowError::Eos)?;
            tx.send(VideoMsg {
                output,
                view,
                event: VideoEvent::Frame(sample),
            })
            .map_err(|_| gst::FlowError::Flushing)?;
            Ok(gst::FlowSuccess::Ok)
        };
        sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |s| send(&frame_tx, s, false))
                .new_preroll(move |s| send(&preroll_tx, s, true))
                .build(),
        );
        let video_sink: gst::Element = match mode {
            Mode::Dmabuf => sink.upcast(),
            Mode::Shm { .. } => {
                // 소프트웨어 디코더의 I420 등을 BGRx로 바꾼다.
                let bin = gst::Bin::new();
                let convert = gst::ElementFactory::make("videoconvert")
                    .build()
                    .context("GStreamer element videoconvert is missing")?;
                bin.add_many([&convert, sink.upcast_ref()])?;
                convert.link(&sink)?;
                let pad = convert
                    .static_pad("sink")
                    .context("videoconvert has no sink pad")?;
                bin.add_pad(&gst::GhostPad::with_target(&pad)?)?;
                bin.upcast()
            }
        };
        pipeline.set_property("video-sink", &video_sink);

        let bus = pipeline.bus().context("playbin has no bus")?;
        bus.set_sync_handler(move |_, msg| {
            use gst::MessageView;
            let event = match msg.view() {
                MessageView::AsyncDone(_) => Some(VideoEvent::AsyncDone),
                MessageView::SegmentDone(_) => Some(VideoEvent::SegmentDone),
                MessageView::Eos(_) => Some(VideoEvent::Eos),
                MessageView::Error(e) => Some(VideoEvent::Error {
                    message: format!(
                        "{} ({})",
                        e.error(),
                        e.debug().map(|d| d.to_string()).unwrap_or_default()
                    ),
                    not_negotiated: e.error().matches(gst::StreamError::Format)
                        || e.debug().is_some_and(|d| d.contains("not-negotiated")),
                }),
                MessageView::Warning(w) => {
                    tracing::warn!(output, "GStreamer: {}", w.error());
                    None
                }
                _ => None,
            };
            if let Some(event) = event {
                let _ = tx.send(VideoMsg {
                    output,
                    view,
                    event,
                });
            }
            gst::BusSyncReply::Drop
        });

        let player = Self {
            pipeline,
            looping: params.looping,
            gapless: params.gapless,
            speed: params.speed,
            started: false,
        };
        player.set_volume(params);
        player
            .pipeline
            .set_state(gst::State::Paused)
            .map_err(|_| anyhow!("{}: cannot start playback", path.display()))?;
        Ok(player)
    }

    fn set_volume(&self, params: &VideoParams) {
        let volume = f64::from(params.volume) / 100.0;
        self.pipeline.set_property("volume", volume);
        self.pipeline
            .set_property("mute", params.mute || !params.audio);
    }

    /// 처음 PAUSED가 되면 반복·속도를 정하는 seek를 하고 재생을 시작한다.
    fn on_async_done(&mut self, paused: bool) -> Result<()> {
        if self.started {
            return Ok(());
        }
        self.started = true;
        self.seek(gst::SeekFlags::FLUSH)?;
        if !paused {
            self.set_paused(false);
        }
        Ok(())
    }

    fn seek(&self, extra: gst::SeekFlags) -> Result<()> {
        let mut flags = extra | gst::SeekFlags::ACCURATE;
        if self.looping {
            // SEGMENT: 끝에서 EOS 대신 SEGMENT_DONE → 이음새 없이 다시 seek.
            flags |= gst::SeekFlags::SEGMENT;
        }
        self.pipeline
            .seek(
                self.speed,
                flags,
                gst::SeekType::Set,
                gst::ClockTime::ZERO,
                gst::SeekType::None,
                gst::ClockTime::NONE,
            )
            .map_err(|e| anyhow!("seek failed: {e}"))
    }

    fn on_segment_done(&self) -> Result<()> {
        if self.gapless {
            self.seek(gst::SeekFlags::empty())
        } else {
            self.seek(gst::SeekFlags::FLUSH)
        }
    }

    fn set_paused(&self, paused: bool) {
        let state = if paused {
            gst::State::Paused
        } else {
            gst::State::Playing
        };
        if self.pipeline.set_state(state).is_err() {
            tracing::warn!(?state, "GStreamer state change failed");
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn glib_uri(path: &Path) -> Result<String> {
    let abs = std::path::absolute(path)?;
    gst::glib::filename_to_uri(&abs, None)
        .map(|u| u.to_string())
        .map_err(|e| anyhow!("{}: {e}", path.display()))
}

fn dmabuf_caps(formats: &[DrmFormat]) -> Result<gst::Caps> {
    if formats.is_empty() {
        bail!("the compositor advertises no dmabuf formats");
    }
    let list = gst::List::new(formats.iter().map(|f| f.to_gst()));
    let caps = gst::Caps::builder("video/x-raw")
        .features([gst_alloc::CAPS_FEATURE_MEMORY_DMABUF])
        .field("format", "DMA_DRM")
        .field("drm-format", list)
        .build();
    Ok(caps)
}

/// 이 view에서 쓰는 dmabuf `wl_buffer` 캐시 키: (dmabuf inode, 첫 plane offset).
type BufferKey = (u64, u64);

pub struct VideoView {
    // 필드 순서 = drop 순서: 파이프라인을 먼저 멈춘다.
    player: Player,
    pub id: u64,
    pub params: VideoParams,
    pub mode: Mode,
    pub name: String,
    path: std::path::PathBuf,
    pub plane: DesyncPlane,
    /// 영상 프레임 크기. 첫 프레임 전에는 `None`.
    size: Option<(u32, u32)>,
    logical: (u32, u32),
    dmabufs: HashMap<BufferKey, WlBuffer>,
    /// 컴포지터가 아직 쓰고 있는 버퍼의 샘플. release 때 GStreamer 풀로 돌려준다.
    held: HashMap<wayland_client::backend::ObjectId, gst::Sample>,
    shm: Option<ShmRing>,
    pub shown_first: bool,
    stats_frames: u32,
    stats_since: Instant,
}

struct ShmRing {
    pool: SlotPool,
    buffers: Vec<Buffer>,
    size: (u32, u32),
}

impl VideoView {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        path: &Path,
        name: String,
        params: VideoParams,
        mode: Mode,
        formats: &[DrmFormat],
        plane: DesyncPlane,
        logical: (u32, u32),
        tx: Sender<VideoMsg>,
        output: u64,
        id: u64,
    ) -> Result<Self> {
        let player = Player::new(path, &params, mode, formats, tx, output, id)?;
        tracing::info!(video = %name, ?mode, "video pipeline started");
        Ok(Self {
            player,
            id,
            params,
            mode,
            name,
            path: path.to_owned(),
            plane,
            size: None,
            logical,
            dmabufs: HashMap::new(),
            held: HashMap::new(),
            shm: None,
            shown_first: false,
            stats_frames: 0,
            stats_since: Instant::now(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn on_async_done(&mut self, paused: bool) -> Result<()> {
        self.player.on_async_done(paused)
    }

    pub fn on_segment_done(&self) -> Result<()> {
        self.player.on_segment_done()
    }

    pub fn set_paused(&self, paused: bool) {
        if self.player.started {
            self.player.set_paused(paused);
        }
    }

    pub fn set_volume(&mut self, volume: u8, mute: bool) {
        self.params.volume = volume;
        self.params.mute = mute;
        self.player.set_volume(&self.params);
    }

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

    pub fn relayout(&mut self, logical: (u32, u32)) {
        self.logical = logical;
        if let Some(size) = self.size {
            self.place(size);
            self.plane.surface.commit();
        }
    }

    fn place(&self, size: (u32, u32)) {
        let p = place(self.params.fit, size, self.logical);
        let (x, y, w, h) = p.source;
        self.plane.viewport.set_source(x, y, w, h);
        self.plane.viewport.set_destination(p.size.0, p.size.1);
        self.plane
            .subsurface()
            .set_position(p.position.0, p.position.1);
    }

    /// 샘플 하나를 서브서피스에 붙이고 커밋한다.
    pub fn present(
        &mut self,
        ctx: &Ctx<'_>,
        dmabuf: &DmabufState,
        sample: gst::Sample,
    ) -> Result<()> {
        let caps = sample
            .caps()
            .context("video sample without caps")?
            .to_owned();
        let buffer = sample.buffer().context("video sample without buffer")?;
        let (wl, size) = match self.mode {
            Mode::Dmabuf => self.dmabuf_buffer(ctx, dmabuf, &caps, buffer)?,
            Mode::Shm { .. } => self.shm_buffer(ctx, &caps, buffer)?,
        };
        if self.size != Some(size) {
            if self.size.is_some() {
                tracing::debug!(video = %self.name, ?size, "video size changed");
            }
            self.size = Some(size);
            self.place(size);
        }
        let surface = &self.plane.surface;
        surface.attach(Some(&wl), 0, 0);
        surface.damage_buffer(0, 0, size.0 as i32, size.1 as i32);
        surface.commit();
        if self.mode == Mode::Dmabuf {
            // release까지 디코더 버퍼를 붙잡아 둔다.
            self.held.insert(wl.id(), sample);
        }
        self.shown_first = true;
        self.stats_frames = self.stats_frames.saturating_add(1);
        Ok(())
    }

    /// 컴포지터가 놓은 dmabuf 버퍼: 샘플을 GStreamer에 돌려준다.
    pub fn released(&mut self, buffer: &WlBuffer) -> bool {
        self.held.remove(&buffer.id()).is_some()
    }

    fn dmabuf_buffer(
        &mut self,
        ctx: &Ctx<'_>,
        dmabuf: &DmabufState,
        caps: &gst::Caps,
        buffer: &gst::BufferRef,
    ) -> Result<(WlBuffer, (u32, u32))> {
        let info = gst_video::VideoInfoDmaDrm::from_caps(caps)
            .map_err(|e| anyhow!("not a DMA_DRM caps: {e}"))?;
        let (w, h) = (info.width(), info.height());
        let meta = buffer.meta::<gst_video::VideoMeta>();
        let n_planes = meta.as_ref().map_or(info.n_planes(), |m| m.n_planes());
        let offsets: Vec<usize> = match &meta {
            Some(m) => m.offset().to_vec(),
            None => info.offset().to_vec(),
        };
        let strides: Vec<i32> = match &meta {
            Some(m) => m.stride().to_vec(),
            None => info.stride().to_vec(),
        };

        let mut planes = Vec::with_capacity(n_planes as usize);
        for i in 0..n_planes as usize {
            let (range, skip) = buffer
                .find_memory(offsets[i]..offsets[i] + 1)
                .context("plane offset outside the buffer")?;
            let mem = buffer.peek_memory(range.start);
            let dma = mem
                .downcast_memory_ref::<gst_alloc::DmaBufMemory>()
                .context("video buffer is not a dmabuf")?;
            planes.push((dma.fd(), (mem.offset() + skip) as u32, strides[i] as u32));
        }
        let (fd0, off0, _) = planes[0];
        // SAFETY: 위와 같음.
        let ino = rustix::fs::fstat(unsafe { BorrowedFd::borrow_raw(fd0) })
            .map(|s| s.st_ino)
            .context("fstat on dmabuf")?;
        let key = (ino, u64::from(off0));
        if let Some(wl) = self.dmabufs.get(&key) {
            return Ok((wl.clone(), (w, h)));
        }

        let params = dmabuf
            .create_params(ctx.qh)
            .context("zwp_linux_dmabuf_v1 is not available")?;
        for (i, (fd, offset, stride)) in planes.iter().enumerate() {
            // SAFETY: fd는 이 함수가 끝날 때까지 살아 있는 sample의 메모리가 갖고 있다.
            let fd = unsafe { BorrowedFd::borrow_raw(*fd) };
            params.add(fd, i as u32, *offset, *stride, info.modifier());
        }
        let (wl, _) =
            params.create_immed(w as i32, h as i32, info.fourcc(), Flags::empty(), ctx.qh);
        tracing::debug!(video = %self.name, cached = self.dmabufs.len() + 1, "new dmabuf wl_buffer");
        self.dmabufs.insert(key, wl.clone());
        Ok((wl, (w, h)))
    }

    fn shm_buffer(
        &mut self,
        ctx: &Ctx<'_>,
        caps: &gst::Caps,
        buffer: &gst::BufferRef,
    ) -> Result<(WlBuffer, (u32, u32))> {
        let info = gst_video::VideoInfo::from_caps(caps).map_err(|e| anyhow!("{e}"))?;
        let (w, h) = (info.width(), info.height());
        let src_stride = buffer
            .meta::<gst_video::VideoMeta>()
            .map_or(info.stride()[0], |m| m.stride()[0]) as usize;
        let stride = w * BYTES_PER_PIXEL;
        if self.shm.as_ref().is_none_or(|r| r.size != (w, h)) {
            let pool = SlotPool::new((stride * h) as usize, ctx.shm).context("wl_shm pool")?;
            self.shm = Some(ShmRing {
                pool,
                buffers: Vec::new(),
                size: (w, h),
            });
        }
        let ring = self.shm.as_mut().context("shm ring")?;
        // 컴포지터가 놓은 버퍼를 다시 쓰고, 다 쓰고 있으면 하나 더 만든다.
        let idx = match ring
            .buffers
            .iter()
            .position(|b| b.canvas(&mut ring.pool).is_some())
        {
            Some(i) => i,
            None => {
                let (b, _) = ring
                    .pool
                    .create_buffer(w as i32, h as i32, stride as i32, wl_shm::Format::Xrgb8888)
                    .context("wl_shm buffer")?;
                ring.buffers.push(b);
                ring.buffers.len() - 1
            }
        };
        let map = buffer.map_readable().context("map video frame")?;
        let canvas = ring.buffers[idx]
            .canvas(&mut ring.pool)
            .context("shm buffer busy")?;
        let row = stride as usize;
        for y in 0..h as usize {
            let src = &map[y * src_stride..y * src_stride + row];
            canvas[y * row..(y + 1) * row].copy_from_slice(src);
        }
        Ok((ring.buffers[idx].wl_buffer().clone(), (w, h)))
    }
}

impl Drop for VideoView {
    fn drop(&mut self) {
        for (_, wl) in self.dmabufs.drain() {
            wl.destroy();
        }
    }
}

/// GIF가 움직이는지(프레임 2장 이상). 1장짜리는 정적 이미지 경로로 보낸다.
pub fn gif_is_animated(path: &Path) -> Result<bool> {
    use image::AnimationDecoder;
    let file =
        std::fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let decoder = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(file))
        .with_context(|| format!("invalid GIF {}", path.display()))?;
    Ok(decoder.into_frames().take(2).count() > 1)
}
