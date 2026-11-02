//! Shadertoy 형식 셰이더 렌더러: 화면을 덮는 삼각형 하나 + Shadertoy uniform + 텍스처 채널 4개
//! (docs/wallpaper-format.md#셰이더-배경).
//!
//! `ShaderRenderer`는 셰이더 배경과 씬 shader 레이어가 함께 쓴다. 채널에 무엇을 묶을지는 쓰는 쪽이 정한다.

use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use chrono::{Datelike, Local, Timelike};
use toml::{Table, Value};
use waywall_proto::config::{ChannelDefaults, ChannelFilter, ChannelWrap};

use crate::backend::scene::Texture;
use crate::backend::view::FrameInfo;
use crate::config::merge;
use crate::gpu::shadertoy::{Compiled, Inputs, Language, Slots, UniformLayout};
use crate::gpu::{FULLSCREEN_ENTRY, Gpu};
use crate::properties::Property;

pub const CHANNELS: usize = 4;
/// 채널 이름 (`[inputs]`의 키).
pub const CHANNEL_NAMES: [&str; CHANNELS] = ["iChannel0", "iChannel1", "iChannel2", "iChannel3"];
/// prelude의 바인딩: 0 = uniform, 1–4 = 채널 텍스처, 5–8 = 채널 샘플러.
const TEXTURE_BINDING: u32 = 1;
const SAMPLER_BINDING: u32 = TEXTURE_BINDING + CHANNELS as u32;
/// 채널 입력 표의 필드.
const SOURCE_KEY: &str = "source";
/// `[types.<타입>]` 아래 채널 기본값 섹션 이름.
pub const CHANNEL_SECTION: &str = "channel";
/// 지정하지 않은 채널에 묶는 1×1 투명 텍스처.
const EMPTY_TEXEL: [u8; 4] = [0; 4];

/// `[inputs]`의 채널 하나 (병합 후).
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelDef {
    pub source: String,
    pub sampling: ChannelDefaults,
}

/// `inputs` 표 → 채널별 정의. 계층: `defaults`(내장 < 사용자) < 채널 값 < `user`(사용자가 직접 적은 섹션).
pub fn parse_inputs(
    inputs: Option<&Table>,
    defaults: &ChannelDefaults,
    user: Option<&Table>,
) -> Result<[Option<ChannelDef>; CHANNELS]> {
    let mut out: [Option<ChannelDef>; CHANNELS] = Default::default();
    let Some(inputs) = inputs else {
        return Ok(out);
    };
    for (key, value) in inputs {
        let Some(i) = CHANNEL_NAMES.iter().position(|n| n == key) else {
            bail!("inputs: unknown channel `{key}` (iChannel0–iChannel3)");
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
    /// 지정하지 않은 채널은 `None` (1×1 투명).
    pub channels: [Option<Texture>; CHANNELS],
    /// 채널마다 샘플링 (지정하지 않은 채널은 기본값, 쓰이지 않음).
    pub sampling: [ChannelDefaults; CHANNELS],
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
    pub fn new(
        gpu: &Gpu,
        compiled: Compiled,
        props: Vec<Property>,
        sampling: [ChannelDefaults; CHANNELS],
        format: wgpu::TextureFormat,
        blend: Option<wgpu::BlendState>,
        name: &str,
    ) -> Result<Self> {
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
                for i in 0..CHANNELS as u32 {
                    entries.push(wgpu::BindGroupLayoutEntry {
                        binding: TEXTURE_BINDING + i,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    });
                    entries.push(wgpu::BindGroupLayoutEntry {
                        binding: SAMPLER_BINDING + i,
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

    /// 채널 텍스처를 묶는다. 텍스처가 바뀔 때마다(크기 변경 등) 다시 부른다.
    pub fn bind(&mut self, gpu: &Gpu, views: [&wgpu::TextureView; CHANNELS]) {
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: self.uniforms.as_entire_binding(),
        }];
        for (i, (view, sampler)) in views.iter().zip(&self.samplers).enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: TEXTURE_BINDING + i as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: SAMPLER_BINDING + i as u32,
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

/// 프레임 값 → uniform 입력. `size`: 셰이더가 그리는 크기(px), `origin`: 그 영역의 왼쪽 아래(렌더 px, 왼쪽 아래 원점).
pub fn frame_inputs(
    f: &FrameInfo<'_>,
    size: (u32, u32),
    origin: [f32; 2],
    channel_resolution: [[f32; 3]; CHANNELS],
) -> Inputs {
    let date = Local::now();
    let midnight = f64::from(date.num_seconds_from_midnight()) + f64::from(date.nanosecond()) / 1e9;
    let p = f.pointer;
    let local = |x: f32, y: f32| [x - origin[0], y - origin[1]];
    // iMouse의 z, w는 부호가 상태를 나타내므로 크기만 옮긴다.
    let shift = |v: f32, o: f32| if v < 0.0 { -(v.abs() - o) } else { v - o };
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
    channel_resolution: [[f32; 3]; CHANNELS],
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
        let mut channel_resolution = [[0.0; 3]; CHANNELS];
        let mut textures = Vec::with_capacity(CHANNELS);
        let ShaderSpec {
            compiled,
            props,
            channels,
            sampling,
        } = spec;
        for (i, ch) in channels.into_iter().enumerate() {
            let texture = match ch {
                Some(t) => {
                    ensure!(
                        t.width <= max_dim && t.height <= max_dim,
                        "{}: texture {}x{} is larger than the GPU limit {max_dim}",
                        CHANNEL_NAMES[i],
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
                    CHANNEL_NAMES[i],
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
        renderer.bind(gpu, [&views[0], &views[1], &views[2], &views[3]]);
        Ok(Self {
            renderer,
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
        let inputs = frame_inputs(f, f.size, [0.0, 0.0], self.channel_resolution);
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
            ("iChannel4 = \"a.png\"", "iChannel4"),
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
}
