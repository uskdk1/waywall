//! image 레이어 효과(`effects`)의 GPU 쪽 (docs/architecture.md#씬-렌더러).
//!
//! 이미지 크기의 `Rgba16Float` 텍스처 두 장을 번갈아 쓴다: 원본 → 효과 1 → 효과 2 … 레이어 사각형은 마지막 결과를 읽는다.

use std::collections::HashMap;

use anyhow::Result;
use waywall_proto::config::ChannelDefaults;

use super::render::{TEXTURE_FORMAT, upload_texture};
use super::shader_layer::{Cropper, OUTPUT_FORMAT, Target};
use super::{EffectKind, EffectSpec, Texture};
use crate::backend::shader::{Draw, ShaderRenderer, empty_texture, frame_inputs};
use crate::backend::view::FrameInfo;
use crate::gpu::shadertoy::{Language, UniformLayout};
use crate::gpu::{FULLSCREEN_ENTRY, Gpu};

/// 효과 파일·`scene/effect_mask.wgsl`의 엔트리 (인터페이스).
const FS_ENTRY: &str = "fs_main";
/// 효과 uniform 중 waywall이 채우는 멤버 (docs/wallpaper-format.md#레이어-효과-effects).
const TIME: &str = "time";
const SIZE: &str = "size";
const RESOLUTION: &str = "resolution";
/// 마스크가 없는 효과에 묶는 1×1 마스크 (효과 100%).
const FULL_MASK: [u8; 4] = [u8::MAX; 4];
/// 마스크: 값 그대로(선형).
const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// 크기와 관계없이 씬에 하나: 효과 파일 바인딩 배치와 마스크 섞기 패스.
pub struct EffectShared {
    builtin_layout: wgpu::BindGroupLayout,
    mix: Option<(wgpu::RenderPipeline, wgpu::BindGroupLayout)>,
    sampler: wgpu::Sampler,
}

impl EffectShared {
    pub fn new(gpu: &Gpu, d: &wgpu::Device, mix_module: Option<&wgpu::ShaderModule>) -> Self {
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let builtin_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waywall effect"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture(1),
                sampler_entry(2),
                texture(3),
            ],
        });
        let mix = mix_module.map(|module| {
            let layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("waywall effect mask"),
                entries: &[texture(0), texture(1), texture(2), sampler_entry(3)],
            });
            (pipeline(gpu, d, &layout, module), layout)
        });
        let sampler = d.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("waywall effect"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            builtin_layout,
            mix,
            sampler,
        }
    }
}

fn pipeline(
    gpu: &Gpu,
    d: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    module: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    let pl = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("waywall effect"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("waywall effect"),
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
            module,
            entry_point: Some(FS_ENTRY),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: OUTPUT_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

// 효과마다 하나라 개수가 적어 변형 간 크기 차이는 문제되지 않는다.
#[allow(clippy::large_enum_variant)]
enum Step {
    Builtin {
        pipeline: wgpu::RenderPipeline,
        uniforms: wgpu::Buffer,
        layout: UniformLayout,
        values: HashMap<String, Vec<f32>>,
        group: wgpu::BindGroup,
    },
    Shader {
        /// GLSL이면 입력을 상하 반전한 사본 (iChannel0은 uv (0, 0)이 왼쪽 아래).
        flipped: Option<(Target, wgpu::Buffer, wgpu::BindGroup)>,
        renderer: Box<ShaderRenderer>,
        /// 셰이더 결과 (마스크로 섞기 전).
        raw: Target,
        mix_group: wgpu::BindGroup,
        channel_resolution: Vec<[f32; 3]>,
        _textures: Vec<wgpu::Texture>,
    },
}

pub struct GpuEffects {
    steps: Vec<Step>,
    /// 번갈아 쓰는 결과 텍스처. 효과 i는 `targets[i % 2]`에 쓴다.
    targets: [Target; 2],
    /// 레이어의 화면 크기 (설계 px).
    size: [f64; 2],
    resolution: (u32, u32),
    _masks: Vec<wgpu::Texture>,
}

impl GpuEffects {
    /// `source`: 레이어 원본 텍스처. `cropper`는 GLSL shader 효과가 있을 때 필요.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        gpu: &Gpu,
        d: &wgpu::Device,
        shared: &EffectShared,
        cropper: Option<&Cropper>,
        specs: Vec<EffectSpec>,
        source: &wgpu::TextureView,
        resolution: (u32, u32),
        size: [f64; 2],
        label: &str,
    ) -> Result<Self> {
        let targets = [
            Target::new(d, label, OUTPUT_FORMAT, resolution),
            Target::new(d, label, OUTPUT_FORMAT, resolution),
        ];
        let mut steps = Vec::with_capacity(specs.len());
        let mut masks = Vec::new();
        for (i, spec) in specs.into_iter().enumerate() {
            let input = if i == 0 {
                source
            } else {
                &targets[(i - 1) % 2].view
            };
            let mask = upload_texture(
                gpu,
                d,
                label,
                spec.mask.as_ref().unwrap_or(&Texture {
                    width: 1,
                    height: 1,
                    data: FULL_MASK.to_vec(),
                }),
                MASK_FORMAT,
            );
            let mask_view = mask.create_view(&wgpu::TextureViewDescriptor::default());
            masks.push(mask);
            steps.push(match spec.kind {
                EffectKind::Builtin {
                    name,
                    module,
                    layout,
                    values,
                } => {
                    let module = gpu.create_module(&name, module)?;
                    let uniforms = d.create_buffer(&wgpu::BufferDescriptor {
                        label: Some(&name),
                        size: layout.buffer_size(),
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    let group = d.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some(&name),
                        layout: &shared.builtin_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: uniforms.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(input),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::Sampler(&shared.sampler),
                            },
                            wgpu::BindGroupEntry {
                                binding: 3,
                                resource: wgpu::BindingResource::TextureView(&mask_view),
                            },
                        ],
                    });
                    Step::Builtin {
                        pipeline: pipeline(gpu, d, &shared.builtin_layout, &module),
                        uniforms,
                        layout,
                        values,
                        group,
                    }
                }
                EffectKind::Shader {
                    compiled,
                    props,
                    lang,
                    channels,
                    sampling,
                } => shader_step(
                    gpu,
                    d,
                    shared,
                    cropper,
                    ShaderStep {
                        compiled,
                        props,
                        lang,
                        channels,
                        sampling,
                    },
                    input,
                    &mask_view,
                    resolution,
                    label,
                )?,
            });
        }
        Ok(Self {
            steps,
            targets,
            size,
            resolution,
            _masks: masks,
        })
    }

    /// 레이어 사각형이 읽을 최종 결과.
    pub fn result(&self) -> &wgpu::TextureView {
        &self.targets[(self.steps.len().max(1) - 1) % 2].view
    }

    pub fn render(
        &self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        shared: &EffectShared,
        cropper: Option<&Cropper>,
        f: &FrameInfo<'_>,
    ) {
        let time = [f.time];
        let size = self.size.map(|v| v as f32);
        let resolution = [self.resolution.0 as f32, self.resolution.1 as f32];
        for (i, step) in self.steps.iter().enumerate() {
            let target = &self.targets[i % 2];
            match step {
                Step::Builtin {
                    pipeline,
                    uniforms,
                    layout,
                    values,
                    group,
                } => {
                    let bytes = layout.write_named(|name| match name {
                        TIME => Some(&time[..]),
                        SIZE => Some(&size[..]),
                        RESOLUTION => Some(&resolution[..]),
                        _ => values.get(name).map(Vec::as_slice),
                    });
                    gpu.queue.write_buffer(uniforms, 0, &bytes);
                    fullscreen(encoder, &target.view, pipeline, group);
                }
                Step::Shader {
                    flipped,
                    renderer,
                    raw,
                    mix_group,
                    channel_resolution,
                    ..
                } => {
                    if let (Some((t, uniforms, group)), Some(c)) = (flipped, cropper) {
                        c.run(gpu, encoder, t, uniforms, group, [0, 0], true, 1.0);
                    }
                    let inputs = frame_inputs(
                        f,
                        self.resolution,
                        [0.0, 0.0],
                        1.0,
                        channel_resolution.clone(),
                    );
                    let draw = Draw {
                        target: &raw.view,
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        scissor: None,
                    };
                    renderer.render(gpu, encoder, draw, &inputs, "waywall effect");
                    // 마스크 섞기 파이프라인은 shader 효과가 있으면 항상 있다.
                    if let Some((pipeline, _)) = &shared.mix {
                        fullscreen(encoder, &target.view, pipeline, mix_group);
                    }
                }
            }
        }
    }
}

