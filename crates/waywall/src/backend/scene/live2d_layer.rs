//! live2d 레이어의 GPU 쪽 (docs/architecture.md#씬-렌더러).
//!
//! 모델을 `rect` 렌더 크기의 자기 텍스처에 그린 뒤(투명 바탕, 선형 premultiplied) image 파이프라인 사각형으로 합성한다.
//! 클리핑 마스크는 같은 마스크 묶음마다 레이어 해상도 그대로의 R8 텍스처에 마스크 drawable을 그려 두고,
//! 그 마스크를 쓰는 drawable이 같은 픽셀 값을 곱한다.

use anyhow::{Result, ensure};
use waywall_proto::color::srgb_to_linear;

use super::render::{
    FitMap, FollowState, GpuLayer, LayerBindings, LayerPlacement, TEXTURE_FORMAT, upload_texture,
};
use super::shader_layer::{OUTPUT_FORMAT, Target};
use super::{Axis, Hit, Live2dLayer, Texture};
use crate::gpu::Gpu;
use crate::live2d::{self, Animator, AnimatorSpec, Model, Physics, Pose};

/// `live2d/*.wgsl` 엔트리 (인터페이스).
const VS_ENTRY: &str = "vs_main";
const FS_ENTRY: &str = "fs_main";
const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
/// draw.wgsl `Layer`: scale, offset (f32 4개).
const LAYER_FLOATS: usize = 4;
/// draw.wgsl `Drawable`: params, multiply, screen (f32 12개).
const DRAWABLE_FLOATS: usize = 12;
/// 마스크가 없는 drawable에 묶는 1×1 (읽지 않음).
const NO_MASK_TEXEL: [u8; 4] = [0; 4];

/// drawable 블렌드 (Cubism 상수 플래그). 순서 = `main` 파이프라인 순서.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrawBlend {
    Normal,
    Additive,
    Multiplicative,
}

const DRAW_BLENDS: [DrawBlend; 3] = [
    DrawBlend::Normal,
    DrawBlend::Additive,
    DrawBlend::Multiplicative,
];

impl DrawBlend {
    fn from_flags(flags: u8) -> Self {
        if flags & live2d::BLEND_ADDITIVE != 0 {
            DrawBlend::Additive
        } else if flags & live2d::BLEND_MULTIPLICATIVE != 0 {
            DrawBlend::Multiplicative
        } else {
            DrawBlend::Normal
        }
    }

    /// premultiplied 기준. 가산·곱하기는 알파(덮은 정도)를 바꾸지 않는다.
    fn state(self) -> wgpu::BlendState {
        use wgpu::BlendFactor as F;
        let keep_alpha = wgpu::BlendComponent {
            src_factor: F::Zero,
            dst_factor: F::One,
            operation: wgpu::BlendOperation::Add,
        };
        let color = |src, dst| wgpu::BlendComponent {
            src_factor: src,
            dst_factor: dst,
            operation: wgpu::BlendOperation::Add,
        };
        match self {
            DrawBlend::Normal => wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
            DrawBlend::Additive => wgpu::BlendState {
                color: color(F::One, F::One),
                alpha: keep_alpha,
            },
            DrawBlend::Multiplicative => wgpu::BlendState {
                color: color(F::Dst, F::OneMinusSrcAlpha),
                alpha: keep_alpha,
            },
        }
    }
}

/// 시선: 추적 상태와 (파라미터 번호, 최대값, 성분).
struct Look {
    follow: FollowState,
    params: Vec<(usize, f32, Axis)>,
}

/// 클릭 규칙 (drawable 번호로 풀어 둔 것).
struct Click {
    /// `None`이면 보이는 메시 어디든.
    drawables: Option<Vec<usize>>,
    clips: Vec<usize>,
}

/// 같은 마스크 drawable 묶음 하나.
struct MaskSet {
    masks: Vec<i32>,
    target: Option<(Target, wgpu::BindGroup)>,
}

/// 정점 버퍼 안에서 drawable 하나의 자리.
#[derive(Debug, Clone, Copy)]
struct Range {
    base_vertex: i32,
    first_index: u32,
    index_count: u32,
}

