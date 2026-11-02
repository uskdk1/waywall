//! 씬 particles 레이어의 GPU 쪽: 고리 버퍼 방출 + compute 시뮬레이션 + 인스턴스 사각형
//! (docs/architecture.md#씬-렌더러).
//!
//! CPU는 프레임마다 "고리 버퍼의 어느 자리에서 몇 개가 태어나는지"만 정한다. 파티클 상태는 GPU에만 있다.

use anyhow::{Result, ensure};
use waywall_proto::config::{EmitterShape, PointerMode};

use super::ParticleLayer;
use super::render::{FitMap, TEXTURE_FORMAT, blend_state, upload_texture};
use crate::backend::view::FrameInfo;
use crate::gpu::Gpu;

/// `particles/simulate.wgsl`, `particles/render.wgsl`의 엔트리 (인터페이스).
const CS_ENTRY: &str = "cs_main";
const VS_ENTRY: &str = "vs_main";
const FS_TEXTURE: &str = "fs_texture";
const FS_SPRITE: &str = "fs_sprite";
/// simulate.wgsl의 `@workgroup_size`.
const WORKGROUP: u32 = 64;
/// `Particle`: pos, vel, age, life (f32 6개).
const PARTICLE_BYTES: u64 = 24;
/// simulate.wgsl `Sim`: 32비트 단어 36개 (필드 순서는 셰이더 주석).
const SIM_WORDS: usize = 36;
/// render.wgsl `Draw`: to_clip, size + 패딩, color_start, color_end (f32 16개).
const DRAW_FLOATS: usize = 16;
/// 텍스처 없이 스프라이트로 그리는 레이어에 묶는 1×1 텍스처 (바인딩을 채우는 용도, 읽지 않음).
const PLACEHOLDER_TEXEL: [u8; 4] = [u8::MAX; 4];

pub struct GpuParticles {
    spec: Box<ParticleLayer>,
    sim_pipeline: wgpu::ComputePipeline,
    sim_bind_group: wgpu::BindGroup,
    sim_uniforms: wgpu::Buffer,
    draw_pipeline: wgpu::RenderPipeline,
    draw_bind_group: wgpu::BindGroup,
    draw_uniforms: wgpu::Buffer,
    emission: Emission,
    /// 이번 프레임에 시뮬레이션하고 그릴 것이 있다.
    active: bool,
    /// 마지막으로 본 `Pointer::presses`.
    presses_seen: u32,
    _particles: wgpu::Buffer,
    _texture: wgpu::Texture,
}

