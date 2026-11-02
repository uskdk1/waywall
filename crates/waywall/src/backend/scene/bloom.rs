//! 블룸: compute 패스 하나 안에서 HDR 장면 → 밝은 부분만 남기며 절반씩 축소 → 작은 단계부터 키우며 더함
//! (docs/architecture.md#씬-렌더러).
//!
//! 후처리는 가장 큰 단계(렌더 크기의 절반)의 확대 결과를 읽어 `intensity`배로 더한다.

use crate::gpu::Gpu;

/// `bloom/downsample.wgsl`, `bloom/upsample.wgsl`의 엔트리와 작업 그룹 크기 (인터페이스).
const CS_ENTRY: &str = "cs_main";
const WORKGROUP: u32 = 8;
/// downsample.wgsl `Down`: threshold, first, 패딩 (32비트 단어 4개).
const DOWN_WORDS: usize = 4;
/// 단계 텍스처 포맷 (셰이더의 `texture_storage_2d<rgba16float, write>`).
const LEVEL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// 크기와 관계없는 부분: 파이프라인과 샘플러, 축소 uniform.
pub struct BloomPipelines {
    down: wgpu::ComputePipeline,
    down_layout: wgpu::BindGroupLayout,
    up: wgpu::ComputePipeline,
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
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        };
        let texture = |filterable| wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
        let storage = wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: LEVEL_FORMAT,
            view_dimension: wgpu::TextureViewDimension::D2,
        };
        let down_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waywall bloom down"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(1, texture(false)),
                entry(2, storage),
            ],
        });
        let up_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waywall bloom up"),
            entries: &[
                entry(0, texture(false)),
                entry(1, texture(true)),
                entry(
                    2,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
                entry(3, storage),
            ],
        });
        let pipeline = |layout: &wgpu::BindGroupLayout, module| {
            let pl = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("waywall bloom"),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("waywall bloom"),
                layout: Some(&pl),
                module,
                entry_point: Some(CS_ENTRY),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let down = pipeline(&down_layout, down_module);
        let up = pipeline(&up_layout, up_module);
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
        let down: Vec<Level> = sizes.iter().map(|&s| Level::new(d, s)).collect();
        // up[i]는 down[i]와 같은 크기. 가장 작은 단계는 확대할 것이 없어 down을 그대로 쓴다.
        let up: Vec<Level> = sizes
            .iter()
            .take(sizes.len().saturating_sub(1))
            .map(|&s| Level::new(d, s))
            .collect();
        let mut passes = Vec::with_capacity(down.len() + up.len());
        for (i, dst) in down.iter().enumerate() {
            let (src, uniforms) = if i == 0 {
                (hdr, &self.first)
            } else {
                (&down[i - 1].view, &self.rest)
            };
            let group = d.create_bind_group(&wgpu::BindGroupDescriptor {
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
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&dst.view),
                    },
                ],
            });
            passes.push((false, group, dst.size));
        }
        for (i, dst) in up.iter().enumerate().rev() {
            let small = up.get(i + 1).unwrap_or(&down[i + 1]);
            let group = d.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("waywall bloom up"),
                layout: &self.up_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&down[i].view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&small.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&dst.view),
                    },
                ],
            });
            passes.push((true, group, dst.size));
        }
        BloomChain { down, up, passes }
    }

    /// 축소 → 확대를 compute 패스 하나에 기록한다.
    pub fn render(&self, encoder: &mut wgpu::CommandEncoder, chain: &BloomChain) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("waywall bloom"),
            timestamp_writes: None,
        });
        for (up, group, size) in &chain.passes {
            pass.set_pipeline(if *up { &self.up } else { &self.down });
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(size.0.div_ceil(WORKGROUP), size.1.div_ceil(WORKGROUP), 1);
        }
    }
}

struct Level {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
}

impl Level {
    fn new(d: &wgpu::Device, size: (u32, u32)) -> Self {
        let texture = d.create_texture(&wgpu::TextureDescriptor {
            label: Some("waywall bloom"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: LEVEL_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            _texture: texture,
            view,
            size,
        }
    }
}

pub struct BloomChain {
    down: Vec<Level>,
    up: Vec<Level>,
    /// 디스패치 순서대로 (확대인지, 바인드 그룹, 출력 크기).
    passes: Vec<(bool, wgpu::BindGroup, (u32, u32))>,
}

impl BloomChain {
    /// 후처리가 읽는 결과: 가장 큰 단계의 확대 결과 (단계가 하나면 축소 결과).
    pub fn result(&self) -> Option<&wgpu::TextureView> {
        self.up.first().or(self.down.first()).map(|l| &l.view)
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
