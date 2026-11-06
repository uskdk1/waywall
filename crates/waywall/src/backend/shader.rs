//! Shadertoy 형식 셰이더 렌더러: 화면을 덮는 삼각형 하나 + Shadertoy uniform + 텍스처 채널 N개
//! (docs/wallpaper-format.md#셰이더-배경). N = max(4, 쓴 가장 큰 채널 번호 + 1), 상한은 GPU 한도.
//!
//! `ShaderRenderer`는 셰이더 배경과 씬 shader 레이어가 함께 쓴다. 채널에 무엇을 묶을지는 쓰는 쪽이 정한다.

use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use chrono::{Datelike, Local, Timelike};
use toml::{Table, Value};
use waywall_proto::config::{
    Animate, AnimateMode, AnimateWindows, ChannelDefaults, ChannelFilter, ChannelWrap,
};

use crate::backend::scene::Texture;
use crate::backend::view::FrameInfo;
use crate::config::merge;
use crate::gpu::shadertoy::{
    Compiled, Inputs, Language, SHADERTOY_CHANNELS, Slots, UniformLayout, UniformReads,
    sampler_binding, texture_binding,
};
use crate::gpu::{FULLSCREEN_ENTRY, Gpu};
use crate::properties::Property;

/// 채널 이름(`[inputs]`의 키)의 앞부분. 뒤에 0부터의 번호가 붙는다.
const CHANNEL_PREFIX: &str = "iChannel";
/// 채널 입력 표의 필드.
const SOURCE_KEY: &str = "source";
/// `[types.<타입>]` 아래 채널 기본값 섹션 이름.
pub const CHANNEL_SECTION: &str = "channel";
/// 지정하지 않은 채널에 묶는 1×1 투명 텍스처.
const EMPTY_TEXEL: [u8; 4] = [0; 4];

/// 셰이더를 언제 계속 그릴지: `animate` 값 + 셰이더가 읽는 uniform (docs/wallpaper-format.md#잠들기-animate).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Activity {
    /// 늘 계속 그림.
    always: bool,
    /// 사건 뒤 계속 그리는 시간(초).
    windows: AnimateWindows,
    /// 포인터·클릭 uniform을 읽음 → 포인터 사건마다 다시 그려야 함.
    pointer: bool,
    /// `iMouse`를 읽음 → 버튼을 누른 뒤 한 프레임 더 (`iMouse.w`가 첫 프레임에만 양수).
    mouse: bool,
}

impl Activity {
    pub fn new(animate: Animate, reads: &UniformReads) -> Self {
        let (always, windows) = match animate {
            Animate::Mode(AnimateMode::Auto) => (reads.time(), AnimateWindows::default()),
            Animate::Mode(AnimateMode::Always) => (true, AnimateWindows::default()),
            Animate::Windows(w) => (false, w),
        };
        Self {
            always,
            windows,
            pointer: reads.pointer(),
            mouse: reads.mouse(),
        }
    }

    /// 이 프레임 다음에도 그려야 하는지.
    pub fn moving(&self, f: &FrameInfo<'_>) -> bool {
        let t = f64::from(f.time);
        let p = f.pointer;
        let within = |since: Option<f32>, window: f64| {
            window > 0.0 && since.is_some_and(|s| t - f64::from(s) < window)
        };
        self.always
            || t < self.windows.start
            || within(p.clicked_at(), self.windows.click)
            || within(p.last_event, self.windows.pointer)
            || (self.mouse && p.first_press_frame())
    }

    /// 포인터가 움직이거나 버튼을 누르면 다시 그려야 하는지.
    pub fn follows_pointer(&self) -> bool {
        self.always || self.pointer || self.windows.pointer > 0.0
    }

    /// 클릭하면 다시 그려야 하는지.
    pub fn wants_clicks(&self) -> bool {
        self.follows_pointer() || self.windows.click > 0.0
    }
}

