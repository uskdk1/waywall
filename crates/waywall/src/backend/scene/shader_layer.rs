//! 씬 shader 레이어의 GPU 쪽 (docs/architecture.md#씬-렌더러).
//!
//! 셰이더는 `rect` 크기의 자기 텍스처에 그리고, 그 텍스처를 image 레이어와 같은 사각형(`GpuLayer`)으로 합성한다.
//! `"below"`·레이어 입력은 그리기 직전에 원본에서 rect 영역을 잘라 채널 텍스처에 쓴다(`Cropper`).

use std::collections::HashMap;

use anyhow::Result;

use super::render::{
    FitMap, GpuLayer, LayerBindings, LayerPlacement, TEXTURE_FORMAT, upload_texture,
};
use super::{ChannelInput, ShaderLayer};
use crate::backend::shader::{CHANNELS, ShaderRenderer, empty_texture, frame_inputs};
use crate::backend::view::FrameInfo;
use crate::gpu::shadertoy::Language;
use crate::gpu::{FULLSCREEN_ENTRY, Gpu};

/// 셰이더 결과와 잘라 낸 입력의 포맷: 필터링 가능한 HDR.
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// `scene/input.wgsl` 엔트리 (인터페이스).
const FS_ENTRY: &str = "fs_main";
/// input.wgsl `Crop`: origin(i32 2개), flip, height (32비트 단어 4개).
const CROP_WORDS: usize = 4;

/// 렌더 타깃으로도, 채널로도 쓰는 텍스처.
pub struct Target {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

impl Target {
    pub fn new(
        d: &wgpu::Device,
        label: &str,
        format: wgpu::TextureFormat,
        size: (u32, u32),
    ) -> Self {
        let texture = d.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { texture, view }
    }
}

/// 원본 텍스처에서 영역을 잘라 채널 텍스처에 쓰는 패스 (`scene/input.wgsl`).
pub struct Cropper {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl Cropper {
    pub fn new(gpu: &Gpu, d: &wgpu::Device, module: &wgpu::ShaderModule) -> Self {
        let layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waywall scene input"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pl = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("waywall scene input"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("waywall scene input"),
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
        });
        Self { pipeline, layout }
    }

