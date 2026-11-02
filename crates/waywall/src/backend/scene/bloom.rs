//! 블룸: HDR 장면 → 밝은 부분만 남기며 절반씩 축소 → 작은 단계부터 키우며 더함 (docs/architecture.md#씬-렌더러).
//!
//! 후처리는 가장 큰 단계(렌더 크기의 절반)를 읽어 `intensity`배로 더한다.

use super::shader_layer::{OUTPUT_FORMAT as LEVEL_FORMAT, Target};
use crate::gpu::{FULLSCREEN_ENTRY, Gpu};

/// `bloom/downsample.wgsl`, `bloom/upsample.wgsl`의 엔트리 (인터페이스).
const FS_ENTRY: &str = "fs_main";
/// downsample.wgsl `Down`: threshold, first, 패딩 (32비트 단어 4개).
const DOWN_WORDS: usize = 4;

/// 크기와 관계없는 부분: 파이프라인과 샘플러, 축소 uniform.
pub struct BloomPipelines {
    down: wgpu::RenderPipeline,
    down_layout: wgpu::BindGroupLayout,
    up: wgpu::RenderPipeline,
    up_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// 첫 단계(threshold 적용)와 나머지 단계의 uniform.
    first: wgpu::Buffer,
    rest: wgpu::Buffer,
    radius: u32,
}

impl BloomPipelines {
    pub fn new(
        gpu: &Gpu,
        d: &wgpu::Device,
        down_module: &wgpu::ShaderModule,
        up_module: &wgpu::ShaderModule,
        threshold: f64,
        radius: u32,
    ) -> Self {
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
        let down_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waywall bloom down"),
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
                texture_entry(1, false),
            ],
        });
        let up_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waywall bloom up"),
            entries: &[
                texture_entry(0, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline = |layout: &wgpu::BindGroupLayout, module, blend| {
            let pl = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("waywall bloom"),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("waywall bloom"),
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
                        format: LEVEL_FORMAT,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::REPLACE,
        };
        let down = pipeline(&down_layout, down_module, None);
        let up = pipeline(&up_layout, up_module, Some(additive));
        let uniform = |first: bool| {
            let buffer = d.create_buffer(&wgpu::BufferDescriptor {
                label: Some("waywall bloom down"),
                size: (DOWN_WORDS * size_of::<u32>()) as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let words = [(threshold as f32).to_bits(), u32::from(first), 0, 0];
            let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_ne_bytes()).collect();
            gpu.queue.write_buffer(&buffer, 0, &bytes);
            buffer
        };
        let sampler = d.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("waywall bloom"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            down,
            down_layout,
            up,
            up_layout,
            sampler,
            first: uniform(true),
            rest: uniform(false),
            radius,
        }
    }

    /// 렌더 크기에 맞춘 단계들. `hdr`는 첫 축소의 원본.
    pub fn chain(&self, d: &wgpu::Device, hdr: &wgpu::TextureView, size: (u32, u32)) -> BloomChain {
        let sizes = level_sizes(size, self.radius);
        let levels: Vec<Target> = sizes
            .iter()
            .map(|&s| Target::new(d, "waywall bloom", LEVEL_FORMAT, s))
            .collect();
        let down = levels
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let (src, uniforms) = if i == 0 {
                    (hdr, &self.first)
                } else {
                    (&levels[i - 1].view, &self.rest)
                };
                d.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("waywall bloom down"),
                    layout: &self.down_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: uniforms.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(src),
                        },
                    ],
                })
            })
            .collect();
        let up = levels
            .iter()
            .skip(1)
            .map(|src| {
                d.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("waywall bloom up"),
                    layout: &self.up_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&src.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                })
            })
            .collect();
        BloomChain { levels, down, up }
    }

    pub fn render(&self, encoder: &mut wgpu::CommandEncoder, chain: &BloomChain) {
        let pass = |encoder: &mut wgpu::CommandEncoder,
                    target: &Target,
                    load,
                    pipeline,
                    group: &wgpu::BindGroup| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("waywall bloom"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
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
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        };
        for (target, group) in chain.levels.iter().zip(&chain.down) {
            let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);
            pass(encoder, target, clear, &self.down, group);
        }
        // up[i]는 levels[i + 1]을 읽어 levels[i]에 더한다. 작은 단계부터.
        for (i, group) in chain.up.iter().enumerate().rev() {
            pass(
                encoder,
                &chain.levels[i],
                wgpu::LoadOp::Load,
                &self.up,
                group,
            );
        }
    }
}

pub struct BloomChain {
    levels: Vec<Target>,
    down: Vec<wgpu::BindGroup>,
    up: Vec<wgpu::BindGroup>,
}

impl BloomChain {
    /// 후처리가 읽는 결과 (가장 큰 단계).
    pub fn result(&self) -> Option<&wgpu::TextureView> {
        self.levels.first().map(|t| &t.view)
    }
}

/// 단계마다 절반 크기. 한 변이 1px이 되면 더 줄이지 않는다.
fn level_sizes(size: (u32, u32), radius: u32) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let (mut w, mut h) = size;
    for _ in 0..radius {
        if w < 2 || h < 2 {
            break;
        }
        w /= 2;
        h /= 2;
        out.push((w, h));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_halve_until_tiny() {
        assert_eq!(
            level_sizes((1920, 1080), 3),
            [(960, 540), (480, 270), (240, 135)]
        );
        assert_eq!(level_sizes((6, 3), 10), [(3, 1)]);
        assert!(level_sizes((1, 100), 4).is_empty());
    }
}
