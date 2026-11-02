//! 씬 렌더러: 파티클 compute 패스 + shader 레이어 + HDR 레이어 패스 + 후처리(노출·톤매핑) 패스 (docs/architecture.md#씬-렌더러).

use std::collections::HashMap;

use anyhow::{Context, Result, ensure};
use waywall_proto::config::{Blend, Fit, HdrFormat, Render};

use super::bloom::{BloomChain, BloomPipelines};
use super::effects::{EffectShared, GpuEffects};
use super::particles::{self, GpuParticles};
use super::shader_layer::{Cropper, GpuShaderLayer, Sources, Target};
use super::{Follow, ImageLayer, Layer, Light, SceneSpec, Texture};
use crate::backend::shader::empty_texture;
use crate::backend::view::FrameInfo;
use crate::gpu::{FULLSCREEN_ENTRY, Gpu};
use waywall_proto::config::OnLeave;

/// `scene/image.wgsl`, `scene/post.wgsl`의 엔트리 (인터페이스).
const VS_ENTRY: &str = "vs_main";
const FS_ENTRY: &str = "fs_main";
/// image.wgsl의 `Layer` uniform: axis_u, axis_v, origin, opacity, lit, design_origin, design_u, design_v, rotation,
/// clip, 패딩 (f32 20개).
const LAYER_FLOATS: usize = 20;
/// image.wgsl `Lights`: ambient(f32 4개) 뒤에 광원마다 color, kind, geom (f32 8개).
const LIGHTS_HEADER_FLOATS: usize = 4;
const LIGHT_FLOATS: usize = 8;
/// 노멀맵이 없는 레이어에 묶는 1×1 노멀맵: 화면을 똑바로 보는 [0, 0, 1] (인코딩 값).
const FLAT_NORMAL: [u8; 4] = [128, 128, 255, 255];
/// post.wgsl의 `Post` uniform: exposure, bloom, vignette, 패딩 (f32 4개).
const POST_FLOATS: usize = 4;
/// 씬 텍스처(이미지 레이어, 파티클, 이미지 입력): 선형 공간에서 알파를 곱해 둔 sRGB.
pub const TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// 노멀맵: 값 그대로(선형).
const NORMAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const BLENDS: [Blend; 4] = [Blend::Normal, Blend::Add, Blend::Multiply, Blend::Screen];

pub struct SceneRenderer {
    size: [f64; 2],
    fit: Fit,
    clear: wgpu::Color,
    pointer_smoothing: f64,
    settle_px: f64,
    hdr_format: wgpu::TextureFormat,
    /// image 레이어용, `BLENDS` 순서. 쓰는 블렌드만 만든다.
    pipelines: [Option<wgpu::RenderPipeline>; BLENDS.len()],
    layers: Vec<DrawLayer>,
    /// `layers`와 같은 순서로, 화면에 그리는지 (아니면 shader 레이어 입력으로만 쓰임).
    drawn: Vec<bool>,
    /// shader 레이어 입력으로 쓰이는 레이어를 단독으로 그린 결과 (HDR 버퍼 크기·포맷).
    isolated: HashMap<usize, Target>,
    /// shader 레이어가 있을 때만.
    cropper: Cropper,
    effects: EffectShared,
    no_clip: wgpu::TextureView,
    _no_clip: wgpu::Texture,
    layer_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    flat_normal: wgpu::TextureView,
    _flat_normal: wgpu::Texture,
    lights: Vec<Light>,
    lights_buffer: wgpu::Buffer,
    /// 마지막으로 쓴 광원 값. 같으면 다시 쓰지 않는다.
    lights_written: Vec<f32>,
    /// `lit` 레이어가 있다 (없으면 광원을 갱신하지 않음).
    lit: bool,
    /// `position = "pointer"` 광원의 위치 (설계 px). 포인터가 나가면 마지막 위치.
    pointer_light: [f64; 2],
    post_pipeline: wgpu::RenderPipeline,
    post: Post,
    target: HdrTarget,
    max_parallax: f64,
    parallax: Parallax,
}

enum DrawLayer {
    Image(Box<GpuLayer>),
    Particles(Box<GpuParticles>),
    Shader(Box<GpuShaderLayer>),
}

impl DrawLayer {
    fn parallax(&self) -> f64 {
        match self {
            DrawLayer::Image(l) => l.parallax,
            DrawLayer::Particles(p) => p.parallax(),
            DrawLayer::Shader(s) => s.parallax(),
        }
    }
}

pub(super) struct GpuLayer {
    blend: Blend,
    opacity: f64,
    pub(super) parallax: f64,
    position: [f64; 2],
    /// 화면 크기의 절반 (설계 px), 회전 전.
    half: [f64; 2],
    /// (cos, sin), 시계 방향.
    rotation: [f64; 2],
    uniforms: wgpu::Buffer,
    bind_group: Option<wgpu::BindGroup>,
    /// 마지막으로 쓴 uniform. 같으면 다시 쓰지 않는다.
    written: Option<[f32; LAYER_FLOATS]>,
    lit: bool,
    /// clip 대상 (`layers`의 인덱스). 그 레이어의 단독 결과 알파를 곱한다.
    pub(super) clip: Option<usize>,
    pub(super) follow: Option<FollowState>,
    /// 묶은 텍스처 (clip 대상이 크기 변경으로 바뀌면 다시 묶는다).
    views: Option<(wgpu::TextureView, Option<wgpu::TextureView>)>,
    /// image 레이어 효과. 사각형은 그 결과를 읽는다.
    effects: Option<Box<GpuEffects>>,
    /// image 레이어가 가진 텍스처 (shader 레이어는 결과 텍스처를 따로 가진다).
    _texture: Option<wgpu::Texture>,
    _normal: Option<wgpu::Texture>,
}

/// `follow` 상태: 패럴랙스와 같은 지수 추적.
pub(super) struct FollowState {
    spec: Follow,
    smooth: Parallax,
    /// 마지막 목표 (`on_leave = "stay"`).
    goal: [f64; 2],
}

impl FollowState {
    pub(super) fn new(spec: Follow) -> Self {
        Self {
            spec,
            smooth: Parallax::default(),
            goal: [0.0; 2],
        }
    }