/// `[inputs]`의 채널 하나 (병합 후).
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelDef {
    pub source: String,
    pub sampling: ChannelDefaults,
}

/// 채널 번호 → `[inputs]` 키 (`iChannel<N>`).
pub fn channel_name(i: usize) -> String {
    format!("{CHANNEL_PREFIX}{i}")
}

/// `iChannel<N>` → N. `iChannel01`처럼 다시 쓰면 다른 이름이 되는 키는 받지 않는다.
fn channel_index(key: &str) -> Option<usize> {
    let i: usize = key.strip_prefix(CHANNEL_PREFIX)?.parse().ok()?;
    (channel_name(i) == key).then_some(i)
}

/// `inputs` 표 → 채널별 정의. 길이 = 셰이더가 받는 채널 수 (`SHADERTOY_CHANNELS` 이상, 쓴 가장 큰 번호 + 1).
/// 계층: `defaults`(내장 < 사용자) < 채널 값 < `user`(사용자가 직접 적은 섹션).
pub fn parse_inputs(
    inputs: Option<&Table>,
    defaults: &ChannelDefaults,
    user: Option<&Table>,
) -> Result<Vec<Option<ChannelDef>>> {
    let mut out: Vec<Option<ChannelDef>> = vec![None; SHADERTOY_CHANNELS];
    let Some(inputs) = inputs else {
        return Ok(out);
    };
    for (key, value) in inputs {
        let Some(i) = channel_index(key) else {
            bail!("inputs: unknown channel `{key}` (iChannel0, iChannel1, …)");
        };
        let mut t = Table::try_from(defaults)?;
        let source = match value {
            Value::String(s) => s.clone(),
            Value::Table(v) => {
                let mut v = v.clone();
                let source = v
                    .remove(SOURCE_KEY)
                    .and_then(|s| s.as_str().map(str::to_owned))
                    .with_context(|| format!("inputs.{key}: missing `{SOURCE_KEY}`"))?;
                merge(&mut t, &v);
                source
            }
            _ => bail!("inputs.{key}: expected a path or {{ {SOURCE_KEY} = …, filter, wrap }}"),
        };
        if let Some(u) = user {
            merge(&mut t, u);
        }
        let sampling: ChannelDefaults = t.try_into().with_context(|| format!("inputs.{key}"))?;
        if out.len() <= i {
            out.resize(i + 1, None);
        }
        out[i] = Some(ChannelDef { source, sampling });
    }
    Ok(out)
}

/// 셰이더 배경 채널 이미지: Shadertoy처럼 값은 그대로(sRGB 해석·premultiply 없음), GLSL이면 상하 반전.
pub fn decode_channel(path: &Path, lang: Language) -> Result<Texture> {
    let img = image::ImageReader::open(path)?
        .with_guessed_format()?
        .decode()?
        .into_rgba8();
    let img = if lang == Language::Glsl {
        image::imageops::flip_vertical(&img)
    } else {
        img
    };
    let (width, height) = img.dimensions();
    Ok(Texture {
        width,
        height,
        data: img.into_raw(),
    })
}

pub struct ShaderSpec {
    pub compiled: Compiled,
    pub props: Vec<Property>,
    /// 지정하지 않은 채널은 `None` (1×1 투명). 길이 = 채널 수.
    pub channels: Vec<Option<Texture>>,
    /// 채널마다 샘플링 (지정하지 않은 채널은 기본값, 쓰이지 않음).
    pub sampling: Vec<ChannelDefaults>,
    pub animate: Animate,
}

pub struct ShaderRenderer {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_layout: wgpu::BindGroupLayout,
    samplers: Vec<wgpu::Sampler>,
    bind_group: Option<wgpu::BindGroup>,
    layout: UniformLayout,
    slots: Slots,
    props: Vec<Property>,
}