impl GpuParticles {
    /// `d`는 `Gpu::scoped` 안의 장치. 한계 검사는 그 전에 `check_limits`로.
    pub fn new(
        gpu: &Gpu,
        d: &wgpu::Device,
        mut spec: Box<ParticleLayer>,
        simulate: &wgpu::ShaderModule,
        hdr_format: wgpu::TextureFormat,
        sampler: &wgpu::Sampler,
    ) -> Result<Self> {
        let shader = std::mem::take(&mut spec.shader);
        let render = gpu.create_module(&spec.id, shader)?;
        let id = spec.id.as_str();
        let uniform = |size: usize| {
            d.create_buffer(&wgpu::BufferDescriptor {
                label: Some(id),
                size: (size * size_of::<u32>()) as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let sim_uniforms = uniform(SIM_WORDS);
        let draw_uniforms = uniform(DRAW_FLOATS);
        // wgpu가 0으로 초기화한다: age = life = 0, 즉 모두 죽은 상태.
        let particles = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some(id),
            size: u64::from(spec.max) * PARTICLE_BYTES,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let textured = spec.texture.is_some();
        let texture = match spec.texture.take() {
            Some(t) => upload_texture(gpu, d, id, &t, TEXTURE_FORMAT),
            None => upload_texture(
                gpu,
                d,
                id,
                &super::Texture {
                    width: 1,
                    height: 1,
                    data: PLACEHOLDER_TEXEL.to_vec(),
                },
                TEXTURE_FORMAT,
            ),
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty,
            count: None,
        };
        let buffer = |ty| wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let sim_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(id),
            entries: &[
                entry(
                    0,
                    wgpu::ShaderStages::COMPUTE,
                    buffer(wgpu::BufferBindingType::Uniform),
                ),
                entry(
                    1,
                    wgpu::ShaderStages::COMPUTE,
                    buffer(wgpu::BufferBindingType::Storage { read_only: false }),
                ),
            ],
        });
        let draw_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(id),
            entries: &[
                entry(
                    0,
                    wgpu::ShaderStages::VERTEX,
                    buffer(wgpu::BufferBindingType::Uniform),
                ),
                entry(
                    1,
                    wgpu::ShaderStages::VERTEX,
                    buffer(wgpu::BufferBindingType::Storage { read_only: true }),
                ),
                entry(
                    2,
                    wgpu::ShaderStages::FRAGMENT,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(
                    3,
                    wgpu::ShaderStages::FRAGMENT,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        });
        let sim_pipeline = d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(id),
            layout: Some(&d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(id),
                bind_group_layouts: &[Some(&sim_layout)],
                immediate_size: 0,
            })),
            module: simulate,
            entry_point: Some(CS_ENTRY),
            compilation_options: Default::default(),
            cache: None,
        });
        let draw_pipeline = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(id),
            layout: Some(&d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(id),
                bind_group_layouts: &[Some(&draw_layout)],
                immediate_size: 0,
            })),
            vertex: wgpu::VertexState {
                module: &render,
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
                module: &render,
                entry_point: Some(if textured { FS_TEXTURE } else { FS_SPRITE }),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: hdr_format,
                    blend: Some(blend_state(spec.blend)),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sim_bind_group = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(id),
            layout: &sim_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: sim_uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: particles.as_entire_binding(),
                },
            ],
        });
        let draw_bind_group = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(id),
            layout: &draw_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: draw_uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: particles.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        Ok(Self {
            spec,
            sim_pipeline,
            sim_bind_group,
            sim_uniforms,
            draw_pipeline,
            draw_bind_group,
            draw_uniforms,
            emission: Emission::default(),
            active: false,
            presses_seen: 0,
            _particles: particles,
            _texture: texture,
        })
    }

    pub fn parallax(&self) -> f64 {
        self.spec.parallax
    }

    pub fn follows_pointer(&self) -> bool {
        self.spec.pointer.mode != PointerMode::None
    }

    pub fn wants_clicks(&self) -> bool {
        self.spec.burst > 0
    }

    /// 이번 프레임의 방출을 정하고 uniform을 쓴다. 다음 프레임도 필요하면(살아 있는 파티클, `rate > 0`) `true`.
    pub fn update(&mut self, gpu: &Gpu, map: FitMap, shift: [f64; 2], f: &FrameInfo<'_>) -> bool {
        let s = &self.spec;
        let p = f.pointer;
        let presses = p.presses.wrapping_sub(self.presses_seen);
        self.presses_seen = p.presses;
        let was_alive = self.emission.alive(s.rate);
        let dt = f64::from(f.time_delta).min(s.max_step);
        let spawn = self.emission.step(
            dt,
            s.rate,
            s.burst.saturating_mul(presses),
            s.max,
            s.lifetime[1],
        );
        self.active = was_alive || spawn.emit + spawn.burst > 0;
        if !self.active {
            return false;
        }

        // 렌더 px(왼쪽 아래 원점) → 이 레이어의 시뮬레이션 좌표(패럴랙스 이동 전 설계 px).
        let offset = [shift[0] * s.parallax, shift[1] * s.parallax];
        let height = f64::from(f.size.1);
        let to_layer = |x: f32, y: f32| {
            let d = map.to_design([f64::from(x), height - f64::from(y)]);
            [d[0] - offset[0], d[1] - offset[1]]
        };
        let pointer = to_layer(p.pos[0], p.pos[1]);
        // iMouse.zw의 절댓값 = 마지막 왼쪽 클릭 위치.
        let burst_pos = to_layer(p.mouse[2].abs(), p.mouse[3].abs());
        let pointer_mode = match s.pointer.mode {
            _ if !p.inside => 0,
            PointerMode::None => 0,
            PointerMode::Attract => 1,
            PointerMode::Repel => 2,
        };
        let shape = match s.shape {
            EmitterShape::Point => 0,
            EmitterShape::Box => 1,
            EmitterShape::Circle => 2,
        };
        let v = &s.velocity;
        let t = &s.turbulence;
        let ints = [
            spawn.start,
            spawn.emit,
            spawn.burst,
            s.max,
            shape,
            pointer_mode,
            self.emission.seed,
            0,
        ];
        let floats = [
            s.center[0],
            s.center[1],
            s.extent[0],
            s.extent[1],
            burst_pos[0],
            burst_pos[1],
            dt,
            self.emission.time,
            v.min[0],
            v.min[1],
            v.max[0],
            v.max[1],
            s.gravity[0],
            s.gravity[1],
            s.drag,
            0.0,
            s.lifetime[0],
            s.lifetime[1],
            t.strength,
            t.scale,
            pointer[0],
            pointer[1],
            s.pointer.radius,
            s.pointer.strength,
            t.speed,
            0.0,
            0.0,
            0.0,
        ];
        debug_assert_eq!(ints.len() + floats.len(), SIM_WORDS);
        // 프레임마다 쓰므로 힙 할당 없이 고정 크기 배열에 담는다.
        let mut bytes = [0u8; SIM_WORDS * 4];
        let words = ints.into_iter().chain(floats.map(|x| (x as f32).to_bits()));
        for (dst, w) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {
            *dst = w.to_ne_bytes();
        }
        gpu.queue.write_buffer(&self.sim_uniforms, 0, &bytes);

        let scale = map.vector_to_clip([1.0, 1.0]);
        let origin = map.to_clip(offset);
        let color = |i: usize| {
            let [r, g, b, a] = s.color[i].to_linear_premultiplied();
            let k = s.emissive * s.opacity;
            [r * k, g * k, b * k, a * s.opacity]
        };
        let [c0, c1] = [color(0), color(1)];
        let draw: [f64; DRAW_FLOATS] = [
            scale[0], scale[1], origin[0], origin[1], s.size[0], s.size[1], 0.0, 0.0, c0[0], c0[1],
            c0[2], c0[3], c1[0], c1[1], c1[2], c1[3],
        ];
        let mut bytes = [0u8; DRAW_FLOATS * 4];
        for (dst, x) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(draw) {
            *dst = (x as f32).to_ne_bytes();
        }
        gpu.queue.write_buffer(&self.draw_uniforms, 0, &bytes);

        self.emission.alive(s.rate)
    }

    pub fn simulate(&self, pass: &mut wgpu::ComputePass<'_>) {
        if !self.active {
            return;
        }
        pass.set_pipeline(&self.sim_pipeline);
        pass.set_bind_group(0, &self.sim_bind_group, &[]);
        pass.dispatch_workgroups(self.spec.max.div_ceil(WORKGROUP), 1, 1);
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if !self.active {
            return;
        }
        pass.set_pipeline(&self.draw_pipeline);
        pass.set_bind_group(0, &self.draw_bind_group, &[]);
        pass.draw(0..4, 0..self.spec.max);
    }
}