    /// `pointer`: 설계 px (배경 밖이면 `None`), `shift`: 이 레이어의 패럴랙스 이동. 아직 움직이면 `true`.
    fn step(&mut self, pointer: Option<[f64; 2]>, shift: [f64; 2], dt: f64, settle: f64) -> bool {
        let s = &self.spec;
        self.goal = match pointer {
            Some(p) => {
                let d = [
                    p[0] - (s.anchor[0] + shift[0]),
                    p[1] - (s.anchor[1] + shift[1]),
                ];
                let dist = d[0].hypot(d[1]);
                if dist > 0.0 {
                    let k = (dist / s.range).min(1.0) / dist;
                    [d[0] * k * s.max_offset[0], d[1] * k * s.max_offset[1]]
                } else {
                    [0.0; 2]
                }
            }
            None => match s.on_leave {
                OnLeave::Center => [0.0; 2],
                OnLeave::Stay => self.goal,
            },
        };
        self.smooth.step(self.goal, dt, s.smoothing, settle)
    }
}

struct HdrTarget {
    view: wgpu::TextureView,
    post_bind_group: wgpu::BindGroup,
    size: (u32, u32),
    bloom: Option<BloomChain>,
}

impl SceneRenderer {
    pub fn new(
        gpu: &Gpu,
        spec: SceneSpec,
        render: &Render,
        format: wgpu::TextureFormat,
        size: (u32, u32),
        name: &str,
    ) -> Result<Self> {
        let (hdr_format, needs) = match render.hdr_format {
            HdrFormat::Rgba16float => (wgpu::TextureFormat::Rgba16Float, wgpu::Features::empty()),
            HdrFormat::Rg11b10ufloat => (
                wgpu::TextureFormat::Rg11b10Ufloat,
                wgpu::Features::RG11B10UFLOAT_RENDERABLE,
            ),
            HdrFormat::Rgba32float => (
                wgpu::TextureFormat::Rgba32Float,
                wgpu::Features::FLOAT32_BLENDABLE,
            ),
        };
        ensure!(
            gpu.device.features().contains(needs),
            "render.hdr_format = {:?} needs {needs:?}, which this GPU does not support",
            render.hdr_format
        );
        let image = gpu.create_module("scene/image.wgsl", spec.image_shader)?;
        let post = gpu.create_module("scene/post.wgsl", spec.post_shader)?;
        let max_dim = gpu.device.limits().max_texture_dimension_2d;
        for l in &spec.layers {
            if let Some(t) = l.texture() {
                ensure!(
                    t.width <= max_dim && t.height <= max_dim,
                    "layer `{}`: texture {}x{} is larger than the GPU limit {max_dim}",
                    l.id(),
                    t.width,
                    t.height
                );
            }
            if let Layer::Particles(p) = l {
                particles::check_limits(gpu, p)?;
            }
        }
        let clipped = spec.layers.iter().any(|l| match l {
            Layer::Image(i) => i.clip.is_some(),
            Layer::Shader(s) => s.clip.is_some(),
            Layer::Particles(_) => false,
        });
        ensure!(
            !clipped
                || hdr_format.has_color_aspect()
                    && hdr_format != wgpu::TextureFormat::Rg11b10Ufloat,
            "`clip` needs an HDR format with alpha (render.hdr_format = rgba16float or rgba32float)"
        );
        let input = gpu.create_module("scene/input.wgsl", spec.input_shader)?;
        let effect_mask = spec
            .effect_mask_shader
            .map(|m| gpu.create_module("scene/effect_mask.wgsl", m))
            .transpose()?;
        let no_clip_texture = gpu.scoped(|d| {
            upload_texture(gpu, d, "waywall no clip", &empty_texture(), TEXTURE_FORMAT)
        })?;
        let no_clip = no_clip_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let simulate = spec
            .layers
            .iter()
            .any(|l| matches!(l, Layer::Particles(_)))
            .then(|| gpu.create_module("particles/simulate.wgsl", spec.simulate_shader))
            .transpose()?;

        let uniform_entry = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let texture_entry = |binding, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };

        // 광원 버퍼: ambient 합 + point/directional마다 하나 (비어 있으면 무시되는 항목 하나).
        let lit = spec
            .layers
            .iter()
            .any(|l| matches!(l, Layer::Image(i) if i.lit));
        let light_count = spec
            .lights
            .iter()
            .filter(|l| !matches!(l, Light::Ambient { .. }))
            .count()
            .max(1);
        let lights_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("waywall scene lights"),
            size: ((LIGHTS_HEADER_FLOATS + LIGHT_FLOATS * light_count) * size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let flat_normal_texture = gpu.scoped(|d| {
            upload_texture(
                gpu,
                d,
                "waywall flat normal",
                &Texture {
                    width: 1,
                    height: 1,
                    data: FLAT_NORMAL.to_vec(),
                },
                NORMAL_FORMAT,
            )
        })?;
        let flat_normal = flat_normal_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let built = gpu
            .scoped(|d| {
                let layer_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("waywall scene layer"),
                    entries: &[
                        uniform_entry(0, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT),
                        texture_entry(1, true),
                        wgpu::BindGroupLayoutEntry {
                            binding: 2,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                        texture_entry(3, true),
                        wgpu::BindGroupLayoutEntry {
                            binding: 4,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        texture_entry(5, false),
                    ],
                });
                let post_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("waywall scene post"),
                    entries: &[
                        uniform_entry(0, wgpu::ShaderStages::FRAGMENT),
                        texture_entry(1, false),
                        texture_entry(2, true),
                        wgpu::BindGroupLayoutEntry {
                            binding: 3,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                    ],
                });
                let layer_pl = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("waywall scene layer"),
                    bind_group_layouts: &[Some(&layer_layout)],
                    immediate_size: 0,
                });
                let post_pl = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("waywall scene post"),
                    bind_group_layouts: &[Some(&post_layout)],
                    immediate_size: 0,
                });
                let pipelines = BLENDS.map(|b| {
                    spec.layers
                        .iter()
                        .any(|l| match l {
                            Layer::Image(i) => i.blend == b,
                            Layer::Shader(s) => s.blend == b,
                            Layer::Particles(_) => false,
                        })
                        .then(|| {
                            d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                                label: Some(name),
                                layout: Some(&layer_pl),
                                vertex: wgpu::VertexState {
                                    module: &image,
                                    entry_point: Some(VS_ENTRY),
                                    compilation_options: Default::default(),
                                    buffers: &[],
                                },
                                primitive: wgpu::PrimitiveState {
                                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                                    ..Default::default()
                                },
                                depth_stencil: None,
                                multisample: wgpu::MultisampleState::default(),
                                fragment: Some(wgpu::FragmentState {
                                    module: &image,
                                    entry_point: Some(FS_ENTRY),
                                    compilation_options: Default::default(),
                                    targets: &[Some(wgpu::ColorTargetState {
                                        format: hdr_format,
                                        blend: Some(blend_state(b)),
                                        write_mask: wgpu::ColorWrites::ALL,
                                    })],
                                }),
                                multiview_mask: None,
                                cache: None,
                            })
                        })
                });
                let post_pipeline = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(name),
                    layout: Some(&post_pl),
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
                        module: &post,
                        entry_point: Some(FS_ENTRY),
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
                let sampler = d.create_sampler(&wgpu::SamplerDescriptor {
                    label: Some("waywall scene"),
                    mag_filter: wgpu::FilterMode::Linear,
                    min_filter: wgpu::FilterMode::Linear,
                    ..Default::default()
                });
                let bindings = LayerBindings {
                    layout: &layer_layout,
                    sampler: &sampler,
                    flat_normal: &flat_normal,
                    lights: &lights_buffer,
                    no_clip: &no_clip,
                };
                let cropper = Cropper::new(gpu, d, &input);
                let effects = EffectShared::new(gpu, d, effect_mask.as_ref());
                let layers: Result<Vec<DrawLayer>> = spec
                    .layers
                    .into_iter()
                    .map(|l| {
                        Ok(match l {
                            Layer::Image(l) => DrawLayer::Image(Box::new(upload_layer(
                                gpu, d, &bindings, *l, &effects, &cropper,
                            )?)),
                            Layer::Particles(p) => {
                                DrawLayer::Particles(Box::new(GpuParticles::new(
                                    gpu,
                                    d,
                                    p,
                                    simulate.as_ref().context("simulate shader")?,
                                    hdr_format,
                                    &sampler,
                                )?))
                            }
                            Layer::Shader(s) => DrawLayer::Shader(Box::new(GpuShaderLayer::new(
                                gpu, d, s, hdr_format,
                            )?)),
                        })
                    })
                    .collect();
                (
                    pipelines,
                    post_pipeline,
                    post_layout,
                    layers,
                    layer_layout,
                    sampler,
                    cropper,
                    effects,
                )
            })
            .with_context(|| format!("{name}: failed to create the scene pipelines"))?;
        let (
            pipelines,
            post_pipeline,
            post_layout,
            layers,
            layer_layout,
            sampler,
            cropper,
            effects,
        ) = built;
        let layers =
            layers.with_context(|| format!("{name}: failed to create the scene layers"))?;

        let post_uniforms = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("waywall scene post"),
            size: (POST_FLOATS * size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bloom = spec
            .bloom_shaders
            .map(|(down, up)| -> Result<BloomPipelines> {
                let down = gpu.create_module("bloom/downsample.wgsl", down)?;
                let up = gpu.create_module("bloom/upsample.wgsl", up)?;
                gpu.scoped(|d| {
                    BloomPipelines::new(gpu, d, &down, &up, spec.bloom.threshold, spec.bloom.radius)
                })
            })
            .transpose()
            .with_context(|| format!("{name}: failed to create the bloom passes"))?;
        let post_values = [
            spec.exposure as f32,
            if bloom.is_some() {
                spec.bloom.intensity as f32
            } else {
                0.0
            },
            spec.vignette as f32,
            0.0,
        ];
        debug_assert_eq!(post_values.len(), POST_FLOATS);
        gpu.queue
            .write_buffer(&post_uniforms, 0, &f32_bytes(&post_values));
        let black = gpu.scoped(|d| {
            upload_texture(gpu, d, "waywall black", &empty_texture(), TEXTURE_FORMAT)
        })?;
        let post = Post {
            layout: post_layout,
            uniforms: post_uniforms,
            black: black.create_view(&wgpu::TextureViewDescriptor::default()),
            _black: black,
            sampler: gpu.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("waywall scene post"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            bloom,
        };
        let target = HdrTarget::new(gpu, hdr_format, &post, size)?;
        let [r, g, b, a] = spec.clear.to_linear_premultiplied();
        let max_parallax = layers
            .iter()
            .map(|l| l.parallax().abs())
            .fold(0.0, f64::max);
        tracing::debug!(
            layers = layers.len(),
            ?hdr_format,
            max_parallax,
            "scene ready"
        );
        let mut renderer = Self {
            size: spec.size,
            fit: spec.fit,
            clear: wgpu::Color { r, g, b, a },
            pointer_smoothing: spec.pointer_smoothing,
            settle_px: spec.settle_px,
            hdr_format,
            pipelines,
            layers,
            drawn: spec.drawn,
            isolated: HashMap::new(),
            cropper,
            effects,
            no_clip,
            _no_clip: no_clip_texture,
            layer_layout,
            sampler,
            flat_normal,
            _flat_normal: flat_normal_texture,
            lit,
            lights: spec.lights,
            lights_buffer,
            lights_written: Vec::new(),
            pointer_light: [spec.size[0] / 2.0, spec.size[1] / 2.0],
            post_pipeline,
            post,
            target,
            max_parallax,
            parallax: Parallax::default(),
        };
        renderer.size_resources(gpu)?;
        Ok(renderer)
    }

    /// 출력 크기에 따르는 자원: shader 레이어의 결과·입력 텍스처, 레이어 단독 결과.
    /// clip 대상 (`layers`의 인덱스, 오름차순, 중복 없음).
    fn clip_targets(&self) -> Vec<usize> {
        let mut out: Vec<usize> = self
            .layers
            .iter()
            .filter_map(|l| match l {
                DrawLayer::Image(l) => l.clip,
                DrawLayer::Shader(s) => s.quad.clip,
                DrawLayer::Particles(_) => None,
            })
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    fn size_resources(&mut self, gpu: &Gpu) -> Result<()> {
        let map = FitMap::new(self.fit, self.size, self.target.size);
        let mut wanted = self.clip_targets();
        for l in &self.layers {
            if let DrawLayer::Shader(s) = l {
                wanted.extend(s.layer_inputs());
            }
        }
        self.isolated = gpu
            .scoped(|d| {
                wanted
                    .into_iter()
                    .map(|j| {
                        let name = "waywall scene isolated layer";
                        (j, Target::new(d, name, self.hdr_format, self.target.size))
                    })
                    .collect()
            })
            .context("failed to create the layer input buffers")?;
        let bindings = LayerBindings {
            layout: &self.layer_layout,
            sampler: &self.sampler,
            flat_normal: &self.flat_normal,
            lights: &self.lights_buffer,
            no_clip: &self.no_clip,
        };
        let sources = Sources {
            below: &self.target.view,
            isolated: &self.isolated,
            cropper: Some(&self.cropper),
        };
        gpu.scoped(|d| {
            for l in &mut self.layers {
                match l {
                    DrawLayer::Shader(s) => s.resize(gpu, map, &bindings, &sources),
                    DrawLayer::Image(l) => {
                        if let Some(j) = l.clip {
                            let clip = sources.isolated.get(&j).map(|t| &t.view);
                            l.rebind(d, &bindings, clip);
                        }
                    }
                    DrawLayer::Particles(_) => {}
                }
            }
        })
        .context("failed to create the shader layer buffers")
    }

    /// 포인터가 움직이면 화면이 바뀔 수 있는지 (패럴랙스, 파티클 `pointer`).
    pub fn follows_pointer(&self) -> bool {
        self.max_parallax > 0.0
            || self.layers.iter().any(|l| match l {
                DrawLayer::Image(l) => l.follow.is_some(),
                DrawLayer::Shader(s) => s.quad.follow.is_some(),
                DrawLayer::Particles(_) => false,
            })
            || (self.lit
                && self
                    .lights
                    .iter()
                    .any(|l| matches!(l, Light::Point { position: None, .. })))
            || self
                .layers
                .iter()
                .any(|l| matches!(l, DrawLayer::Particles(p) if p.follows_pointer()))
    }

    /// 왼쪽 클릭에 반응하는지 (파티클 `burst`).
    pub fn wants_clicks(&self) -> bool {
        self.layers
            .iter()
            .any(|l| matches!(l, DrawLayer::Particles(p) if p.wants_clicks()))
    }

    pub fn resize(&mut self, gpu: &Gpu, size: (u32, u32)) -> Result<()> {
        if size != self.target.size {
            self.target = HdrTarget::new(gpu, self.hdr_format, &self.post, size)?;
            self.size_resources(gpu)?;
        }
        Ok(())
    }

    /// 한 장 그린다. 아직 움직이는 중(다음 프레임도 필요)이면 `true`.
    pub fn render(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        f: &FrameInfo<'_>,
    ) -> bool {
        let map = FitMap::new(self.fit, self.size, f.size);
        let mut moving = if self.max_parallax > 0.0 {
            let p = f.pointer;
            let goal = if p.inside {
                // 포인터는 왼쪽 아래 원점 렌더 px.
                let d = map.to_design([
                    f64::from(p.pos[0]),
                    f64::from(f.size.1) - f64::from(p.pos[1]),
                ]);
                [self.size[0] / 2.0 - d[0], self.size[1] / 2.0 - d[1]]
            } else {
                [0.0, 0.0]
            };
            // 설계 px 단위 허용 오차: 가장 많이 움직이는 레이어가 출력에서 settle_px 미만으로 움직이는 거리.
            let settle = self.settle_px / (map.scale[0].max(map.scale[1]) * self.max_parallax);
            self.parallax.step(
                goal,
                f64::from(f.time_delta),
                self.pointer_smoothing,
                settle,
            )
        } else {
            false
        };

        if self.lit {
            let p = f.pointer;
            if p.inside {
                self.pointer_light = map.to_design([
                    f64::from(p.pos[0]),
                    f64::from(f.size.1) - f64::from(p.pos[1]),
                ]);
            }
            let (values, changing) =
                light_values(&self.lights, self.pointer_light, f64::from(f.time));
            if values != self.lights_written {
                gpu.queue
                    .write_buffer(&self.lights_buffer, 0, &f32_bytes(&values));
                self.lights_written = values;
            }
            moving |= changing;
        }

        let shift = self.parallax.shift;
        // follow: 포인터(설계 px)를 향해. 정착 허용 오차는 패럴랙스와 같은 출력 px.
        let p = f.pointer;
        let pointer = p.inside.then(|| {
            map.to_design([
                f64::from(p.pos[0]),
                f64::from(f.size.1) - f64::from(p.pos[1]),
            ])
        });
        let settle = self.settle_px / map.scale[0].max(map.scale[1]);
        let dt = f64::from(f.time_delta);
        let follow = |q: &mut GpuLayer| {
            let layer_shift = [shift[0] * q.parallax, shift[1] * q.parallax];
            q.follow
                .as_mut()
                .is_some_and(|s| s.step(pointer, layer_shift, dt, settle))
        };
        for l in &mut self.layers {
            match l {
                DrawLayer::Image(l) => {
                    moving |= follow(l);
                    moving |= l.effects.is_some();
                    l.write_uniform(gpu, &map, shift);
                }
                DrawLayer::Shader(s) => {
                    follow(&mut s.quad);
                    s.quad.write_uniform(gpu, &map, shift);
                    // 셰이더는 시간에 따라 변한다.
                    moving = true;
                }
                DrawLayer::Particles(p) => moving |= p.update(gpu, map, shift, f),
            }
        }

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("waywall scene particles"),
                timestamp_writes: None,
            });
            for l in &self.layers {
                if let DrawLayer::Particles(p) = l {
                    p.simulate(&mut pass);
                }
            }
        }

        // 효과: 레이어 이미지 → 효과 결과 (레이어 사각형과 clip 대상 단독 그리기가 읽음).
        for l in &self.layers {
            if let DrawLayer::Image(l) = l
                && let Some(e) = &l.effects
            {
                e.render(gpu, encoder, &self.effects, Some(&self.cropper), f);
            }
        }
        // clip 대상을 단독으로. 아래 레이어부터라 clip 대상이 clip을 가져도 이미 준비되어 있다.
        for j in self.clip_targets() {
            if let Some(t) = self.isolated.get(&j) {
                let clear = wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT);
                draw_layers(
                    &self.pipelines,
                    [&self.layers[j]],
                    encoder,
                    &t.view,
                    clear,
                    map,
                );
            }
        }

        // 레이어 패스. "below"를 읽거나 직접 그리는 shader 레이어 앞에서 끊는다.
        let drawn = |range: std::ops::Range<usize>| {
            range.filter(|&i| self.drawn[i]).map(|i| &self.layers[i])
        };
        // 그릴 것이 없고 지울 것도 없으면 패스를 만들지 않는다(패스마다 CPU 비용).
        let flush = |encoder: &mut wgpu::CommandEncoder, range: std::ops::Range<usize>, load| {
            let mut layers = drawn(range).peekable();
            if layers.peek().is_some() || !matches!(load, wgpu::LoadOp::Load) {
                draw_layers(
                    &self.pipelines,
                    layers,
                    encoder,
                    &self.target.view,
                    load,
                    map,
                );
            }
        };
        let mut start = 0;
        let mut load = wgpu::LoadOp::Clear(self.clear);
        for (k, l) in self.layers.iter().enumerate() {
            let DrawLayer::Shader(s) = l else {
                continue;
            };
            let cropper = &self.cropper;
            if s.needs_below() || s.direct() {
                flush(encoder, start..k, load);
                load = wgpu::LoadOp::Load;
                // 직접 그리는 레이어는 합성 사각형이 없다.
                start = if s.direct() { k + 1 } else { k };
            }
            for j in s.layer_inputs() {
                if let Some(t) = self.isolated.get(&j) {
                    let clear = wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT);
                    draw_layers(
                        &self.pipelines,
                        [&self.layers[j]],
                        encoder,
                        &t.view,
                        clear,
                        map,
                    );
                }
            }
            if self.drawn[k] || !s.direct() {
                s.render(gpu, encoder, cropper, f, map, shift, &self.target.view);
            }
        }
        flush(encoder, start..self.layers.len(), load);

        if let (Some(b), Some(chain)) = (&self.post.bloom, &self.target.bloom) {
            b.render(encoder, chain);
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("waywall scene post"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.post_pipeline);
        pass.set_bind_group(0, &self.target.post_bind_group, &[]);
        pass.draw(0..3, 0..1);
        moving
    }
}