pub struct GpuLive2dLayer {
    pub quad: GpuLayer,
    id: String,
    rect: [f64; 4],
    render_scale: f64,
    model: Model,
    animator: Animator,
    /// 애니메이터가 쓴 값 (Core 배열로 옮기기 전).
    params: Vec<f32>,
    parts: Vec<f32>,
    look: Option<Look>,
    look_add: Vec<(usize, f32)>,
    clicks: Vec<Click>,
    /// 마지막으로 본 `Pointer::presses` (처음 프레임 전에는 `None`).
    presses_seen: Option<u32>,
    /// 클릭 위치(설계 px) → 모델 단위 변환에 쓰는 배치.
    position: [f64; 2],
    scale: f64,
    main: [wgpu::RenderPipeline; DRAW_BLENDS.len()],
    mask: wgpu::RenderPipeline,
    mask_layout: wgpu::BindGroupLayout,
    layer_group: wgpu::BindGroup,
    texture_groups: Vec<wgpu::BindGroup>,
    no_mask_group: wgpu::BindGroup,
    drawable_group: wgpu::BindGroup,
    drawable_uniforms: wgpu::Buffer,
    /// 동적 오프셋 간격 (장치의 uniform 정렬).
    drawable_stride: u64,
    positions: wgpu::Buffer,
    _uvs: wgpu::Buffer,
    indices: wgpu::Buffer,
    ranges: Vec<Range>,
    mask_sets: Vec<MaskSet>,
    /// drawable마다 `mask_sets`의 번호.
    mask_of: Vec<Option<usize>>,
    output: Option<Target>,
    /// 그리는 크기 (렌더 px).
    size: (u32, u32),
    /// 다시 그려야 함 (크기 변경, 모델 변화).
    dirty: bool,
    /// 프레임마다 다시 쓰는 버퍼 (할당을 재사용).
    scratch_positions: Vec<f32>,
    scratch_uniforms: Vec<u8>,
    scratch_order: Vec<usize>,
    _textures: Vec<wgpu::Texture>,
    _no_mask: wgpu::Texture,
}

fn f32_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_ne_bytes()).collect()
}

/// f32 슬라이스의 바이트 (복사 없이).
fn bytemuck_f32(v: &[f32]) -> &[u8] {
    // SAFETY: f32는 패딩이 없고 u8 정렬은 1이라 같은 메모리를 바이트로 읽어도 된다.
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast(), std::mem::size_of_val(v)) }
}

/// `write_buffer`는 4바이트 단위.
fn padded(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.resize(bytes.len().next_multiple_of(4).max(4), 0);
    bytes
}

/// 모델 단위 → 레이어 텍스처 clip 좌표: clip = position × scale + offset.
/// 캔버스 px = origin + (x, −y) × ppu, rect 안 설계 px = position + (캔버스 px − 캔버스 크기 / 2) × scale.
fn layer_transform(
    c: live2d::Canvas,
    rect: [f64; 4],
    position: [f64; 2],
    scale: f64,
) -> [f32; LAYER_FLOATS] {
    let ppu = f64::from(c.pixels_per_unit);
    let (w, h) = (rect[2], rect[3]);
    let local =
        |i: usize| position[i] + (f64::from(c.origin[i]) - f64::from(c.size[i]) / 2.0) * scale;
    [
        (ppu * scale * 2.0 / w) as f32,
        (ppu * scale * 2.0 / h) as f32,
        (local(0) * 2.0 / w - 1.0) as f32,
        (1.0 - local(1) * 2.0 / h) as f32,
    ]
}

