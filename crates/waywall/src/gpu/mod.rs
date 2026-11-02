//! GPU(wgpu, Vulkan) 공유 상태 (docs/architecture.md#셰이더씬-wgpu).
//!
//! shader/scene 배경이 처음 필요할 때 만들고, 쓰는 출력이 없어지면 해제한다.

pub mod shadertoy;

use std::borrow::Cow;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use wayland_client::Proxy;
use wayland_client::protocol::wl_surface::WlSurface;
use waywall_proto::config::{GpuPreference, Render};

use crate::assets::Assets;

const FULLSCREEN_VS: &str = "shaders/fullscreen.wgsl";
/// `fullscreen.wgsl`의 엔트리 (인터페이스).
pub const FULLSCREEN_ENTRY: &str = "vs_main";

pub struct Gpu {
    instance: wgpu::Instance,
    display: NonNull<c_void>,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// 화면을 덮는 삼각형 버텍스 셰이더.
    pub fullscreen: wgpu::ShaderModule,
}

impl Gpu {
    /// `surface`와 호환되는 어댑터를 고르고, 그 surface의 wgpu Surface도 돌려준다.
    pub fn new(
        render: &Render,
        assets: &Assets,
        display: NonNull<c_void>,
        surface: &WlSurface,
    ) -> Result<(Self, wgpu::Surface<'static>)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            flags: wgpu::InstanceFlags::from_build_config().with_env(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let target = create_surface(&instance, display, surface)?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: match render.gpu {
                GpuPreference::LowPower => wgpu::PowerPreference::LowPower,
                GpuPreference::HighPerformance => wgpu::PowerPreference::HighPerformance,
            },
            compatible_surface: Some(&target),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .context(
            "no Vulkan adapter can present to this compositor (is a Vulkan driver installed?)",
        )?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("waywall"),
            // 상시 실행이므로 속도보다 메모리를 아낀다.
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            ..Default::default()
        }))
        .context("failed to create GPU device")?;
        device.on_uncaptured_error(Arc::new(|e: wgpu::Error| {
            tracing::error!("wgpu: {e}");
        }));
        tracing::info!(adapter = %info.name, driver = %info.driver, backend = ?info.backend, "GPU initialized");

        let src = assets.read(FULLSCREEN_VS)?;
        let src =
            std::str::from_utf8(&src).with_context(|| format!("{FULLSCREEN_VS} is not UTF-8"))?;
        let module = naga::front::wgsl::parse_str(src)
            .map_err(|e| anyhow!("{FULLSCREEN_VS}: {}", e.emit_to_string(src)))?;
        let fullscreen = create_module(&device, FULLSCREEN_VS, module)?;
        let gpu = Self {
            instance,
            display,
            adapter,
            device,
            queue,
            fullscreen,
        };
        Ok((gpu, target))
    }

    pub fn create_surface(&self, surface: &WlSurface) -> Result<wgpu::Surface<'static>> {
        create_surface(&self.instance, self.display, surface)
    }

    /// 검증된 naga 모듈 → wgpu 모듈. 검증 오류는 패닉 대신 `Err`.
    pub fn create_module(&self, label: &str, module: naga::Module) -> Result<wgpu::ShaderModule> {
        create_module(&self.device, label, module)
    }

    /// wgpu 검증 오류를 잡아 `Err`로 돌려준다.
    pub fn scoped<T>(&self, f: impl FnOnce(&wgpu::Device) -> T) -> Result<T> {
        scoped(&self.device, f)
    }
}

/// WGSL 소스 → 검증된 naga 모듈. GPU 없이 동작하므로 배경을 고를 때(`set`) 오류를 알릴 수 있다.
pub fn compile_wgsl(name: &str, src: &str) -> Result<naga::Module> {
    let module = naga::front::wgsl::parse_str(src)
        .map_err(|e| anyhow!("{name}: {}", e.emit_to_string(src)))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map_err(|e| anyhow!("{name}: {}", e.emit_to_string(src)))?;
    Ok(module)
}

fn scoped<T>(device: &wgpu::Device, f: impl FnOnce(&wgpu::Device) -> T) -> Result<T> {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let value = f(device);
    if let Some(e) = pollster::block_on(scope.pop()) {
        bail!("{e}");
    }
    Ok(value)
}

/// 검증된 naga 모듈 → wgpu 모듈.
fn create_module(
    device: &wgpu::Device,
    label: &str,
    module: naga::Module,
) -> Result<wgpu::ShaderModule> {
    scoped(device, |d| {
        d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Naga(Cow::Owned(module)),
        })
    })
    .with_context(|| label.to_owned())
}

fn create_surface(
    instance: &wgpu::Instance,
    display: NonNull<c_void>,
    surface: &WlSurface,
) -> Result<wgpu::Surface<'static>> {
    let ptr = NonNull::new(surface.id().as_ptr().cast::<c_void>()).context("wl_surface is dead")?;
    // SAFETY: wl_display는 데몬이 끝날 때까지 살아 있고, wl_surface는 이 Surface를 가진
    // ShaderView가 Surface를 먼저 버린 뒤에 파괴한다 (필드 선언 순서).
    unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
                display,
            ))),
            raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(ptr)),
        })
    }
    .context("failed to create Vulkan surface")
}
