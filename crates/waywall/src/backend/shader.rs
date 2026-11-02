//! 셰이더 배경 렌더러: 화면을 덮는 삼각형 하나 + Shadertoy uniform
//! (docs/wallpaper-format.md#셰이더-배경).

use anyhow::{Context, Result};
use chrono::{Datelike, Local, Timelike};

use crate::backend::view::FrameInfo;
use crate::gpu::shadertoy::{Compiled, Inputs, UniformLayout};
use crate::gpu::{FULLSCREEN_ENTRY, Gpu};
use crate::properties::Property;

pub struct ShaderSpec {
    pub compiled: Compiled,
    pub props: Vec<Property>,
}

pub struct ShaderRenderer {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    layout: UniformLayout,
    props: Vec<Property>,
}

impl ShaderRenderer {
    pub fn new(
        gpu: &Gpu,
        spec: ShaderSpec,
        format: wgpu::TextureFormat,
        name: &str,
    ) -> Result<Self> {
        let layout = spec.compiled.layout.clone();
        let entry = spec.compiled.entry;
        let fragment = gpu.create_module(name, spec.compiled.module)?;
        let device = &gpu.device;
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("waywall shader uniforms"),
            size: layout.buffer_size(),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (pipeline, bind_group) = gpu
            .scoped(|d| {
                let bgl = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("waywall shader globals"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    }],
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
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
                let bind_group = d.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("waywall shader globals"),
                    layout: &bgl,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniforms.as_entire_binding(),
                    }],
                });
                (pipeline, bind_group)
            })
            .with_context(|| format!("{name}: failed to create the render pipeline"))?;
        Ok(Self {
            pipeline,
            uniforms,
            bind_group,
            layout,
            props: spec.props,
        })
    }

    pub fn render(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        f: &FrameInfo<'_>,
        name: &str,
    ) {
        let date = Local::now();
        let midnight =
            f64::from(date.num_seconds_from_midnight()) + f64::from(date.nanosecond()) / 1e9;
        let p = f.pointer;
        let inputs = Inputs {
            resolution: [f.size.0 as f32, f.size.1 as f32, 1.0],
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
            mouse: p.mouse,
            pointer: [
                p.pos[0],
                p.pos[1],
                f32::from(u8::from(p.inside)),
                p.buttons as f32,
            ],
            click: p.click,
        };
        gpu.queue
            .write_buffer(&self.uniforms, 0, &self.layout.write(&inputs, &self.props));

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(name),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // 삼각형이 전부 덮으므로 이전 내용은 필요 없다.
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
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