impl GpuLive2dLayer {
    pub fn new(gpu: &Gpu, d: &wgpu::Device, spec: Live2dLayer) -> Result<Self> {
        let Live2dLayer {
            id,
            blend,
            opacity,
            parallax,
            rect,
            position,
            scale,
            render_scale,
            clip,
            moc,
            textures,
            clips,
            idle,
            eye_ids,
            blink,
            breath,
            physics,
            pose,
            max_step,
            look_at,
            on_click,
            draw_shader,
            mask_shader,
        } = spec;
        let max_dim = d.limits().max_texture_dimension_2d;
        for t in &textures {
            ensure!(
                t.width <= max_dim && t.height <= max_dim,
                "layer `{id}`: texture {}x{} is larger than the GPU limit {max_dim}",
                t.width,
                t.height
            );
        }
        let model = Model::new(&moc)?;
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        let physics = physics.map(|(f, g, w)| Physics::new(&f, &model.parameter_ids, g, w));
        let pose = pose.map(|(f, fade)| Pose::new(&f, &model.part_ids, fade));
        let animator = Animator::new(AnimatorSpec {
            parameter_ids: &model.parameter_ids,
            parameter_defaults: &model.parameter_defaults,
            parameter_ranges: &model.parameter_ranges,
            part_ids: &model.part_ids,
            part_defaults: &model.part_defaults,
            idle,
            clips,
            eye_ids: &eye_ids,
            blink,
            breath,
            physics,
            pose,
            max_step: max_step as f32,
            seed,
        });
        let params = model.parameter_defaults.clone();
        let parts = model.part_defaults.clone();
        let param_index = |id: &str| model.parameter_ids.iter().position(|p| p == id);
        let look = look_at.map(|l| {
            let params = l
                .params
                .iter()
                .filter_map(|(id, max, axis)| Some((param_index(id)?, *max, *axis)))
                .collect();
            Look {
                follow: FollowState::new(l.follow),
                params,
            }
        });
        let drawable_index = |id: &str| model.drawable_ids.iter().position(|d| d == id);
        let clicks = on_click
            .into_iter()
            .map(|r| Click {
                drawables: match r.hit {
                    Hit::Drawables(ids) => {
                        Some(ids.iter().filter_map(|id| drawable_index(id)).collect())
                    }
                    Hit::Anywhere => None,
                },
                clips: r.clips,
            })
            .collect();
        let draw = gpu.create_module("live2d/draw.wgsl", draw_shader)?;
        let mask_module = gpu.create_module("live2d/mask.wgsl", mask_shader)?;

        let uniform_entry = |binding, has_dynamic_offset| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset,
                min_binding_size: None,
            },
            count: None,
        };
        let texture_entry = |filterable| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layer_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waywall live2d layer"),
            entries: &[
                uniform_entry(0, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let texture_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waywall live2d texture"),
            entries: &[texture_entry(true)],
        });
        let mask_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waywall live2d mask"),
            entries: &[texture_entry(false)],
        });
        let drawable_layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waywall live2d drawable"),
            entries: &[uniform_entry(0, true)],
        });

        let vertex_buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: (2 * size_of::<f32>()) as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x2],
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: (2 * size_of::<f32>()) as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![1 => Float32x2],
            }),
        ];
        let pipeline = |label: &str,
                        layout: &wgpu::PipelineLayout,
                        module: &wgpu::ShaderModule,
                        format: wgpu::TextureFormat,
                        blend: wgpu::BlendState| {
            d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(VS_ENTRY),
                    compilation_options: Default::default(),
                    buffers: &vertex_buffers,
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(FS_ENTRY),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let main_layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("waywall live2d"),
            bind_group_layouts: &[
                Some(&layer_layout),
                Some(&texture_layout),
                Some(&mask_layout),
                Some(&drawable_layout),
            ],
            immediate_size: 0,
        });
        let mask_pl = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("waywall live2d mask"),
            bind_group_layouts: &[Some(&layer_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let main =
            DRAW_BLENDS.map(|b| pipeline(&id, &main_layout, &draw, OUTPUT_FORMAT, b.state()));
        // 마스크 합치기: 1 − Π(1 − a).
        let union = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrc,
            operation: wgpu::BlendOperation::Add,
        };
        let mask = pipeline(
            &id,
            &mask_pl,
            &mask_module,
            MASK_FORMAT,
            wgpu::BlendState {
                color: union,
                alpha: union,
            },
        );

        let layer_uniforms = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&id),
            size: (LAYER_FLOATS * size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue.write_buffer(
            &layer_uniforms,
            0,
            &f32_bytes(&layer_transform(model.canvas, rect, position, scale)),
        );
        let sampler = d.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("waywall live2d"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let layer_group = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&id),
            layout: &layer_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: layer_uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let texture_bind = |layout: &wgpu::BindGroupLayout, view: &wgpu::TextureView| {
            d.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&id),
                layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                }],
            })
        };
        let gpu_textures: Vec<wgpu::Texture> = textures
            .iter()
            .map(|t| upload_texture(gpu, d, &id, t, TEXTURE_FORMAT))
            .collect();
        let texture_groups = gpu_textures
            .iter()
            .map(|t| texture_bind(&texture_layout, &t.create_view(&Default::default())))
            .collect();
        let no_mask = upload_texture(
            gpu,
            d,
            "waywall live2d no mask",
            &Texture {
                width: 1,
                height: 1,
                data: NO_MASK_TEXEL.to_vec(),
            },
            MASK_FORMAT,
        );
        let no_mask_group = texture_bind(&mask_layout, &no_mask.create_view(&Default::default()));

        // 정점·인덱스: UV와 인덱스는 바뀌지 않고, 위치만 프레임마다 쓴다.
        let drawables = model.drawables();
        let mut ranges = Vec::with_capacity(drawables.len());
        let (mut uvs, mut indices) = (Vec::new(), Vec::new());
        let mut vertex_count = 0usize;
        for dr in &drawables {
            ranges.push(Range {
                base_vertex: i32::try_from(vertex_count)?,
                first_index: u32::try_from(indices.len())?,
                index_count: u32::try_from(dr.indices.len())?,
            });
            uvs.extend(dr.uvs.iter().flat_map(|v| [v.x, v.y]));
            indices.extend_from_slice(dr.indices);
            vertex_count += dr.positions.len();
        }
        let buffer = |label: &str, size: u64, usage| {
            d.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size.max(4).next_multiple_of(4),
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let vertex_bytes = (vertex_count * 2 * size_of::<f32>()) as u64;
        let positions = buffer(
            "waywall live2d positions",
            vertex_bytes,
            wgpu::BufferUsages::VERTEX,
        );
        let uv_buffer = buffer(
            "waywall live2d uvs",
            vertex_bytes,
            wgpu::BufferUsages::VERTEX,
        );
        gpu.queue
            .write_buffer(&uv_buffer, 0, &padded(f32_bytes(&uvs)));
        let index_bytes: Vec<u8> = indices.iter().flat_map(|i| i.to_ne_bytes()).collect();
        let index_buffer = buffer(
            "waywall live2d indices",
            index_bytes.len() as u64,
            wgpu::BufferUsages::INDEX,
        );
        gpu.queue
            .write_buffer(&index_buffer, 0, &padded(index_bytes));

        let align = u64::from(d.limits().min_uniform_buffer_offset_alignment);
        let drawable_stride = ((DRAWABLE_FLOATS * size_of::<f32>()) as u64).next_multiple_of(align);
        let drawable_uniforms = buffer(
            "waywall live2d drawables",
            drawable_stride * drawables.len().max(1) as u64,
            wgpu::BufferUsages::UNIFORM,
        );
        let drawable_group = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&id),
            layout: &drawable_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &drawable_uniforms,
                    offset: 0,
                    size: wgpu::BufferSize::new((DRAWABLE_FLOATS * size_of::<f32>()) as u64),
                }),
            }],
        });

        // 마스크 묶음: 마스크 drawable 목록이 같은 drawable끼리 하나를 쓴다.
        let mut mask_sets: Vec<MaskSet> = Vec::new();
        let mask_of = drawables
            .iter()
            .map(|dr| {
                (!dr.masks.is_empty()).then(|| {
                    mask_sets
                        .iter()
                        .position(|m| m.masks == dr.masks)
                        .unwrap_or_else(|| {
                            mask_sets.push(MaskSet {
                                masks: dr.masks.to_vec(),
                                target: None,
                            });
                            mask_sets.len() - 1
                        })
                })
            })
            .collect();
        drop(drawables);

        let mut quad = GpuLayer::new(
            d,
            &id,
            LayerPlacement {
                blend,
                opacity,
                parallax,
                position: [rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0],
                size: [rect[2], rect[3]],
                rotation: 0.0,
                lit: false,
            },
        );
        quad.clip = clip;
        tracing::debug!(
            layer = %id,
            drawables = ranges.len(),
            vertices = vertex_count,
            mask_sets = mask_sets.len(),
            canvas = ?model.canvas,
            "live2d layer ready"
        );
        Ok(Self {
            quad,
            id,
            rect,
            render_scale,
            model,
            animator,
            params,
            parts,
            look,
            look_add: Vec::new(),
            clicks,
            presses_seen: None,
            position,
            scale,
            main,
            mask,
            mask_layout,
            layer_group,
            texture_groups,
            no_mask_group,
            drawable_group,
            drawable_uniforms,
            drawable_stride,
            positions,
            _uvs: uv_buffer,
            indices: index_buffer,
            ranges,
            mask_sets,
            mask_of,
            output: None,
            size: (1, 1),
            dirty: true,
            scratch_positions: Vec::with_capacity(vertex_count * 2),
            scratch_uniforms: Vec::new(),
            scratch_order: Vec::new(),
            _textures: gpu_textures,
            _no_mask: no_mask,
        })
    }

    pub fn parallax(&self) -> f64 {
        self.quad.parallax
    }

    /// 출력 크기에 따르는 자원: 결과 텍스처와 마스크 텍스처.
    pub fn resize(
        &mut self,
        d: &wgpu::Device,
        map: FitMap,
        b: &LayerBindings<'_>,
        clip: Option<&wgpu::TextureView>,
    ) {
        self.size = (
            ((self.rect[2] * map.scale[0] * self.render_scale).round() as u32).max(1),
            ((self.rect[3] * map.scale[1] * self.render_scale).round() as u32).max(1),
        );
        for set in &mut self.mask_sets {
            let target = Target::new(d, &self.id, MASK_FORMAT, self.size);
            let group = d.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&self.id),
                layout: &self.mask_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&target.view),
                }],
            });
            set.target = Some((target, group));
        }
        let output = Target::new(d, &self.id, OUTPUT_FORMAT, self.size);
        self.quad.bind(d, b, &output.view, None, clip);
        self.output = Some(output);
        self.dirty = true;
    }

    pub fn follows_pointer(&self) -> bool {
        self.look.is_some()
    }

    pub fn wants_clicks(&self) -> bool {
        !self.clicks.is_empty()
    }

    /// 설계 px → 모델 단위 (`layer_transform`의 역). `shift`: 이 레이어의 패럴랙스 이동.
    fn to_model(&self, p: [f64; 2], shift: [f64; 2]) -> [f32; 2] {
        let c = self.model.canvas;
        let local = [
            p[0] - self.rect[0] - shift[0],
            p[1] - self.rect[1] - shift[1],
        ];
        let canvas = |i: usize| {
            (local[i] - self.position[i]) / self.scale + f64::from(c.size[i]) / 2.0
                - f64::from(c.origin[i])
        };
        let ppu = f64::from(c.pixels_per_unit);
        [(canvas(0) / ppu) as f32, (-canvas(1) / ppu) as f32]
    }

    /// 클릭한 곳(모델 단위)에 맞는 규칙. 부위가 겹치면 위에 그려진 쪽.
    fn hit(&self, at: [f32; 2]) -> Option<&[usize]> {
        let drawables = self.model.drawables();
        let inside = |i: usize| {
            let dr = &drawables[i];
            dr.indices.as_chunks::<3>().0.iter().any(|t| {
                let v = |k: usize| dr.positions[usize::from(t[k])];
                point_in_triangle(at, v(0), v(1), v(2))
            })
        };
        let mut best: Option<(i32, &Click)> = None;
        for c in &self.clicks {
            let order = match &c.drawables {
                Some(ds) => ds
                    .iter()
                    .filter(|&&i| inside(i))
                    .map(|&i| drawables[i].render_order)
                    .max(),
                None => (0..drawables.len())
                    .filter(|&i| drawables[i].visible && drawables[i].opacity > 0.0 && inside(i))
                    .map(|i| drawables[i].render_order)
                    .max()
                    // "*"는 부위 규칙이 하나도 맞지 않을 때만.
                    .filter(|_| best.is_none()),
            };
            if let Some(o) = order
                && best.is_none_or(|(b, _)| o > b)
            {
                best = Some((o, c));
            }
        }
        best.map(|(_, c)| c.clips.as_slice())
    }

    /// 모델 상태를 `dt`초 진행한다. 다음 프레임도 그려야 하면 `true`.
    /// `pointer`·`click`: 설계 px, `shift`: 장면 패럴랙스, `presses`: 왼쪽 버튼 누른 횟수.
    pub fn update(
        &mut self,
        dt: f32,
        pointer: Option<[f64; 2]>,
        shift: [f64; 2],
        settle: f64,
        presses: u32,
        click: [f64; 2],
    ) -> bool {
        let layer_shift = [shift[0] * self.quad.parallax, shift[1] * self.quad.parallax];
        let new_presses = self.presses_seen.map_or(0, |s| presses.wrapping_sub(s));
        self.presses_seen = Some(presses);
        if new_presses > 0 && !self.clicks.is_empty() {
            let at = self.to_model(click, layer_shift);
            if let Some(clips) = self.hit(at).map(<[usize]>::to_vec) {
                self.animator.play_random(&clips);
            }
        }
        let mut moving = false;
        self.look_add.clear();
        if let Some(Look { follow, params }) = &mut self.look {
            // 정착 허용 오차: 출력 px → 방향 단위 (range px에서 최대).
            let range = follow.spec_range();
            moving |= follow.step(pointer, layer_shift, f64::from(dt), settle / range);
            let [x, y] = follow.shift();
            for &(i, max, axis) in params.iter() {
                // 설계 px는 y가 아래로 커진다. 시선 y는 위가 +.
                let v = match axis {
                    Axis::X => x,
                    Axis::Y => -y,
                };
                self.look_add.push((i, v as f32 * max));
            }
        }
        if !self.animator.animating() && !moving {
            return false;
        }
        self.animator
            .step(dt, &mut self.params, &mut self.parts, &self.look_add);
        self.model.parameters().copy_from_slice(&self.params);
        self.model.part_opacities().copy_from_slice(&self.parts);
        self.model.update();
        self.dirty = true;
        self.animator.animating() || moving
    }

    /// 바뀐 것이 있으면 결과 텍스처에 다시 그린다.
    pub fn render(&mut self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder) {
        if !self.dirty {
            return;
        }
        let Some(output) = &self.output else {
            return;
        };
        let drawables = self.model.drawables();
        let positions = &mut self.scratch_positions;
        positions.clear();
        positions.extend(
            drawables
                .iter()
                .flat_map(|dr| dr.positions.iter().flat_map(|v| [v.x, v.y])),
        );
        if !positions.is_empty() {
            gpu.queue
                .write_buffer(&self.positions, 0, bytemuck_f32(positions));
        }
        let lin = |c: f32| srgb_to_linear(f64::from(c)) as f32;
        let uniforms = &mut self.scratch_uniforms;
        uniforms.clear();
        uniforms.resize(self.drawable_stride as usize * drawables.len(), 0);
        for (i, dr) in drawables.iter().enumerate() {
            let inverted = dr.flags & live2d::INVERTED_MASK != 0;
            let values = [
                dr.opacity,
                f32::from(u8::from(self.mask_of[i].is_some())),
                f32::from(u8::from(inverted)),
                0.0,
                lin(dr.multiply.x),
                lin(dr.multiply.y),
                lin(dr.multiply.z),
                1.0,
                lin(dr.screen.x),
                lin(dr.screen.y),
                lin(dr.screen.z),
                0.0,
            ];
            let at = i * self.drawable_stride as usize;
            uniforms[at..at + DRAWABLE_FLOATS * size_of::<f32>()]
                .copy_from_slice(bytemuck_f32(&values));
        }
        gpu.queue.write_buffer(&self.drawable_uniforms, 0, uniforms);

        let shown = |i: usize| {
            let dr = &drawables[i];
            dr.visible && self.ranges[i].index_count > 0
        };
        let set_geometry = |pass: &mut wgpu::RenderPass<'_>| {
            pass.set_vertex_buffer(0, self.positions.slice(..));
            pass.set_vertex_buffer(1, self._uvs.slice(..));
            pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint16);
        };
        let draw = |pass: &mut wgpu::RenderPass<'_>, i: usize| {
            let r = self.ranges[i];
            pass.draw_indexed(
                r.first_index..r.first_index + r.index_count,
                r.base_vertex,
                0..1,
            );
        };
        let attachment = |view, load| wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        };
        let clear = wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT);

        for set in &self.mask_sets {
            let Some((target, _)) = &set.target else {
                continue;
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("waywall live2d mask"),
                color_attachments: &[Some(attachment(&target.view, clear))],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.mask);
            pass.set_bind_group(0, &self.layer_group, &[]);
            set_geometry(&mut pass);
            for &m in &set.masks {
                // −1: 숨은 마스크 drawable.
                let Ok(m) = usize::try_from(m) else { continue };
                if m >= drawables.len() || !shown(m) {
                    continue;
                }
                pass.set_bind_group(1, &self.texture_groups[drawables[m].texture as usize], &[]);
                draw(&mut pass, m);
            }
        }

        let mut order = std::mem::take(&mut self.scratch_order);
        order.clear();
        order.extend((0..drawables.len()).filter(|&i| shown(i) && drawables[i].opacity > 0.0));
        order.sort_by_key(|&i| drawables[i].render_order);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("waywall live2d"),
            color_attachments: &[Some(attachment(&output.view, clear))],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &self.layer_group, &[]);
        set_geometry(&mut pass);
        for &i in &order {
            let dr = &drawables[i];
            let blend = DrawBlend::from_flags(dr.flags);
            let k = DRAW_BLENDS
                .iter()
                .position(|&b| b == blend)
                .unwrap_or_default();
            pass.set_pipeline(&self.main[k]);
            pass.set_bind_group(1, &self.texture_groups[dr.texture as usize], &[]);
            let mask_group = self.mask_of[i]
                .and_then(|s| self.mask_sets[s].target.as_ref())
                .map_or(&self.no_mask_group, |(_, g)| g);
            pass.set_bind_group(2, mask_group, &[]);
            let offset = u32::try_from(i as u64 * self.drawable_stride).unwrap_or_default();
            pass.set_bind_group(3, &self.drawable_group, &[offset]);
            draw(&mut pass, i);
        }
        drop(pass);
        self.scratch_order = order;
        self.dirty = false;
    }
}