/// `ShaderRenderer::render`의 그릴 곳.
pub struct Draw<'a> {
    pub target: &'a wgpu::TextureView,
    /// 이전 내용을 지울지(전부 덮는 경우) 둘지(장면 위에 블렌드).
    pub load: wgpu::LoadOp<wgpu::Color>,
    /// 이 영역만 그림 (px).
    pub scissor: Option<(u32, u32, u32, u32)>,
}

impl ShaderRenderer {
    /// 채널 수는 `sampling`의 길이 (`compile`에 넘긴 수와 같아야 함).
    pub fn new(
        gpu: &Gpu,
        compiled: Compiled,
        props: Vec<Property>,
        sampling: Vec<ChannelDefaults>,
        format: wgpu::TextureFormat,
        blend: Option<wgpu::BlendState>,
        name: &str,
    ) -> Result<Self> {
        let channels = sampling.len();
        let limits = gpu.device.limits();
        let max = limits
            .max_sampled_textures_per_shader_stage
            .min(limits.max_samplers_per_shader_stage) as usize;
        ensure!(
            channels <= max,
            "{name}: {channels} channels is more than this GPU allows ({max} per shader)"
        );
        let layout = compiled.layout.clone();
        let slots = layout.slots(&props);
        let entry = compiled.entry;
        let fragment = gpu.create_module(name, compiled.module)?;
        let device = &gpu.device;
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("waywall shader uniforms"),
            size: layout.buffer_size(),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (pipeline, bind_layout, samplers) = gpu
            .scoped(|d| {
                let mut entries = vec![wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }];
                for i in 0..channels {
                    entries.push(wgpu::BindGroupLayoutEntry {
                        binding: texture_binding(i),
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    });
                    entries.push(wgpu::BindGroupLayoutEntry {
                        binding: sampler_binding(channels, i),
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    });
                }
                let bgl = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("waywall shader globals"),
                    entries: &entries,
                });
                let pl = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("waywall shader"),
                    bind_group_layouts: &[Some(&bgl)],
                    immediate_size: 0,
                });
                let pipeline = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(name),
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
                            blend,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
                let samplers = sampling.iter().map(|s| sampler(d, s)).collect();
                (pipeline, bgl, samplers)
            })
            .with_context(|| format!("{name}: failed to create the render pipeline"))?;
        Ok(Self {
            pipeline,
            uniforms,
            bind_layout,
            samplers,
            bind_group: None,
            layout,
            slots,
            props,
        })
    }

    /// 채널 텍스처를 묶는다(채널 수만큼). 텍스처가 바뀔 때마다(크기 변경 등) 다시 부른다.
    pub fn bind(&mut self, gpu: &Gpu, views: &[&wgpu::TextureView]) {
        let channels = self.samplers.len();
        debug_assert_eq!(views.len(), channels);
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: self.uniforms.as_entire_binding(),
        }];
        for (i, (view, sampler)) in views.iter().zip(&self.samplers).enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: texture_binding(i),
                resource: wgpu::BindingResource::TextureView(view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: sampler_binding(channels, i),
                resource: wgpu::BindingResource::Sampler(sampler),
            });
        }
        self.bind_group = Some(gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waywall shader globals"),
            layout: &self.bind_layout,
            entries: &entries,
        }));
    }

    pub fn render(
        &self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        draw: Draw<'_>,
        inputs: &Inputs,
        name: &str,
    ) {
        let Some(bind_group) = &self.bind_group else {
            return;
        };
        gpu.queue.write_buffer(
            &self.uniforms,
            0,
            &self.layout.write(&self.slots, inputs, &self.props),
        );
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(name),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: draw.target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: draw.load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if let Some((x, y, w, h)) = draw.scissor {
            pass.set_scissor_rect(x, y, w, h);
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// 프레임 값 → uniform 입력. `size`: 셰이더가 그리는 크기(px), `origin`: 그 영역의 왼쪽 아래(렌더 px, 왼쪽 아래 원점),
/// `scale`: 그리는 해상도 / 렌더 해상도 (포인터 좌표를 그리는 해상도로 옮김).
pub fn frame_inputs(
    f: &FrameInfo<'_>,
    size: (u32, u32),
    origin: [f32; 2],
    scale: f32,
    channel_resolution: Vec<[f32; 3]>,
) -> Inputs {
    let date = Local::now();
    let midnight = f64::from(date.num_seconds_from_midnight()) + f64::from(date.nanosecond()) / 1e9;
    let p = f.pointer;
    let local = |x: f32, y: f32| [(x - origin[0]) * scale, (y - origin[1]) * scale];
    // iMouse의 z, w는 부호가 상태를 나타내므로 크기만 옮긴다.
    let shift = |v: f32, o: f32| {
        if v < 0.0 {
            -(v.abs() - o) * scale
        } else {
            (v - o) * scale
        }
    };
    let [px, py] = local(p.pos[0], p.pos[1]);
    let [cx, cy] = local(p.click[0], p.click[1]);
    let [mx, my] = local(p.mouse[0], p.mouse[1]);
    Inputs {
        resolution: [size.0 as f32, size.1 as f32, 1.0],
        time: f.time,
        time_delta: f.time_delta,
        frame: f.frame,
        frame_rate: if f.time_delta > 0.0 {
            1.0 / f.time_delta
        } else {
            0.0
        },
        date: [
            date.year() as f32,
            date.month0() as f32,
            date.day() as f32,
            midnight as f32,
        ],
        mouse: [
            mx,
            my,
            shift(p.mouse[2], origin[0]),
            shift(p.mouse[3], origin[1]),
        ],
        pointer: [px, py, f32::from(u8::from(p.inside)), p.buttons as f32],
        click: [cx, cy, p.click[2], p.click[3]],
        channel_resolution,
        origin: [0.0, 0.0],
    }
}

pub fn sampler(d: &wgpu::Device, s: &ChannelDefaults) -> wgpu::Sampler {
    let filter = match s.filter {
        ChannelFilter::Linear => wgpu::FilterMode::Linear,
        ChannelFilter::Nearest => wgpu::FilterMode::Nearest,
    };
    let wrap = match s.wrap {
        ChannelWrap::Repeat => wgpu::AddressMode::Repeat,
        ChannelWrap::Clamp => wgpu::AddressMode::ClampToEdge,
        ChannelWrap::Mirror => wgpu::AddressMode::MirrorRepeat,
    };
    d.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("waywall channel"),
        address_mode_u: wrap,
        address_mode_v: wrap,
        address_mode_w: wrap,
        mag_filter: filter,
        min_filter: filter,
        ..Default::default()
    })
}