/// GPU 한계 안에 들어가는지 (`max` → 버퍼 크기, 디스패치 수).
pub fn check_limits(gpu: &Gpu, l: &ParticleLayer) -> Result<()> {
    let limits = gpu.device.limits();
    let bytes = u64::from(l.max) * PARTICLE_BYTES;
    let max_bytes = limits
        .max_storage_buffer_binding_size
        .min(limits.max_buffer_size);
    ensure!(
        bytes <= max_bytes,
        "layer `{}`: max = {} needs {bytes} bytes, more than the GPU limit {max_bytes}",
        l.id,
        l.max
    );
    let groups = l.max.div_ceil(WORKGROUP);
    ensure!(
        groups <= limits.max_compute_workgroups_per_dimension,
        "layer `{}`: max = {} is more than this GPU can simulate ({})",
        l.id,
        l.max,
        limits.max_compute_workgroups_per_dimension * WORKGROUP
    );
    Ok(())
}

/// 이번 프레임에 태어나는 고리 버퍼 범위: `[start, start + emit + burst)` (mod max).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Spawn {
    start: u32,
    emit: u32,
    burst: u32,
}

/// 방출 수 계산과 "모두 죽었는지" 판단. GPU 상태를 읽지 않고 CPU 쪽 기록만으로 안다.
#[derive(Debug, Clone, Copy, Default)]
struct Emission {
    cursor: u32,
    /// `rate × dt`의 이월된 소수부.
    carry: f64,
    /// 시뮬레이션 시계(초, dt 합).
    time: f64,
    /// 이 시각이 지나면 모든 파티클이 죽어 있다.
    alive_until: f64,
    seed: u32,
}