impl GpuLayer {
    /// 위치가 바뀌었을 때만 uniform을 쓴다.
    pub(super) fn write_uniform(&mut self, gpu: &Gpu, map: &FitMap, shift: [f64; 2]) {
        let values = self.uniform(map, shift);
        if self.written != Some(values) {
            gpu.queue
                .write_buffer(&self.uniforms, 0, &f32_bytes(&values));
            self.written = Some(values);
        }
    }

    fn uniform(&self, map: &FitMap, shift: [f64; 2]) -> [f32; LAYER_FLOATS] {
        quad_uniform(
            self.position,
            self.half,
            self.rotation,
            self.parallax,
            self.opacity,
            self.lit,
            map,
            shift,
            self.offset(),
            self.clip.is_some(),
        )
    }
}

/// image.wgsl `Layer`: clip = origin + u × axis_u + v × axis_v (u, v ∈ [0, 1], 텍스처 왼쪽 위가 (0, 0)).
#[allow(clippy::too_many_arguments)]
fn quad_uniform(
    position: [f64; 2],
    half: [f64; 2],
    rotation: [f64; 2],
    parallax: f64,
    opacity: f64,
    lit: bool,
    map: &FitMap,
    shift: [f64; 2],
    offset: [f64; 2],
    clip: bool,
) -> [f32; LAYER_FLOATS] {
    let c = [
        position[0] + shift[0] * parallax + offset[0],
        position[1] + shift[1] * parallax + offset[1],
    ];
    let [cos, sin] = rotation;
    // y가 아래로 커지는 좌표에서 (cos, sin)은 시계 방향 회전.
    let u = [cos * half[0] * 2.0, sin * half[0] * 2.0];
    let v = [-sin * half[1] * 2.0, cos * half[1] * 2.0];
    let corner = [c[0] - (u[0] + v[0]) / 2.0, c[1] - (u[1] + v[1]) / 2.0];
    let origin = map.to_clip(corner);
    let au = map.vector_to_clip(u);
    let av = map.vector_to_clip(v);
    [
        au[0] as f32,
        au[1] as f32,
        av[0] as f32,
        av[1] as f32,
        origin[0] as f32,
        origin[1] as f32,
        opacity as f32,
        f32::from(u8::from(lit)),
        corner[0] as f32,
        corner[1] as f32,
        u[0] as f32,
        u[1] as f32,
        v[0] as f32,
        v[1] as f32,
        cos as f32,
        sin as f32,
        f32::from(u8::from(clip)),
        0.0,
        0.0,
        0.0,
    ]
}

