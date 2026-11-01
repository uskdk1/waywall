//! `zwp_linux_dmabuf_v1` (stable): 컴포지터가 받는 포맷/모디파이어 목록과 영상 버퍼 release
//! (docs/protocols.md, docs/architecture.md#영상-복사-없는-dmabuf-경로).

use smithay_client_toolkit::dmabuf::{DmabufFeedback, DmabufHandler, DmabufState};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::{Connection, QueueHandle};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
    zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
};

use crate::daemon::State;

/// DRM_FORMAT_MOD_INVALID: 암시적 모디파이어. 디코더 출력과 맞출 수 없으므로 쓰지 않는다.
const MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

/// fourcc + modifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DrmFormat {
    pub fourcc: u32,
    pub modifier: u64,
}

impl DrmFormat {
    /// GStreamer `drm-format` 문자열 (`NV12`, `NV12:0x0100000000000002`).
    pub fn to_gst(self) -> String {
        let fourcc: String = self
            .fourcc
            .to_le_bytes()
            .iter()
            .map(|&b| char::from(b))
            .collect::<String>()
            .trim_end()
            .to_owned();
        if self.modifier == 0 {
            fourcc
        } else {
            format!("{fourcc}:0x{:016x}", self.modifier)
        }
    }
}

/// 선호 순서(tranche 순)대로, 중복 없이.
fn formats_from_feedback(feedback: &DmabufFeedback) -> Vec<DrmFormat> {
    let table = feedback.format_table();
    let mut out: Vec<DrmFormat> = Vec::new();
    for tranche in feedback.tranches() {
        for &i in &tranche.formats {
            let Some(f) = table.get(usize::from(i)) else {
                continue;
            };
            let f = DrmFormat {
                fourcc: f.format,
                modifier: f.modifier,
            };
            if f.modifier != MOD_INVALID && !out.contains(&f) {
                out.push(f);
            }
        }
    }
    out
}

impl State {
    /// 영상 appsink caps에 넣을 포맷. v4 feedback이 없으면 v3 modifier 이벤트 목록.
    pub fn dmabuf_formats(&self) -> Vec<DrmFormat> {
        if !self.dmabuf_formats.is_empty() {
            return self.dmabuf_formats.clone();
        }
        self.dmabuf
            .modifiers()
            .iter()
            .filter(|f| f.modifier != MOD_INVALID)
            .map(|f| DrmFormat {
                fourcc: f.format,
                modifier: f.modifier,
            })
            .collect()
    }
}

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf
    }

    fn dmabuf_feedback(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ZwpLinuxDmabufFeedbackV1,
        feedback: DmabufFeedback,
    ) {
        self.dmabuf_formats = formats_from_feedback(&feedback);
        tracing::debug!(formats = self.dmabuf_formats.len(), "dmabuf feedback");
    }

    fn created(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ZwpLinuxBufferParamsV1,
        _: WlBuffer,
    ) {
    }

    fn failed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ZwpLinuxBufferParamsV1) {
        tracing::warn!("compositor rejected a dmabuf buffer");
    }

    fn released(&mut self, _: &Connection, _: &QueueHandle<Self>, buffer: &WlBuffer) {
        self.video_buffer_released(buffer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gst_drm_format_strings() {
        let nv12 = u32::from_le_bytes(*b"NV12");
        assert_eq!(
            DrmFormat {
                fourcc: nv12,
                modifier: 0
            }
            .to_gst(),
            "NV12"
        );
        assert_eq!(
            DrmFormat {
                fourcc: nv12,
                modifier: 0x0100_0000_0000_0002
            }
            .to_gst(),
            "NV12:0x0100000000000002"
        );
        // 3글자 fourcc는 공백으로 채워져 있다.
        let r8 = u32::from_le_bytes(*b"R8  ");
        assert_eq!(
            DrmFormat {
                fourcc: r8,
                modifier: 0
            }
            .to_gst(),
            "R8"
        );
    }
}