struct ShaderStep {
    compiled: crate::gpu::shadertoy::Compiled,
    props: Vec<crate::properties::Property>,
    lang: Language,
    channels: Vec<Option<Texture>>,
    sampling: Vec<ChannelDefaults>,
}

#[allow(clippy::too_many_arguments)]
fn shader_step(
    gpu: &Gpu,
    d: &wgpu::Device,
    shared: &EffectShared,
    cropper: Option<&Cropper>,
    s: ShaderStep,
    input: &wgpu::TextureView,
    mask: &wgpu::TextureView,
    resolution: (u32, u32),
    label: &str,
) -> Result<Step> {
    let mut renderer = ShaderRenderer::new(
        gpu,
        s.compiled,
        s.props,
        s.sampling,
        OUTPUT_FORMAT,
        None,
        label,
    )?;
    let flipped = match (s.lang, cropper) {
        (Language::Glsl, Some(c)) => {
            let t = Target::new(d, label, OUTPUT_FORMAT, resolution);
            let uniforms = c.uniforms(d, label);
            let group = c.bind(d, input, &uniforms);
            Some((t, uniforms, group))
        }
        _ => None,
    };
    let mut textures = Vec::new();
    let mut views = Vec::new();
    let mut channel_resolution = vec![[0.0; 3]; s.channels.len()];
    channel_resolution[0] = [resolution.0 as f32, resolution.1 as f32, 1.0];
    for (i, ch) in s.channels.iter().enumerate().skip(1) {
        let t = ch.as_ref();
        if let Some(t) = t {
            channel_resolution[i] = [t.width as f32, t.height as f32, 1.0];
        }
        let texture = upload_texture(gpu, d, label, t.unwrap_or(&empty_texture()), TEXTURE_FORMAT);
        views.push(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        textures.push(texture);
    }
    let ch0 = flipped.as_ref().map_or(input, |(t, ..)| &t.view);
    let mut bound = vec![ch0];
    bound.extend(views.iter());
    renderer.bind(gpu, &bound);
    let raw = Target::new(d, label, OUTPUT_FORMAT, resolution);
    let (_, mix_layout) = shared
        .mix
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("effect mask shader missing"))?;
    let mix_group = d.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("waywall effect mask"),
        layout: mix_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(input),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&raw.view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(mask),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(&shared.sampler),
            },
        ],
    });
    Ok(Step::Shader {
        flipped,
        renderer: Box::new(renderer),
        raw,
        mix_group,
        channel_resolution,
        _textures: textures,
    })
}

fn fullscreen(
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    group: &wgpu::BindGroup,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("waywall effect"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[]);
    pass.draw(0..3, 0..1);
}