fn upload_layer(
    gpu: &Gpu,
    d: &wgpu::Device,
    b: &LayerBindings<'_>,
    l: ImageLayer,
    fx: &EffectShared,
    cropper: &Cropper,
) -> Result<GpuLayer> {
    let texture = upload_texture(gpu, d, &l.id, &l.texture, TEXTURE_FORMAT);
    let t = &l.texture;
    let size = [
        f64::from(t.width) * l.scale[0],
        f64::from(t.height) * l.scale[1],
    ];
    let mut layer = GpuLayer::new(
        d,
        &l.id,
        LayerPlacement {
            blend: l.blend,
            opacity: l.opacity,
            parallax: l.parallax,
            position: l.position,
            size,
            rotation: l.rotation,
            lit: l.lit,
        },
    );
    layer.clip = l.clip;
    layer.follow = l.follow.map(FollowState::new);
    let normal = l
        .normal_map
        .as_ref()
        .map(|n| upload_texture(gpu, d, &l.id, n, NORMAL_FORMAT));
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let normal_view = normal
        .as_ref()
        .map(|n| n.create_view(&wgpu::TextureViewDescriptor::default()));
    if !l.effects.is_empty() {
        let resolution = (t.width, t.height);
        let effects = GpuEffects::new(
            gpu,
            d,
            fx,
            Some(cropper),
            l.effects,
            &view,
            resolution,
            size,
            &l.id,
        )?;
        layer.bind(d, b, effects.result(), normal_view.as_ref(), None);
        layer.effects = Some(Box::new(effects));
    } else {
        layer.bind(d, b, &view, normal_view.as_ref(), None);
    }
    layer._texture = Some(texture);
    layer._normal = normal;
    Ok(layer)
}

/// image.wgsl 바인딩 중 레이어마다 같은 것.
pub(super) struct LayerBindings<'a> {
    pub layout: &'a wgpu::BindGroupLayout,
    pub sampler: &'a wgpu::Sampler,
    /// 노멀맵이 없는 레이어의 노멀맵.
    pub flat_normal: &'a wgpu::TextureView,
    pub lights: &'a wgpu::Buffer,
    /// clip이 없는 레이어에 묶는 1×1 (읽지 않음).
    pub no_clip: &'a wgpu::TextureView,
}

/// 장면에 사각형으로 합성되는 레이어의 배치 (image 레이어, shader 레이어 결과).
pub(super) struct LayerPlacement {
    pub blend: Blend,
    pub opacity: f64,
    pub parallax: f64,
    /// 중심 (설계 px).
    pub position: [f64; 2],
    /// 회전 전 화면 크기 (설계 px).
    pub size: [f64; 2],
    /// 도, 시계 방향.
    pub rotation: f64,
    pub lit: bool,
}