/// 점이 삼각형 안(경계 포함)인지. 감는 방향과 관계없다.
fn point_in_triangle(p: [f32; 2], a: live2d::Vec2, b: live2d::Vec2, c: live2d::Vec2) -> bool {
    let cross =
        |o: live2d::Vec2, q: live2d::Vec2| (q.x - o.x) * (p[1] - o.y) - (q.y - o.y) * (p[0] - o.x);
    let (d1, d2, d3) = (cross(a, b), cross(b, c), cross(c, a));
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangle_hit() {
        let v = |x, y| live2d::Vec2 { x, y };
        let (a, b, c) = (v(0.0, 0.0), v(1.0, 0.0), v(0.0, 1.0));
        assert!(point_in_triangle([0.2, 0.2], a, b, c));
        assert!(
            point_in_triangle([0.2, 0.2], a, c, b),
            "winding must not matter"
        );
        assert!(!point_in_triangle([0.8, 0.8], a, b, c));
    }

    /// 캔버스 중심이 position(기본: rect 중심)에 오고, 캔버스 높이 × scale이 rect 높이와 같으면 위아래 끝이 clip ±1.
    #[test]
    fn transform_maps_canvas_into_rect() {
        let c = live2d::Canvas {
            size: [1000.0, 2000.0],
            origin: [500.0, 1000.0],
            pixels_per_unit: 1000.0,
        };
        let rect = [0.0, 0.0, 540.0, 1080.0];
        let [sx, sy, ox, oy] = layer_transform(c, rect, [270.0, 540.0], 0.54);
        // 모델 원점(캔버스 중심) → clip (0, 0).
        assert!(ox.abs() < 1e-6 && oy.abs() < 1e-6, "{ox} {oy}");
        // 위로 1 단위(= 캔버스 1000 px × 0.54 = 540 설계 px) → clip y = +1 (rect 위 끝).
        assert!((sy - 1.0).abs() < 1e-6, "{sy}");
        assert!((sx - 2.0).abs() < 1e-6, "{sx}");
    }
}
