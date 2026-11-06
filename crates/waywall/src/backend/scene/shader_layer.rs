//! 씬 shader 레이어의 GPU 쪽 (docs/architecture.md#씬-렌더러).
//!
//! 셰이더는 `rect` 크기의 자기 텍스처에 그리고, 그 텍스처를 image 레이어와 같은 사각형(`GpuLayer`)으로 합성한다.
//! `"below"`·레이어 입력은 그리기 직전에 원본에서 rect 영역을 잘라 채널 텍스처에 쓴다(`Cropper`).

use std::collections::HashMap;

use anyhow::Result;

use super::render::{
    FitMap, FollowState, GpuLayer, LayerBindings, LayerPlacement, TEXTURE_FORMAT, blend_state,
    upload_texture,
};
use super::{ChannelInput, ShaderLayer};
use crate::backend::shader::{Activity, Draw, ShaderRenderer, empty_texture, frame_inputs};
use crate::backend::view::FrameInfo;
use crate::gpu::shadertoy::Language;
use crate::gpu::{FULLSCREEN_ENTRY, Gpu};

/// 셰이더 결과와 잘라 낸 입력의 포맷: 필터링 가능한 HDR.
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// `scene/input.wgsl` 엔트리 (인터페이스).
const FS_ENTRY: &str = "fs_main";
/// input.wgsl `Crop`: origin(i32 2개), flip, height, step, 패딩 (32비트 단어 8개).
const CROP_WORDS: usize = 8;

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

    /// `run`에 줄 uniform 버퍼.
    pub fn uniforms(&self, d: &wgpu::Device, label: &str) -> wgpu::Buffer {
        d.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (CROP_WORDS * size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// 원본·uniform이 바뀔 때(크기 변경)만 만든다.
    pub fn bind(
        &self,
        d: &wgpu::Device,
        src: &wgpu::TextureView,
        uniforms: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        d.create_bind_group(&wgpu::BindGroupDescriptor {
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
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        dst: &Target,
        uniforms: &wgpu::Buffer,
        bind_group: &wgpu::BindGroup,
        origin: [i32; 2],
        flip: bool,
        step: f32,
    ) {
        let words: [u32; CROP_WORDS] = [
            origin[0] as u32,
            origin[1] as u32,
            u32::from(flip),
            dst.texture.height(),
            step.to_bits(),
            0,
            0,
            0,
        ];
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_ne_bytes()).collect();
        gpu.queue.write_buffer(uniforms, 0, &bytes);
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
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

enum Input {
    /// 잘라 낸 결과 (크기에 따라 다시 만듦).
    Crop {
        source: CropSource,
        uniforms: wgpu::Buffer,
        /// 결과 텍스처와, 원본을 묶은 바인드 그룹.
        bound: Option<(Target, wgpu::BindGroup)>,
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
    /// 언제 계속 그릴지 (docs/wallpaper-format.md#잠들기-animate).
    pub activity: Activity,
    /// 텍스처 경유일 때 결과를 합성하는 사각형. 직접 그릴 때는 위치(패럴랙스)만 쓴다.
    pub quad: GpuLayer,
    id: String,
    rect: [f64; 4],
    lang: Language,
    render_scale: f64,
    /// HDR 버퍼에 직접 그리는지 (docs/architecture.md#씬-렌더러).
    direct: bool,
    renderer: ShaderRenderer,
    /// 채널마다 (길이 = 채널 수).
    inputs: Vec<Option<Input>>,
    /// 텍스처 경유일 때의 결과.
    output: Option<Target>,
    /// 그리는 영역 크기 (렌더 px).
    size: (u32, u32),
    _empty: wgpu::Texture,
    empty_view: wgpu::TextureView,
}

/// 크기가 바뀔 때 다시 묶을 원본들.
pub struct Sources<'a> {
    pub below: &'a wgpu::TextureView,
    pub isolated: &'a HashMap<usize, Target>,
    pub cropper: Option<&'a Cropper>,
}

impl GpuShaderLayer {
    pub fn new(
        gpu: &Gpu,
        d: &wgpu::Device,
        spec: Box<ShaderLayer>,
        hdr_format: wgpu::TextureFormat,
    ) -> Result<Self> {
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
            render_scale,
            animate,
            direct,
            clip,
            follow,
        } = *spec;
        let activity = Activity::new(animate, &compiled.reads);
        let (format, blend_state) = if direct {
            (hdr_format, Some(blend_state(blend)))
        } else {
            (OUTPUT_FORMAT, None)
        };
        let renderer =
            ShaderRenderer::new(gpu, compiled, props, sampling, format, blend_state, &id)?;
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
        quad.follow = follow.map(FollowState::new);
        let inputs: Vec<Option<Input>> = inputs
            .into_iter()
            .map(|i| {
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
                        uniforms: d.create_buffer(&wgpu::BufferDescriptor {
                            label: Some(&id),
                            size: (CROP_WORDS * size_of::<u32>()) as u64,
                            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        }),
                        bound: None,
                    },
                })
            })
            .collect();
        let empty = upload_texture(gpu, d, &id, &empty_texture(), TEXTURE_FORMAT);
        let empty_view = empty.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Self {
            activity,
            quad,
            id,
            rect,
            lang,
            render_scale,
            direct,
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

    /// HDR 버퍼에 직접 그린다 (레이어 패스를 이 앞에서 끊고, 합성 사각형은 그리지 않음).
    pub fn direct(&self) -> bool {
        self.direct
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

    /// 출력 크기에 맞춰 결과·입력 텍스처를 만들고 다시 묶는다. 원본(HDR 버퍼 등)도 바뀌므로 매번 다시 묶는다.
    pub fn resize(&mut self, gpu: &Gpu, map: FitMap, b: &LayerBindings<'_>, src: &Sources<'_>) {
        let d = &gpu.device;
        self.size = (
            ((self.rect[2] * map.scale[0] * self.render_scale).round() as u32).max(1),
            ((self.rect[3] * map.scale[1] * self.render_scale).round() as u32).max(1),
        );
        for input in self.inputs.iter_mut().flatten() {
            let Input::Crop {
                source,
                uniforms,
                bound,
            } = input
            else {
                continue;
            };
            let from = match source {
                CropSource::Below => Some(src.below),
                CropSource::Layer(j) => src.isolated.get(j).map(|t| &t.view),
            };
            *bound = from.zip(src.cropper).map(|(from, cropper)| {
                let target = Target::new(d, &self.id, OUTPUT_FORMAT, self.size);
                let group = cropper.bind(d, from, uniforms);
                (target, group)
            });
        }
        let views: Vec<&wgpu::TextureView> = self
            .inputs
            .iter()
            .map(|i| match i {
                Some(Input::Crop {
                    bound: Some((t, _)),
                    ..
                }) => &t.view,
                Some(Input::Image { view, .. }) => view,
                _ => &self.empty_view,
            })
            .collect();
        self.renderer.bind(gpu, &views);
        if !self.direct {
            let output = Target::new(d, &self.id, OUTPUT_FORMAT, self.size);
            let clip = self
                .quad
                .clip
                .and_then(|j| src.isolated.get(&j))
                .map(|t| &t.view);
            self.quad.bind(d, b, &output.view, None, clip);
            self.output = Some(output);
        }
    }

    /// 입력을 잘라 내고 셰이더를 그린다. 직접 그리면 `hdr`에 블렌드, 아니면 결과 텍스처에.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        cropper: &Cropper,
        f: &FrameInfo<'_>,
        map: FitMap,
        shift: [f64; 2],
        hdr: &wgpu::TextureView,
    ) {
        let p = self.quad.parallax;
        let o = self.quad.offset();
        let top_left = map.to_render([
            self.rect[0] + shift[0] * p + o[0],
            self.rect[1] + shift[1] * p + o[1],
        ]);
        let origin = [top_left[0].round() as i32, top_left[1].round() as i32];
        let flip = self.lang == Language::Glsl;
        let mut resolution = vec![[0.0; 3]; self.inputs.len()];
        for (i, input) in self.inputs.iter().enumerate() {
            match input {
                Some(Input::Crop {
                    uniforms,
                    bound: Some((target, group)),
                    ..
                }) => {
                    let step = (1.0 / self.render_scale) as f32;
                    cropper.run(gpu, encoder, target, uniforms, group, origin, flip, step);
                    resolution[i] = [self.size.0 as f32, self.size.1 as f32, 1.0];
                }
                Some(Input::Image { size, .. }) => {
                    resolution[i] = [size[0] as f32, size[1] as f32, 1.0];
                }
                _ => {}
            }
        }
        // rect의 왼쪽 아래 (렌더 px, 왼쪽 아래 원점). 화면에서 rect의 높이는 그리는 크기 / render_scale.
        let screen_h = (self.size.1 as f64 / self.render_scale) as f32;
        let bottom = f.size.1 as f32 - (origin[1] as f32 + screen_h);
        let scale = self.render_scale as f32;
        let mut inputs = frame_inputs(f, self.size, [origin[0] as f32, bottom], scale, resolution);
        let draw = if self.direct {
            let Some(scissor) = clip_rect(origin, self.size, f.size, map.scissor()) else {
                return;
            };
            inputs.origin = [origin[0] as f32, origin[1] as f32];
            Draw {
                target: hdr,
                load: wgpu::LoadOp::Load,
                scissor: Some(scissor),
            }
        } else {
            let Some(output) = &self.output else { return };
            Draw {
                target: &output.view,
                // 삼각형이 전부 덮으므로 이전 내용은 필요 없다.
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                scissor: None,
            }
        };
        self.renderer.render(gpu, encoder, draw, &inputs, &self.id);
    }
}

/// 렌더 타깃 안의 사각형 `origin + size`와 설계 영역 scissor의 교집합. 비면 `None`.
fn clip_rect(
    origin: [i32; 2],
    size: (u32, u32),
    target: (u32, u32),
    design: Option<(u32, u32, u32, u32)>,
) -> Option<(u32, u32, u32, u32)> {
    let (dx, dy, dw, dh) = design?;
    let x0 = i64::from(origin[0]).max(i64::from(dx));
    let y0 = i64::from(origin[1]).max(i64::from(dy));
    let x1 = (i64::from(origin[0]) + i64::from(size.0))
        .min(i64::from(dx + dw))
        .min(i64::from(target.0));
    let y1 = (i64::from(origin[1]) + i64::from(size.1))
        .min(i64::from(dy + dh))
        .min(i64::from(target.1));
    (x1 > x0 && y1 > y0).then(|| (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_scissor_clips_to_target_and_design() {
        let full = Some((0, 0, 100, 50));
        assert_eq!(
            clip_rect([10, 5], (20, 10), (100, 50), full),
            Some((10, 5, 20, 10))
        );
        // 패럴랙스로 밖으로 밀려남.
        assert_eq!(
            clip_rect([-4, -2], (100, 50), (100, 50), full),
            Some((0, 0, 96, 48))
        );
        // contain 레터박스.
        assert_eq!(
            clip_rect([0, 0], (100, 50), (100, 50), Some((0, 10, 100, 30))),
            Some((0, 10, 100, 30))
        );
        assert_eq!(clip_rect([200, 0], (10, 10), (100, 50), full), None);
        assert_eq!(clip_rect([0, 0], (10, 10), (100, 50), None), None);
    }
}
