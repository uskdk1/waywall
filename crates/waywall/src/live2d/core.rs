//! Cubism Core C ABI(v6) 바인딩. 구현은 정적 링크한 PurismCore (build.rs).

use std::alloc::Layout;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_uint, c_ushort, c_void};

use anyhow::{Result, bail, ensure};

use super::physics::ParamRange;

#[repr(C)]
struct CsmMoc {
    _p: [u8; 0],
}
#[repr(C)]
struct CsmModel {
    _p: [u8; 0],
}
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

/// 헤더의 정렬 요구 (`csmAlignofMoc`, `csmAlignofModel`).
const MOC_ALIGN: usize = 64;
const MODEL_ALIGN: usize = 16;
/// drawable 상수 플래그 (`csmBlendAdditive` 등).
pub const BLEND_ADDITIVE: u8 = 1 << 0;
pub const BLEND_MULTIPLICATIVE: u8 = 1 << 1;
pub const INVERTED_MASK: u8 = 1 << 3;
/// drawable 동적 플래그 `csmIsVisible`.
const IS_VISIBLE: u8 = 1 << 0;

unsafe extern "C" {
    fn csmGetLatestMocVersion() -> c_uint;
    fn csmGetMocVersion(p: *const c_void, n: c_uint) -> c_uint;
    fn csmHasMocConsistency(p: *mut c_void, n: c_uint) -> c_int;
    fn csmReviveMocInPlace(p: *mut c_void, n: c_uint) -> *mut CsmMoc;
    fn csmGetSizeofModel(m: *const CsmMoc) -> c_uint;
    fn csmInitializeModelInPlace(m: *const CsmMoc, p: *mut c_void, n: c_uint) -> *mut CsmModel;
    fn csmUpdateModel(m: *mut CsmModel);
    fn csmReadCanvasInfo(m: *const CsmModel, size: *mut Vec2, origin: *mut Vec2, ppu: *mut f32);
    fn csmResetDrawableDynamicFlags(m: *mut CsmModel);
    fn csmGetParameterCount(m: *const CsmModel) -> c_int;
    fn csmGetParameterIds(m: *const CsmModel) -> *const *const c_char;
    fn csmGetParameterValues(m: *mut CsmModel) -> *mut f32;
    fn csmGetParameterDefaultValues(m: *const CsmModel) -> *const f32;
    fn csmGetParameterMinimumValues(m: *const CsmModel) -> *const f32;
    fn csmGetParameterMaximumValues(m: *const CsmModel) -> *const f32;
    fn csmGetPartCount(m: *const CsmModel) -> c_int;
    fn csmGetPartIds(m: *const CsmModel) -> *const *const c_char;
    fn csmGetPartOpacities(m: *mut CsmModel) -> *mut f32;
    fn csmGetDrawableCount(m: *const CsmModel) -> c_int;
    fn csmGetDrawableIds(m: *const CsmModel) -> *const *const c_char;
    fn csmGetDrawableConstantFlags(m: *const CsmModel) -> *const u8;
    fn csmGetDrawableDynamicFlags(m: *const CsmModel) -> *const u8;
    fn csmGetDrawableTextureIndices(m: *const CsmModel) -> *const c_int;
    fn csmGetRenderOrders(m: *const CsmModel) -> *const c_int;
    fn csmGetDrawableOpacities(m: *const CsmModel) -> *const f32;
    fn csmGetDrawableMaskCounts(m: *const CsmModel) -> *const c_int;
    fn csmGetDrawableMasks(m: *const CsmModel) -> *const *const c_int;
    fn csmGetDrawableVertexCounts(m: *const CsmModel) -> *const c_int;
    fn csmGetDrawableVertexPositions(m: *const CsmModel) -> *const *const Vec2;
    fn csmGetDrawableVertexUvs(m: *const CsmModel) -> *const *const Vec2;
    fn csmGetDrawableIndexCounts(m: *const CsmModel) -> *const c_int;
    fn csmGetDrawableIndices(m: *const CsmModel) -> *const *const c_ushort;
    fn csmGetDrawableMultiplyColors(m: *const CsmModel) -> *const Vec4;
    fn csmGetDrawableScreenColors(m: *const CsmModel) -> *const Vec4;
}

/// 정렬된 메모리 (Core가 그 안에 객체를 만든다).
struct Block {
    ptr: *mut u8,
    layout: Layout,
}