/// 1×1 투명 텍스처 (지정하지 않은 채널).
pub fn empty_texture() -> Texture {
    Texture {
        width: 1,
        height: 1,
        data: EMPTY_TEXEL.to_vec(),
    }
}

/// 셰이더 배경: 채널 이미지는 처음에 한 번 올린다.
pub struct ShaderBackground {
    renderer: ShaderRenderer,
    pub activity: Activity,
    channel_resolution: Vec<[f32; 3]>,
    _textures: Vec<wgpu::Texture>,
}

impl ShaderBackground {
    pub fn new(
        gpu: &Gpu,
        spec: ShaderSpec,
        format: wgpu::TextureFormat,
        name: &str,
    ) -> Result<Self> {
        let max_dim = gpu.device.limits().max_texture_dimension_2d;
        let ShaderSpec {
            compiled,
            props,
            channels,
            sampling,
            animate,
        } = spec;
        let activity = Activity::new(animate, &compiled.reads);
        let mut channel_resolution = vec![[0.0; 3]; channels.len()];
        let mut textures = Vec::with_capacity(channels.len());
        for (i, ch) in channels.into_iter().enumerate() {
            let texture = match ch {
                Some(t) => {
                    ensure!(
                        t.width <= max_dim && t.height <= max_dim,
                        "{}: texture {}x{} is larger than the GPU limit {max_dim}",
                        channel_name(i),
                        t.width,
                        t.height
                    );
                    channel_resolution[i] = [t.width as f32, t.height as f32, 1.0];
                    t
                }
                None => empty_texture(),
            };
            textures.push(gpu.scoped(|d| {
                crate::backend::scene::upload_texture(
                    gpu,
                    d,
                    &channel_name(i),
                    &texture,
                    wgpu::TextureFormat::Rgba8Unorm,
                )
            })?);
        }
        let mut renderer = ShaderRenderer::new(gpu, compiled, props, sampling, format, None, name)?;
        let views: Vec<wgpu::TextureView> = textures
            .iter()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()))
            .collect();
        renderer.bind(gpu, &views.iter().collect::<Vec<_>>());
        Ok(Self {
            renderer,
            activity,
            channel_resolution,
            _textures: textures,
        })
    }

    pub fn render(
        &self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        f: &FrameInfo<'_>,
        name: &str,
    ) {
        let inputs = frame_inputs(f, f.size, [0.0, 0.0], 1.0, self.channel_resolution.clone());
        let draw = Draw {
            target,
            // 삼각형이 전부 덮으므로 이전 내용은 필요 없다.
            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            scissor: None,
        };
        self.renderer.render(gpu, encoder, draw, &inputs, name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULTS: ChannelDefaults = ChannelDefaults {
        filter: ChannelFilter::Linear,
        wrap: ChannelWrap::Repeat,
    };

    #[test]
    fn inputs_layer_like_other_fields() {
        let t: Table =
            "iChannel0 = \"a.png\"\niChannel2 = { source = \"b.png\", wrap = \"clamp\" }"
                .parse()
                .unwrap();
        let c = parse_inputs(Some(&t), &DEFAULTS, None).unwrap();
        assert_eq!(c.len(), SHADERTOY_CHANNELS);
        assert_eq!(c[0].as_ref().unwrap().sampling, DEFAULTS);
        assert!(c[1].is_none());
        let b = c[2].as_ref().unwrap();
        assert_eq!(
            (b.source.as_str(), b.sampling.wrap),
            ("b.png", ChannelWrap::Clamp)
        );
        // 사용자가 [types.shader.channel]에 적은 값은 배경의 채널 값보다 우선.
        let user: Table = "filter = \"nearest\"\nwrap = \"mirror\"".parse().unwrap();
        let c = parse_inputs(Some(&t), &DEFAULTS, Some(&user)).unwrap();
        let s = c[2].as_ref().unwrap().sampling;
        assert_eq!(
            (s.filter, s.wrap),
            (ChannelFilter::Nearest, ChannelWrap::Mirror)
        );

        for (bad, why) in [
            ("iChannel01 = \"a.png\"", "iChannel01"),
            ("iChannelX = \"a.png\"", "iChannelX"),
            ("channel0 = \"a.png\"", "channel0"),
            ("iChannel0 = { wrap = \"clamp\" }", "source"),
            (
                "iChannel0 = { source = \"a\", wrap = \"spin\" }",
                "iChannel0",
            ),
            ("iChannel0 = 3", "expected a path"),
        ] {
            let t: Table = bad.parse().unwrap();
            let e = parse_inputs(Some(&t), &DEFAULTS, None).expect_err(bad);
            assert!(format!("{e:#}").contains(why), "{bad}: {e:#}");
        }
    }

    /// 채널 수 = max(4, 쓴 가장 큰 번호 + 1). 중간에 비운 채널은 `None`.
    #[test]
    fn channel_count_follows_highest_input() {
        assert_eq!(
            parse_inputs(None, &DEFAULTS, None).unwrap().len(),
            SHADERTOY_CHANNELS
        );
        let t: Table = "iChannel0 = \"a.png\"\niChannel9 = \"b.png\""
            .parse()
            .unwrap();
        let c = parse_inputs(Some(&t), &DEFAULTS, None).unwrap();
        assert_eq!(c.len(), 10);
        assert!(c[1..9].iter().all(Option::is_none));
        assert_eq!(c[9].as_ref().unwrap().source, "b.png");
        assert_eq!(channel_name(9), "iChannel9");
    }

    fn activity(animate: Animate, src: &str) -> Activity {
        let assets = crate::assets::Assets::new(std::path::PathBuf::from("/nonexistent"));
        let c = crate::gpu::shadertoy::compile(
            &assets,
            Language::Glsl,
            &format!("void mainImage(out vec4 c, in vec2 p) {{ {src} }}"),
            &[],
            SHADERTOY_CHANNELS,
            "t",
        )
        .unwrap();
        Activity::new(animate, &c.reads)
    }

    fn at(time: f32, pointer: &crate::backend::view::Pointer) -> FrameInfo<'_> {
        FrameInfo {
            size: (100, 100),
            time,
            time_delta: 0.0,
            frame: 0,
            pointer,
        }
    }

    const AUTO: Animate = Animate::Mode(AnimateMode::Auto);

    /// docs/wallpaper-format.md#잠들기-animate
    #[test]
    fn activity_sleeps_unless_time_or_window() {
        let still = crate::backend::view::Pointer::default();
        // 시간을 읽으면 계속, 포인터만 읽으면 잠들고 포인터 사건에 깨어남.
        let a = activity(AUTO, "c = vec4(sin(iTime));");
        assert!(a.moving(&at(9.0, &still)));
        let a = activity(AUTO, "c = vec4(iPointer.x);");
        assert!(!a.moving(&at(9.0, &still)));
        assert!(a.follows_pointer() && a.wants_clicks());
        // 아무 입력도 안 읽으면 포인터 사건에도 다시 그리지 않음.
        let a = activity(AUTO, "c = vec4(1.0);");
        assert!(!a.follows_pointer() && !a.wants_clicks());
        assert!(
            activity(Animate::Mode(AnimateMode::Always), "c = vec4(1.0);").moving(&at(9.0, &still))
        );

        // 창: 시간을 읽어도 사건 뒤 그 시간 동안만.
        let w = |click, pointer, start| {
            activity(
                Animate::Windows(AnimateWindows {
                    click,
                    pointer,
                    start,
                }),
                "c = vec4(iTime - iClick.z);",
            )
        };
        let a = w(0.7, 0.0, 0.0);
        assert!(!a.moving(&at(9.0, &still)), "no click yet");
        let clicked = crate::backend::view::Pointer {
            click: [1.0, 1.0, 5.0, 1.0],
            ..Default::default()
        };
        assert!(a.moving(&at(5.5, &clicked)));
        assert!(!a.moving(&at(5.8, &clicked)));
        assert!(a.wants_clicks() && a.follows_pointer(), "reads iClick");
        let moved = crate::backend::view::Pointer {
            last_event: Some(3.0),
            ..Default::default()
        };
        assert!(w(0.0, 0.5, 0.0).moving(&at(3.4, &moved)));
        assert!(!w(0.0, 0.5, 0.0).moving(&at(3.6, &moved)));
        assert!(
            !w(0.0, 0.0, 0.0).moving(&at(3.4, &moved)),
            "{{}} = only on input"
        );
        assert!(w(0.0, 0.0, 2.0).moving(&at(1.0, &still)));
        assert!(!w(0.0, 0.0, 2.0).moving(&at(2.5, &still)));

        // iMouse를 읽으면 누른 첫 프레임 다음에 한 장 더 (w가 음수가 된 모습).
        let a = activity(AUTO, "c = iMouse;");
        let pressed = crate::backend::view::Pointer {
            mouse: [1.0, 2.0, 1.0, 2.0],
            ..Default::default()
        };
        assert!(a.moving(&at(1.0, &pressed)));
        assert!(!a.moving(&at(1.0, &still)));
    }
}
