//! 씬 렌더러: HDR 레이어 패스 + 후처리(노출·톤매핑) 패스 (docs/architecture.md#씬-렌더러).

use anyhow::{Context, Result, ensure};
use waywall_proto::config::{Blend, Fit, HdrFormat, Render};

use super::{ImageLayer, SceneSpec};
use crate::backend::view::FrameInfo;
use crate::gpu::{FULLSCREEN_ENTRY, Gpu};

/// `scene/image.wgsl`, `scene/post.wgsl`의 엔트리 (인터페이스).
const VS_ENTRY: &str = "vs_main";
const FS_ENTRY: &str = "fs_main";
/// image.wgsl의 `Layer` uniform: axis_u, axis_v, origin, opacity, 패딩 (f32 8개).
const LAYER_FLOATS: usize = 8;
/// post.wgsl의 `Post` uniform: exposure, 패딩 (f32 4개).
const POST_FLOATS: usize = 4;
const BLENDS: [Blend; 4] = [Blend::Normal, Blend::Add, Blend::Multiply, Blend::Screen];

pub struct SceneRenderer {
    size: [f64; 2],
    fit: Fit,
    clear: wgpu::Color,
    pointer_smoothing: f64,
    settle_px: f64,
    hdr_format: wgpu::TextureFormat,
    /// `BLENDS` 순서. 쓰는 블렌드만 만든다.
    pipelines: [Option<wgpu::RenderPipeline>; BLENDS.len()],
    layers: Vec<GpuLayer>,
    post_pipeline: wgpu::RenderPipeline,
    post_layout: wgpu::BindGroupLayout,
    post_uniforms: wgpu::Buffer,
    target: HdrTarget,
    max_parallax: f64,
    parallax: Parallax,
}

struct GpuLayer {
    blend: Blend,
    opacity: f64,
    parallax: f64,
    position: [f64; 2],
    /// 화면 크기의 절반 (설계 px), 회전 전.
    half: [f64; 2],
    /// (cos, sin), 시계 방향.
    rotation: [f64; 2],
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// 마지막으로 쓴 uniform. 같으면 다시 쓰지 않는다.
    written: Option<[f32; LAYER_FLOATS]>,
    _texture: wgpu::Texture,
}