impl Block {
    fn zeroed(size: usize, align: usize) -> Result<Self> {
        ensure!(size > 0, "empty block");
        let layout = Layout::from_size_align(size, align)?;
        // SAFETY: layout의 크기는 0이 아니다.
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        Ok(Self { ptr, layout })
    }
}

impl Drop for Block {
    fn drop(&mut self) {
        // SAFETY: zeroed가 같은 layout으로 할당했다.
        unsafe { std::alloc::dealloc(self.ptr, self.layout) };
    }
}

/// 캔버스: 모델 단위 → 캔버스 px = origin + (x, −y) × pixels_per_unit (y가 아래로 커짐).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Canvas {
    pub size: [f32; 2],
    pub origin: [f32; 2],
    pub pixels_per_unit: f32,
}

/// drawable 하나의 현재 상태 (`Model::update` 뒤).
#[derive(Debug, Clone, Copy)]
pub struct Drawable<'a> {
    pub flags: u8,
    pub visible: bool,
    pub texture: i32,
    pub render_order: i32,
    pub opacity: f32,
    /// 마스크 drawable 번호.
    pub masks: &'a [i32],
    /// 모델 단위, y가 위로 커짐.
    pub positions: &'a [Vec2],
    /// 왼쪽 아래 원점.
    pub uvs: &'a [Vec2],
    pub indices: &'a [u16],
    pub multiply: Vec4,
    pub screen: Vec4,
}

/// moc 하나 + 모델 인스턴스 하나.
pub struct Model {
    model: *mut CsmModel,
    pub parameter_ids: Vec<String>,
    /// 파라미터 기본값 (`parameter_ids` 순서).
    pub parameter_defaults: Vec<f32>,
    /// 파라미터 범위 (`parameter_ids` 순서).
    pub parameter_ranges: Vec<ParamRange>,
    pub part_ids: Vec<String>,
    /// 모델을 만들었을 때의 파츠 불투명도.
    pub part_defaults: Vec<f32>,
    pub drawable_ids: Vec<String>,
    pub canvas: Canvas,
    // model은 moc 메모리를 가리키므로 moc보다 먼저 버린다(필드 순서).
    _model_mem: Block,
    _moc_mem: Block,
}