    #[allow(clippy::too_many_arguments)]
    fn run(
        &self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        dst: &Target,
        uniforms: &wgpu::Buffer,
        origin: [i32; 2],
        flip: bool,
    ) {
        let words: [u32; CROP_WORDS] = [
            origin[0] as u32,
            origin[1] as u32,
            u32::from(flip),
            dst.texture.height(),
        ];
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_ne_bytes()).collect();
        gpu.queue.write_buffer(uniforms, 0, &bytes);
        let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waywall scene input"),
            layout: &self.layout,
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
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("waywall scene input"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &dst.view,
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

enum Input {
    /// 잘라 낸 결과 (크기에 따라 다시 만듦).
    Crop {
        source: CropSource,
        target: Option<Target>,
        uniforms: wgpu::Buffer,
    },
    Image {
        _texture: wgpu::Texture,
        view: wgpu::TextureView,
        size: [u32; 2],
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CropSource {
    Below,
    Layer(usize),
}

pub struct GpuShaderLayer {
    pub quad: GpuLayer,
    id: String,
    rect: [f64; 4],
    lang: Language,
    renderer: ShaderRenderer,
    inputs: [Option<Input>; CHANNELS],
    output: Option<Target>,
    /// 결과 텍스처 크기 (렌더 px).
    size: (u32, u32),
    _empty: wgpu::Texture,
    empty_view: wgpu::TextureView,
}

impl GpuShaderLayer {
    pub fn new(gpu: &Gpu, d: &wgpu::Device, spec: Box<ShaderLayer>) -> Result<Self> {
        let ShaderLayer {
            id,
            blend,
            opacity,
            parallax,
            rect,
            compiled,
            props,
            lang,
            inputs,
            sampling,
        } = *spec;
        let renderer = ShaderRenderer::new(gpu, compiled, props, sampling, OUTPUT_FORMAT, &id)?;
        let quad = GpuLayer::new(
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
        let inputs = inputs.map(|i| {
            i.map(|i| match i {
                ChannelInput::Image(t) => {
                    let texture = upload_texture(gpu, d, &id, &t, TEXTURE_FORMAT);
                    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                    Input::Image {
                        _texture: texture,
                        view,
                        size: [t.width, t.height],
                    }
                }
                ChannelInput::Below | ChannelInput::Layer(_) => Input::Crop {
                    source: match i {
                        ChannelInput::Layer(j) => CropSource::Layer(j),
                        _ => CropSource::Below,
                    },
                    target: None,
                    uniforms: d.create_buffer(&wgpu::BufferDescriptor {
                        label: Some(&id),
                        size: (CROP_WORDS * size_of::<u32>()) as u64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                },
            })
        });
        let empty = upload_texture(gpu, d, &id, &empty_texture(), TEXTURE_FORMAT);
        let empty_view = empty.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Self {
            quad,
            id,
            rect,
            lang,
            renderer,
            inputs,
            output: None,
            size: (0, 0),
            _empty: empty,
            empty_view,
        })
    }

    pub fn parallax(&self) -> f64 {
        self.quad.parallax
    }

    /// 레이어 패스를 이 레이어 앞에서 끊어야 하는지 (`"below"` 입력).
    pub fn needs_below(&self) -> bool {
        self.inputs.iter().flatten().any(|i| {
            matches!(
                i,
                Input::Crop {
                    source: CropSource::Below,
                    ..
                }
            )
        })
    }

    /// 입력으로 쓰는 레이어 (`SceneSpec::layers`의 인덱스).
    pub fn layer_inputs(&self) -> impl Iterator<Item = usize> + '_ {
        self.inputs.iter().flatten().filter_map(|i| match i {
            Input::Crop {
                source: CropSource::Layer(j),
                ..
            } => Some(*j),
            _ => None,
        })
    }

    /// 출력 크기에 맞춰 결과·입력 텍스처를 만들고 다시 묶는다.
    pub fn resize(&mut self, gpu: &Gpu, map: FitMap, b: &LayerBindings<'_>) {
        let size = (
            ((self.rect[2] * map.scale[0]).round() as u32).max(1),
            ((self.rect[3] * map.scale[1]).round() as u32).max(1),
        );
        if self.output.is_some() && size == self.size {
            return;
        }
        let d = &gpu.device;
        self.size = size;
        let output = Target::new(d, &self.id, OUTPUT_FORMAT, size);
        for input in self.inputs.iter_mut().flatten() {
            if let Input::Crop { target, .. } = input {
                *target = Some(Target::new(d, &self.id, OUTPUT_FORMAT, size));
            }
        }
        let views: Vec<&wgpu::TextureView> = self
            .inputs
            .iter()
            .map(|i| match i {
                Some(Input::Crop {
                    target: Some(t), ..
                }) => &t.view,
                Some(Input::Image { view, .. }) => view,
                _ => &self.empty_view,
            })
            .collect();
        self.renderer
            .bind(gpu, [views[0], views[1], views[2], views[3]]);
        self.quad.bind(d, b, &output.view, None);
        self.output = Some(output);
    }

    /// 입력을 잘라 내고 셰이더를 결과 텍스처에 그린다. `below`: 지금까지 그린 장면, `isolated`: 레이어 단독 결과.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        cropper: &Cropper,
        f: &FrameInfo<'_>,
        map: FitMap,
        shift: [f64; 2],
        below: &wgpu::TextureView,
        isolated: &HashMap<usize, Target>,
    ) {
        let Some(output) = &self.output else { return };
        let p = self.quad.parallax;
        let top_left = map.to_render([self.rect[0] + shift[0] * p, self.rect[1] + shift[1] * p]);
        let origin = [top_left[0].round() as i32, top_left[1].round() as i32];
        let flip = self.lang == Language::Glsl;
        let mut resolution = [[0.0; 3]; CHANNELS];
        for (i, input) in self.inputs.iter().enumerate() {
            match input {
                Some(Input::Crop {
                    source,
                    target: Some(target),
                    uniforms,
                }) => {
                    let src = match source {
                        CropSource::Below => Some(below),
                        CropSource::Layer(j) => isolated.get(j).map(|t| &t.view),
                    };
                    if let Some(src) = src {
                        cropper.run(gpu, encoder, src, target, uniforms, origin, flip);
                    }
                    resolution[i] = [self.size.0 as f32, self.size.1 as f32, 1.0];
                }
                Some(Input::Image { size, .. }) => {
                    resolution[i] = [size[0] as f32, size[1] as f32, 1.0];
                }
                _ => {}
            }
        }
        // rect의 왼쪽 아래 (렌더 px, 왼쪽 아래 원점).
        let bottom = f.size.1 as f32 - (origin[1] as f32 + self.size.1 as f32);
        let inputs = frame_inputs(f, self.size, [origin[0] as f32, bottom], resolution);
        self.renderer
            .render(gpu, encoder, &output.view, &inputs, &self.id);
    }
}