impl GpuLayer {
    pub(super) fn new(d: &wgpu::Device, label: &str, p: LayerPlacement) -> Self {
        let uniforms = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (LAYER_FLOATS * size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let rad = p.rotation.to_radians();
        GpuLayer {
            blend: p.blend,
            opacity: p.opacity,
            parallax: p.parallax,
            position: p.position,
            half: [p.size[0] / 2.0, p.size[1] / 2.0],
            rotation: [rad.cos(), rad.sin()],
            uniforms,
            bind_group: None,
            written: None,
            lit: p.lit,
            clip: None,
            follow: None,
            views: None,
            effects: None,
            _texture: None,
            _normal: None,
        }
    }

    /// `follow` 이동량 (설계 px).
    pub(super) fn offset(&self) -> [f64; 2] {
        self.follow.as_ref().map_or([0.0; 2], |f| f.smooth.shift)
    }

    /// clip 대상의 단독 결과가 바뀌었을 때 (크기 변경) 같은 텍스처로 다시 묶는다.
    pub(super) fn rebind(
        &mut self,
        d: &wgpu::Device,
        b: &LayerBindings<'_>,
        clip: Option<&wgpu::TextureView>,
    ) {
        if let Some((view, normal)) = self.views.take() {
            self.bind(d, b, &view, normal.as_ref(), clip);
        }
    }

    /// 그릴 텍스처를 묶는다 (바뀔 때마다 다시).
    pub(super) fn bind(
        &mut self,
        d: &wgpu::Device,
        b: &LayerBindings<'_>,
        view: &wgpu::TextureView,
        normal: Option<&wgpu::TextureView>,
        clip: Option<&wgpu::TextureView>,
    ) {
        self.bind_group = Some(d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waywall scene layer"),
            layout: b.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(b.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(normal.unwrap_or(b.flat_normal)),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: b.lights.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(clip.unwrap_or(b.no_clip)),
                },
            ],
        }));
        self.views = Some((view.clone(), normal.cloned()));
    }
}

/// RGBA8 텍스처를 GPU에 올린다. 씬 텍스처는 `TEXTURE_FORMAT`(premultiplied sRGB).
pub fn upload_texture(
    gpu: &Gpu,
    d: &wgpu::Device,
    label: &str,
    t: &Texture,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    let extent = wgpu::Extent3d {
        width: t.width,
        height: t.height,
        depth_or_array_layers: 1,
    };
    let texture = d.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &t.data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * t.width),
            rows_per_image: Some(t.height),
        },
        extent,
    );
    texture
}

impl HdrTarget {
    fn new(gpu: &Gpu, format: wgpu::TextureFormat, post: &Post, size: (u32, u32)) -> Result<Self> {
        gpu.scoped(|d| {
            let texture = d.create_texture(&wgpu::TextureDescriptor {
                label: Some("waywall scene hdr"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bloom = post.bloom.as_ref().map(|b| b.chain(d, &view, size));
            let bloom_view = bloom
                .as_ref()
                .and_then(BloomChain::result)
                .unwrap_or(&post.black);
            let post_bind_group = d.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("waywall scene post"),
                layout: &post.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: post.uniforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(bloom_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&post.sampler),
                    },
                ],
            });
            Self {
                view,
                post_bind_group,
                size,
                bloom,
            }
        })
        .context("failed to create the HDR buffer")
    }
}

/// 후처리 자원 중 크기와 관계없는 것.
struct Post {
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    /// `bloom.intensity > 0`일 때만.
    bloom: Option<BloomPipelines>,
    /// 블룸이 없을 때 묶는 1×1 검정.
    black: wgpu::TextureView,
    _black: wgpu::Texture,
    sampler: wgpu::Sampler,
}

/// `layers`를 `view`에 차례로 그리는 렌더 패스 하나. contain이면 설계 영역만(scissor).
fn draw_layers<'a>(
    pipelines: &[Option<wgpu::RenderPipeline>; BLENDS.len()],
    layers: impl IntoIterator<Item = &'a DrawLayer>,
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
    map: FitMap,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("waywall scene layers"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    let Some((x, y, w, h)) = map.scissor() else {
        return;
    };
    pass.set_scissor_rect(x, y, w, h);
    for l in layers {
        match l {
            DrawLayer::Image(l) => draw_quad(&mut pass, pipelines, l),
            DrawLayer::Shader(s) => draw_quad(&mut pass, pipelines, &s.quad),
            DrawLayer::Particles(p) => p.draw(&mut pass),
        }
    }
}

fn draw_quad(
    pass: &mut wgpu::RenderPass<'_>,
    pipelines: &[Option<wgpu::RenderPipeline>; BLENDS.len()],
    l: &GpuLayer,
) {
    let (Some(pipeline), Some(bind_group)) = (&pipelines[blend_index(l.blend)], &l.bind_group)
    else {
        return;
    };
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..4, 0..1);
}

/// premultiplied 알파 기준 블렌드 (docs/wallpaper-format.md#색과-알파).
pub(super) fn blend_state(b: Blend) -> wgpu::BlendState {
    use wgpu::BlendFactor as F;
    let (src, dst) = match b {
        Blend::Normal => (F::One, F::OneMinusSrcAlpha),
        Blend::Add => (F::One, F::One),
        Blend::Multiply => (F::Dst, F::OneMinusSrcAlpha),
        Blend::Screen => (F::One, F::OneMinusSrc),
    };
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: src,
            dst_factor: dst,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent::OVER,
    }
}

fn blend_index(b: Blend) -> usize {
    BLENDS.iter().position(|&x| x == b).unwrap_or_default()
}

pub(super) fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_ne_bytes()).collect()
}

/// image.wgsl `Lights` 버퍼 값과, 시간에 따라 변하는지(`flicker`).
fn light_values(lights: &[Light], pointer: [f64; 2], time: f64) -> (Vec<f32>, bool) {
    let mut ambient = [0.0; 3];
    let mut items = Vec::new();
    let mut changing = false;
    for (i, l) in lights.iter().enumerate() {
        match *l {
            Light::Ambient { color } => {
                for (a, c) in ambient.iter_mut().zip(color) {
                    *a += c;
                }
            }
            Light::Point {
                position,
                color,
                radius,
                height,
                flicker,
                flicker_speed,
            } => {
                let k = if flicker > 0.0 {
                    changing = true;
                    1.0 + flicker * smooth_noise(time * flicker_speed, i as u32)
                } else {
                    1.0
                };
                let [x, y] = position.unwrap_or(pointer);
                items.extend([
                    color[0] * k,
                    color[1] * k,
                    color[2] * k,
                    0.0,
                    x,
                    y,
                    height,
                    radius,
                ]);
            }
            Light::Directional { direction, color } => {
                let [dx, dy, dz] = direction;
                items.extend([color[0], color[1], color[2], 1.0, dx, dy, dz, 0.0]);
            }
        }
    }
    if items.is_empty() {
        // 버퍼의 빈 항목: kind가 0·1이 아니면 셰이더가 무시한다.
        items.extend([0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0]);
    }
    let mut out = vec![ambient[0], ambient[1], ambient[2], 0.0];
    out.extend(items);
    debug_assert_eq!((out.len() - LIGHTS_HEADER_FLOATS) % LIGHT_FLOATS, 0);
    (out.into_iter().map(|v| v as f32).collect(), changing)
}