impl Emission {
    fn step(&mut self, dt: f64, rate: f64, burst: u32, max: u32, life_max: f64) -> Spawn {
        self.time += dt;
        let want = self.carry + rate * dt;
        let whole = want.floor();
        self.carry = want - whole;
        let burst = burst.min(max);
        let emit = whole.min(f64::from(max - burst)) as u32;
        let start = self.cursor;
        let n = emit + burst;
        self.cursor = ((u64::from(start) + u64::from(n)) % u64::from(max)) as u32;
        if n > 0 {
            self.alive_until = self.time + life_max;
        }
        self.seed = self.seed.wrapping_add(1);
        Spawn { start, emit, burst }
    }

    fn alive(&self, rate: f64) -> bool {
        rate > 0.0 || self.time < self.alive_until
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emission_carries_fractions_and_wraps() {
        let mut e = Emission::default();
        // 초당 30개, 60fps → 프레임마다 0.5개: 두 프레임에 한 개.
        let counts: Vec<u32> = (0..4)
            .map(|_| e.step(1.0 / 60.0, 30.0, 0, 10, 1.0).emit)
            .collect();
        assert_eq!(counts.iter().sum::<u32>(), 2, "{counts:?}");
        let mut e = Emission {
            cursor: 8,
            ..Default::default()
        };
        let s = e.step(0.1, 50.0, 0, 10, 1.0);
        assert_eq!(
            s,
            Spawn {
                start: 8,
                emit: 5,
                burst: 0
            }
        );
        assert_eq!(e.cursor, 3, "ring wraps");
    }

    #[test]
    fn emission_caps_at_max_bursts_first() {
        let mut e = Emission::default();
        let s = e.step(1.0, 100.0, 7, 10, 1.0);
        assert_eq!((s.emit, s.burst), (3, 7));
        let s = e.step(0.0, 0.0, 50, 10, 1.0);
        assert_eq!((s.emit, s.burst), (0, 10));
        assert_eq!(e.cursor, 0);
    }

    #[test]
    fn burst_only_layer_sleeps_after_lifetime() {
        let mut e = Emission::default();
        assert!(!e.alive(0.0), "nothing spawned yet");
        e.step(0.016, 0.0, 20, 100, 2.0);
        assert!(e.alive(0.0));
        let mut frames = 0;
        while e.alive(0.0) {
            e.step(0.1, 0.0, 0, 100, 2.0);
            frames += 1;
        }
        assert!(
            (19..=21).contains(&frames),
            "max lifetime 2 s at 0.1 s steps ({frames})"
        );
        assert!(Emission::default().alive(5.0), "rate > 0 never sleeps");
    }
}