struct HdrTarget {
    view: wgpu::TextureView,
    post_bind_group: wgpu::BindGroup,
    size: (u32, u32),
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
        let hdr_format = match render.hdr_format {
            HdrFormat::Rgba16float => wgpu::TextureFormat::Rgba16Float,
            HdrFormat::Rg11b10ufloat => wgpu::TextureFormat::Rg11b10Ufloat,
            HdrFormat::Rgba32float => wgpu::TextureFormat::Rgba32Float,
        };
        let image = gpu.create_module("scene/image.wgsl", spec.image_shader)?;
        let post = gpu.create_module("scene/post.wgsl", spec.post_shader)?;
        let max_dim = gpu.device.limits().max_texture_dimension_2d;
        for l in &spec.layers {
            ensure!(
                l.texture.width <= max_dim && l.texture.height <= max_dim,
                "layer `{}`: texture {}x{} is larger than the GPU limit {max_dim}",
                l.id,
                l.texture.width,
                l.texture.height
            );
        }

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
                    ],
                });
                let post_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("waywall scene post"),
                    entries: &[
                        uniform_entry(0, wgpu::ShaderStages::FRAGMENT),
                        texture_entry(1, false),
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
                    spec.layers.iter().any(|l| l.blend == b).then(|| {
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
                let layers: Vec<GpuLayer> = spec
                    .layers
                    .iter()
                    .map(|l| upload_layer(gpu, d, &layer_layout, &sampler, l))
                    .collect();
                (pipelines, post_pipeline, post_layout, layers)
            })
            .with_context(|| format!("{name}: failed to create the scene pipelines"))?;
        let (pipelines, post_pipeline, post_layout, layers) = built;

        let post_uniforms = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("waywall scene post"),
            size: (POST_FLOATS * size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut post_values = [0f32; POST_FLOATS];
        post_values[0] = spec.exposure as f32;
        gpu.queue
            .write_buffer(&post_uniforms, 0, &f32_bytes(&post_values));
        let target = HdrTarget::new(gpu, hdr_format, &post_layout, &post_uniforms, size)?;
        let [r, g, b, a] = spec.clear.to_linear_premultiplied();
        let max_parallax = layers.iter().map(|l| l.parallax.abs()).fold(0.0, f64::max);
        tracing::debug!(
            layers = layers.len(),
            ?hdr_format,
            max_parallax,
            "scene ready"
        );
        Ok(Self {
            size: spec.size,
            fit: spec.fit,
            clear: wgpu::Color { r, g, b, a },
            pointer_smoothing: spec.pointer_smoothing,
            settle_px: spec.settle_px,
            hdr_format,
            pipelines,
            layers,
            post_pipeline,
            post_layout,
            post_uniforms,
            target,
            max_parallax,
            parallax: Parallax::default(),
        })
    }

    /// 포인터가 움직이면 화면이 바뀌는지.
    pub fn follows_pointer(&self) -> bool {
        self.max_parallax > 0.0
    }

    pub fn resize(&mut self, gpu: &Gpu, size: (u32, u32)) -> Result<()> {
        if size != self.target.size {
            self.target = HdrTarget::new(
                gpu,
                self.hdr_format,
                &self.post_layout,
                &self.post_uniforms,
                size,
            )?;
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
        let moving = if self.follows_pointer() {
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

        for l in &mut self.layers {
            let values = l.uniform(&map, self.parallax.shift);
            if l.written != Some(values) {
                gpu.queue.write_buffer(&l.uniforms, 0, &f32_bytes(&values));
                l.written = Some(values);
            }
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("waywall scene layers"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some((x, y, w, h)) = map.scissor() {
                pass.set_scissor_rect(x, y, w, h);
                for l in &self.layers {
                    let Some(pipeline) = &self.pipelines[blend_index(l.blend)] else {
                        continue;
                    };
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, &l.bind_group, &[]);
                    pass.draw(0..4, 0..1);
                }
            }
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
    fn uniform(&self, map: &FitMap, shift: [f64; 2]) -> [f32; LAYER_FLOATS] {
        quad_uniform(
            self.position,
            self.half,
            self.rotation,
            self.parallax,
            self.opacity,
            map,
            shift,
        )
    }
}

/// image.wgsl `Layer`: clip = origin + u × axis_u + v × axis_v (u, v ∈ [0, 1], 텍스처 왼쪽 위가 (0, 0)).
fn quad_uniform(
    position: [f64; 2],
    half: [f64; 2],
    rotation: [f64; 2],
    parallax: f64,
    opacity: f64,
    map: &FitMap,
    shift: [f64; 2],
) -> [f32; LAYER_FLOATS] {
    let c = [
        position[0] + shift[0] * parallax,
        position[1] + shift[1] * parallax,
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
        0.0,
    ]
}

fn upload_layer(
    gpu: &Gpu,
    d: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    l: &ImageLayer,
) -> GpuLayer {
    let t = &l.texture;
    let extent = wgpu::Extent3d {
        width: t.width,
        height: t.height,
        depth_or_array_layers: 1,
    };
    let texture = d.create_texture(&wgpu::TextureDescriptor {
        label: Some(&l.id),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let uniforms = d.create_buffer(&wgpu::BufferDescriptor {
        label: Some(&l.id),
        size: (LAYER_FLOATS * size_of::<f32>()) as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = d.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(&l.id),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });
    let rad = l.rotation.to_radians();
    GpuLayer {
        blend: l.blend,
        opacity: l.opacity,
        parallax: l.parallax,
        position: l.position,
        half: [
            f64::from(t.width) * l.scale[0] / 2.0,
            f64::from(t.height) * l.scale[1] / 2.0,
        ],
        rotation: [rad.cos(), rad.sin()],
        uniforms,
        bind_group,
        written: None,
        _texture: texture,
    }
}

impl HdrTarget {
    fn new(
        gpu: &Gpu,
        format: wgpu::TextureFormat,
        post_layout: &wgpu::BindGroupLayout,
        post_uniforms: &wgpu::Buffer,
        size: (u32, u32),
    ) -> Result<Self> {
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
            let post_bind_group = d.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("waywall scene post"),
                layout: post_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: post_uniforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                ],
            });
            Self {
                view,
                post_bind_group,
                size,
            }
        })
        .context("failed to create the HDR buffer")
    }
}

/// premultiplied 알파 기준 블렌드 (docs/wallpaper-format.md#색과-알파).
fn blend_state(b: Blend) -> wgpu::BlendState {
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

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_ne_bytes()).collect()
}

/// 설계 좌표 ↔ 렌더 픽셀 ↔ clip 공간.
#[derive(Debug, Clone, Copy)]
struct FitMap {
    scale: [f64; 2],
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

    fn to_render(self, d: [f64; 2]) -> [f64; 2] {
        [
            d[0] * self.scale[0] + self.offset[0],
            d[1] * self.scale[1] + self.offset[1],
        ]
    }

    fn to_design(self, r: [f64; 2]) -> [f64; 2] {
        [
            (r[0] - self.offset[0]) / self.scale[0],
            (r[1] - self.offset[1]) / self.scale[1],
        ]
    }

    fn to_clip(self, d: [f64; 2]) -> [f64; 2] {
        let r = self.to_render(d);
        [
            r[0] / self.target[0] * 2.0 - 1.0,
            1.0 - r[1] / self.target[1] * 2.0,
        ]
    }

    fn vector_to_clip(self, v: [f64; 2]) -> [f64; 2] {
        [
            v[0] * self.scale[0] / self.target[0] * 2.0,
            -v[1] * self.scale[1] / self.target[1] * 2.0,
        ]
    }

    /// 설계 영역 ∩ 렌더 영역 (px). 비어 있으면 `None`.
    fn scissor(self) -> Option<(u32, u32, u32, u32)> {
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
                &map,
                shift,
            )
        };
        let u = quad(0.0, 0.0, [0.0, 0.0]);
        // 왼쪽 위 모서리 (40, 30) → clip (-0.2, 0.4), 가로 20px, 세로 40px.
        let e = [0.4, 0.0, 0.0, -0.8, -0.2, 0.4];
        for (a, b) in u.iter().zip(e) {
            assert!((a - b).abs() < 1e-6, "{u:?}");
        }
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