/// −1–1의 매끄러운 1차원 노이즈 (정수마다 무작위 값, 사이는 smoothstep 보간). `seed`마다 다른 곡선.
fn smooth_noise(t: f64, seed: u32) -> f64 {
    let hash = |i: i64| {
        let mut h = (i as u64 ^ (u64::from(seed) << 32)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        h ^= h >> 29;
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^= h >> 32;
        (h >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    let i = t.floor();
    let f = t - i;
    let s = f * f * (3.0 - 2.0 * f);
    let a = hash(i as i64);
    a + (hash(i as i64 + 1) - a) * s
}

/// 설계 좌표 ↔ 렌더 픽셀 ↔ clip 공간.
#[derive(Debug, Clone, Copy)]
pub(super) struct FitMap {
    pub(super) scale: [f64; 2],
    offset: [f64; 2],
    design: [f64; 2],
    target: [f64; 2],
}

impl FitMap {
    fn new(fit: Fit, design: [f64; 2], target: (u32, u32)) -> Self {
        let t = [f64::from(target.0), f64::from(target.1)];
        let (sx, sy) = (t[0] / design[0], t[1] / design[1]);
        let scale = match fit {
            Fit::Fill => [sx, sy],
            Fit::Cover => [sx.max(sy); 2],
            Fit::Contain => [sx.min(sy); 2],
        };
        let offset = [
            (t[0] - design[0] * scale[0]) / 2.0,
            (t[1] - design[1] * scale[1]) / 2.0,
        ];
        Self {
            scale,
            offset,
            design,
            target: t,
        }
    }

    pub(super) fn to_render(self, d: [f64; 2]) -> [f64; 2] {
        [
            d[0] * self.scale[0] + self.offset[0],
            d[1] * self.scale[1] + self.offset[1],
        ]
    }

    pub(super) fn to_design(self, r: [f64; 2]) -> [f64; 2] {
        [
            (r[0] - self.offset[0]) / self.scale[0],
            (r[1] - self.offset[1]) / self.scale[1],
        ]
    }

    pub(super) fn to_clip(self, d: [f64; 2]) -> [f64; 2] {
        let r = self.to_render(d);
        [
            r[0] / self.target[0] * 2.0 - 1.0,
            1.0 - r[1] / self.target[1] * 2.0,
        ]
    }

    pub(super) fn vector_to_clip(self, v: [f64; 2]) -> [f64; 2] {
        [
            v[0] * self.scale[0] / self.target[0] * 2.0,
            -v[1] * self.scale[1] / self.target[1] * 2.0,
        ]
    }

    /// 설계 영역 ∩ 렌더 영역 (px). 비어 있으면 `None`.
    pub(super) fn scissor(self) -> Option<(u32, u32, u32, u32)> {
        let a = self.to_render([0.0, 0.0]);
        let b = self.to_render(self.design);
        let clamp = |v: f64, max: f64| v.round().clamp(0.0, max) as u32;
        let (x0, y0) = (clamp(a[0], self.target[0]), clamp(a[1], self.target[1]));
        let (x1, y1) = (clamp(b[0], self.target[0]), clamp(b[1], self.target[1]));
        (x1 > x0 && y1 > y0).then(|| (x0, y0, x1 - x0, y1 - y0))
    }
}

/// 패럴랙스 이동량(설계 px, `parallax` = 1 기준)을 목표로 지수적으로 따라간다.
#[derive(Debug, Clone, Copy, Default)]
struct Parallax {
    shift: [f64; 2],
    /// 직전 프레임에서 움직이는 중이었다.
    moving: bool,
}

impl Parallax {
    /// 아직 목표에 닿지 않았으면 `true`.
    fn step(&mut self, goal: [f64; 2], dt: f64, tau: f64, settle: f64) -> bool {
        // 잠들어 있다 깨어난 첫 프레임의 dt는 잠든 시간이라 쓰지 않는다.
        let dt = if self.moving { dt } else { 0.0 };
        let k = if tau > 0.0 {
            1.0 - (-dt / tau).exp()
        } else {
            1.0
        };
        for (s, g) in self.shift.iter_mut().zip(goal) {
            *s += (g - *s) * k;
        }
        let far = self
            .shift
            .iter()
            .zip(goal)
            .any(|(s, g)| (g - s).abs() >= settle);
        if !far {
            self.shift = goal;
        }
        self.moving = far;
        far
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    #[test]
    fn fit_maps() {
        let d = [1920.0, 1080.0];
        // 16:10 출력에 16:9 설계.
        let cover = FitMap::new(Fit::Cover, d, (1920, 1200));
        assert!(close(cover.to_render([960.0, 540.0]), [960.0, 600.0]));
        assert_eq!(cover.scissor(), Some((0, 0, 1920, 1200)));
        let contain = FitMap::new(Fit::Contain, d, (1920, 1200));
        assert!(close(contain.to_render([0.0, 0.0]), [0.0, 60.0]));
        assert_eq!(
            contain.scissor(),
            Some((0, 60, 1920, 1080)),
            "letterbox excluded"
        );
        let fill = FitMap::new(Fit::Fill, d, (960, 1200));
        assert!(close(fill.to_render([1920.0, 1080.0]), [960.0, 1200.0]));
        let p = [123.0, 456.0];
        assert!(close(cover.to_design(cover.to_render(p)), p));
        assert!(close(contain.to_clip([0.0, 0.0]), [-1.0, 0.9]));
    }

    #[test]
    fn layer_quad_covers_its_rect() {
        let map = FitMap::new(Fit::Fill, [100.0, 100.0], (100, 100));
        let quad = |degrees: f64, parallax, shift| {
            let rad = f64::to_radians(degrees);
            quad_uniform(
                [50.0, 50.0],
                [10.0, 20.0],
                [rad.cos(), rad.sin()],
                parallax,
                1.0,
                true,
                &map,
                shift,
                [0.0; 2],
                false,
            )
        };
        let u = quad(0.0, 0.0, [0.0, 0.0]);
        // 왼쪽 위 모서리 (40, 30) → clip (-0.2, 0.4), 가로 20px, 세로 40px.
        let e = [0.4, 0.0, 0.0, -0.8, -0.2, 0.4];
        for (a, b) in u.iter().zip(e) {
            assert!((a - b).abs() < 1e-6, "{u:?}");
        }
        // 같은 사각형의 설계 좌표: 모서리 (40, 30), 축 (20, 0)·(0, 40), lit.
        assert_eq!(&u[7..14], &[1.0, 40.0, 30.0, 20.0, 0.0, 0.0, 40.0]);
        let u = quad(0.0, 0.5, [10.0, -4.0]);
        assert!(
            (u[4] - (-0.1)).abs() < 1e-6 && (u[5] - 0.44).abs() < 1e-6,
            "{u:?}"
        );
        // 90° 시계 방향: u축이 아래(clip -y)를 향함.
        let u = quad(90.0, 0.0, [0.0, 0.0]);
        assert!(u[0].abs() < 1e-6 && (u[1] + 0.4).abs() < 1e-6, "{u:?}");
    }

    #[test]
    fn lights_pack_and_flicker() {
        let lights = [
            Light::Ambient {
                color: [0.1, 0.2, 0.3],
            },
            Light::Ambient {
                color: [0.1, 0.0, 0.0],
            },
            Light::Point {
                position: None,
                color: [1.0, 1.0, 1.0],
                radius: 300.0,
                height: 50.0,
                flicker: 0.0,
                flicker_speed: 8.0,
            },
            Light::Directional {
                direction: [1.0, 1.0, -1.0],
                color: [0.5, 0.5, 0.5],
            },
        ];
        let (v, changing) = light_values(&lights, [10.0, 20.0], 0.0);
        assert!(!changing);
        assert_eq!(v.len(), LIGHTS_HEADER_FLOATS + 2 * LIGHT_FLOATS);
        assert!((v[0] - 0.2).abs() < 1e-6, "ambient sums");
        assert_eq!(
            &v[4..12],
            &[1.0, 1.0, 1.0, 0.0, 10.0, 20.0, 50.0, 300.0],
            "pointer position"
        );
        assert_eq!(v[15], 1.0, "directional kind");
        let (v, _) = light_values(&[], [0.0; 2], 0.0);
        assert_eq!(
            v.len(),
            LIGHTS_HEADER_FLOATS + LIGHT_FLOATS,
            "one ignored item"
        );
        assert_eq!(v[7], -1.0);

        let torch = [Light::Point {
            position: Some([0.0, 0.0]),
            color: [1.0, 1.0, 1.0],
            radius: 1.0,
            height: 1.0,
            flicker: 0.5,
            flicker_speed: 4.0,
        }];
        let samples: Vec<f32> = (0..200)
            .map(|i| light_values(&torch, [0.0; 2], f64::from(i) * 0.01).0[4])
            .collect();
        assert!(light_values(&torch, [0.0; 2], 0.0).1);
        assert!(
            samples.iter().all(|k| (0.5..=1.5).contains(k)),
            "within 1 ± flicker"
        );
        let max_step = samples
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(max_step < 0.1, "smooth ({max_step})");
        assert!(
            samples.iter().any(|k| (k - samples[0]).abs() > 0.05),
            "varies"
        );
    }

    #[test]
    fn follow_points_toward_the_pointer_within_limits() {
        let spec = Follow {
            anchor: [100.0, 100.0],
            max_offset: [10.0, 4.0],
            range: 200.0,
            smoothing: 0.0,
            on_leave: OnLeave::Center,
        };
        let mut f = FollowState::new(spec);
        // 깨어난 첫 프레임은 dt = 0이라 움직이지 않는다 (패럴랙스와 같음).
        f.step(Some([500.0, 100.0]), [0.0; 2], 0.016, 0.01);
        assert!(!f.step(Some([500.0, 100.0]), [0.0; 2], 0.016, 0.01));
        assert_eq!(f.smooth.shift, [10.0, 0.0], "far right: full x offset");
        f.step(Some([100.0, 200.0]), [0.0; 2], 0.016, 0.01);
        assert!(
            (f.smooth.shift[1] - 2.0).abs() < 1e-9,
            "half range: half offset ({:?})",
            f.smooth.shift
        );
        // 기준점은 패럴랙스 이동을 따라간다.
        f.step(Some([150.0, 100.0]), [50.0, 0.0], 0.016, 0.01);
        assert_eq!(f.smooth.shift, [0.0, 0.0]);
        f.step(Some([500.0, 100.0]), [0.0; 2], 0.016, 0.01);
        f.step(None, [0.0; 2], 0.016, 0.01);
        assert_eq!(f.smooth.shift, [0.0, 0.0], "center on leave");
        let mut s = FollowState::new(Follow {
            on_leave: OnLeave::Stay,
            ..spec
        });
        s.step(Some([500.0, 100.0]), [0.0; 2], 0.016, 0.01);
        s.step(Some([500.0, 100.0]), [0.0; 2], 0.016, 0.01);
        s.step(None, [0.0; 2], 0.016, 0.01);
        assert_eq!(s.smooth.shift, [10.0, 0.0], "stay on leave");
    }

    #[test]
    fn parallax_settles_and_sleeps() {
        let mut p = Parallax::default();
        // 깨어난 첫 프레임은 움직이지 않지만 다음 프레임을 요청한다.
        assert!(p.step([100.0, 0.0], 10.0, 0.1, 0.5));
        assert_eq!(p.shift, [0.0, 0.0]);
        let mut frames = 0;
        while p.step([100.0, 0.0], 1.0 / 60.0, 0.1, 0.5) {
            frames += 1;
            assert!(frames < 1000);
        }
        assert_eq!(p.shift, [100.0, 0.0], "snaps to the goal when settled");
        assert!(frames > 10, "smoothing takes several frames ({frames})");
        assert!(!p.step([100.0, 0.0], 5.0, 0.1, 0.5), "stays asleep");
        let mut q = Parallax {
            moving: true,
            ..Default::default()
        };
        assert!(!q.step([3.0, 4.0], 0.016, 0.0, 0.5), "tau 0 = immediate");
        assert_eq!(q.shift, [3.0, 4.0]);
    }
}