/// SAFETY: `n`개의 유효한 C 문자열 포인터 배열.
unsafe fn ids(p: *const *const c_char, n: c_int) -> Vec<String> {
    (0..n.max(0) as usize)
        .map(|i| {
            unsafe { CStr::from_ptr(*p.add(i)) }
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// SAFETY: `p`가 `n`개 이상의 원소를 가리킨다(`n` ≤ 0이면 읽지 않음).
unsafe fn slice<'a, T>(p: *const T, n: c_int) -> &'a [T] {
    if n <= 0 || p.is_null() {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(p, n as usize) }
    }
}

impl Model {
    /// `.moc3` 바이트로 모델을 만든다. 형식·일관성 검사에 실패하면 오류.
    pub fn new(moc: &[u8]) -> Result<Self> {
        let len = c_uint::try_from(moc.len())?;
        let moc_mem = Block::zeroed(moc.len(), MOC_ALIGN)?;
        // SAFETY: moc_mem은 moc.len() 바이트이고 겹치지 않는다. 이후 Core 호출은 모두 그 범위 안을 읽는다.
        unsafe {
            std::ptr::copy_nonoverlapping(moc.as_ptr(), moc_mem.ptr, moc.len());
            let version = csmGetMocVersion(moc_mem.ptr as _, len);
            let latest = csmGetLatestMocVersion();
            ensure!(
                version != 0 && version <= latest,
                "unsupported .moc3 version {version} (this runtime reads up to {latest})"
            );
            if csmHasMocConsistency(moc_mem.ptr as _, len) == 0 {
                bail!(".moc3 file is corrupt");
            }
            let moc = csmReviveMocInPlace(moc_mem.ptr as _, len);
            ensure!(!moc.is_null(), "failed to read .moc3");
            let size = csmGetSizeofModel(moc) as usize;
            let model_mem = Block::zeroed(size, MODEL_ALIGN)?;
            let model = csmInitializeModelInPlace(moc, model_mem.ptr as _, size as c_uint);
            ensure!(!model.is_null(), "failed to initialize the Live2D model");
            let param_count = csmGetParameterCount(model);
            let parameter_ids = ids(csmGetParameterIds(model), param_count);
            let parameter_defaults =
                slice(csmGetParameterDefaultValues(model), param_count).to_vec();
            let minimum = slice(csmGetParameterMinimumValues(model), param_count);
            let maximum = slice(csmGetParameterMaximumValues(model), param_count);
            let parameter_ranges = (0..parameter_defaults.len())
                .map(|i| ParamRange {
                    minimum: minimum[i],
                    maximum: maximum[i],
                })
                .collect();
            let part_count = csmGetPartCount(model);
            let part_ids = ids(csmGetPartIds(model), part_count);
            let part_defaults = slice(csmGetPartOpacities(model), part_count).to_vec();
            let drawable_ids = ids(csmGetDrawableIds(model), csmGetDrawableCount(model));
            let (mut size, mut origin, mut ppu) = (Vec2::default(), Vec2::default(), 0.0);
            csmReadCanvasInfo(model, &mut size, &mut origin, &mut ppu);
            ensure!(ppu > 0.0, "Live2D model has an invalid canvas");
            let mut m = Self {
                model,
                parameter_ids,
                parameter_defaults,
                parameter_ranges,
                part_ids,
                part_defaults,
                drawable_ids,
                canvas: Canvas {
                    size: [size.x, size.y],
                    origin: [origin.x, origin.y],
                    pixels_per_unit: ppu,
                },
                _model_mem: model_mem,
                _moc_mem: moc_mem,
            };
            m.update();
            Ok(m)
        }
    }

    /// 파라미터 값 (`parameter_ids` 순서). 바꾼 뒤 `update`.
    pub fn parameters(&mut self) -> &mut [f32] {
        // SAFETY: Core가 parameter_ids.len()개 배열을 돌려준다.
        unsafe {
            std::slice::from_raw_parts_mut(
                csmGetParameterValues(self.model),
                self.parameter_ids.len(),
            )
        }
    }

    /// 파츠 불투명도 (`part_ids` 순서). 바꾼 뒤 `update`.
    pub fn part_opacities(&mut self) -> &mut [f32] {
        // SAFETY: Core가 part_ids.len()개 배열을 돌려준다.
        unsafe {
            std::slice::from_raw_parts_mut(csmGetPartOpacities(self.model), self.part_ids.len())
        }
    }

    /// 파라미터·파츠 값으로 메시를 다시 계산한다.
    pub fn update(&mut self) {
        // SAFETY: model은 살아 있는 인스턴스.
        unsafe {
            csmResetDrawableDynamicFlags(self.model);
            csmUpdateModel(self.model);
        }
    }

    /// drawable 목록 (`drawable_ids` 순서). 마스크 번호도 이 순서를 가리킨다.
    pub fn drawables(&self) -> Vec<Drawable<'_>> {
        let m = self.model;
        let n = self.drawable_ids.len() as c_int;
        // SAFETY: 모든 배열은 drawable 수만큼이고, 원소별 배열은 그 개수만큼이다(Core ABI). update 전까지 유효.
        unsafe {
            let flags = slice(csmGetDrawableConstantFlags(m), n);
            let dynamic = slice(csmGetDrawableDynamicFlags(m), n);
            let texture = slice(csmGetDrawableTextureIndices(m), n);
            let order = slice(csmGetRenderOrders(m), n);
            let opacity = slice(csmGetDrawableOpacities(m), n);
            let mask_counts = slice(csmGetDrawableMaskCounts(m), n);
            let masks = slice(csmGetDrawableMasks(m), n);
            let vertex_counts = slice(csmGetDrawableVertexCounts(m), n);
            let positions = slice(csmGetDrawableVertexPositions(m), n);
            let uvs = slice(csmGetDrawableVertexUvs(m), n);
            let index_counts = slice(csmGetDrawableIndexCounts(m), n);
            let indices = slice(csmGetDrawableIndices(m), n);
            let multiply = slice(csmGetDrawableMultiplyColors(m), n);
            let screen = slice(csmGetDrawableScreenColors(m), n);
            (0..n as usize)
                .map(|i| Drawable {
                    flags: flags[i],
                    visible: dynamic[i] & IS_VISIBLE != 0,
                    texture: texture[i],
                    render_order: order[i],
                    opacity: opacity[i],
                    masks: slice(masks[i], mask_counts[i]),
                    positions: slice(positions[i], vertex_counts[i]),
                    uvs: slice(uvs[i], vertex_counts[i]),
                    indices: slice(indices[i], index_counts[i]),
                    multiply: multiply[i],
                    screen: screen[i],
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_garbage() {
        let e = Model::new(&[0u8; 256]).err().expect("garbage must fail");
        assert!(format!("{e:#}").contains("moc3"), "{e:#}");
    }
}
