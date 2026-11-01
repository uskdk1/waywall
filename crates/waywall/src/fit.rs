//! `fit` 모드 → wp_viewporter source/destination 계산. image와 video가 공유한다.

use waywall_proto::config::Fit;

/// 서브서피스 배치 (논리 좌표) + 버퍼에서 잘라낼 영역 (버퍼 픽셀).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub source: (f64, f64, f64, f64),
    pub position: (i32, i32),
    pub size: (i32, i32),
}

pub fn place(fit: Fit, buffer: (u32, u32), output: (u32, u32)) -> Placement {
    let (bw, bh) = (buffer.0.max(1) as f64, buffer.1.max(1) as f64);
    let (ow, oh) = (output.0.max(1) as f64, output.1.max(1) as f64);
    let full = (0.0, 0.0, bw, bh);
    match fit {
        Fit::Fill => Placement {
            source: full,
            position: (0, 0),
            size: (ow as i32, oh as i32),
        },
        Fit::Cover => {
            let s = (ow / bw).max(oh / bh);
            let (sw, sh) = ((ow / s).min(bw), (oh / s).min(bh));
            Placement {
                source: ((bw - sw) / 2.0, (bh - sh) / 2.0, sw, sh),
                position: (0, 0),
                size: (ow as i32, oh as i32),
            }
        }
        Fit::Contain => {
            let s = (ow / bw).min(oh / bh);
            let (dw, dh) = ((bw * s).round().max(1.0), (bh * s).round().max(1.0));
            Placement {
                source: full,
                position: (
                    ((ow - dw) / 2.0).round() as i32,
                    ((oh - dh) / 2.0).round() as i32,
                ),
                size: (dw as i32, dh as i32),
            }
        }
    }
}

/// 화면에 보일 픽셀보다 큰 이미지를 미리 줄일 크기. 줄일 필요가 없으면 `None`.
pub fn downscale_size(fit: Fit, src: (u32, u32), target: (u32, u32)) -> Option<(u32, u32)> {
    let (sw, sh) = (src.0 as f64, src.1 as f64);
    let (tw, th) = (target.0 as f64, target.1 as f64);
    let size = match fit {
        Fit::Fill => (src.0.min(target.0), src.1.min(target.1)),
        Fit::Cover | Fit::Contain => {
            let s = if fit == Fit::Cover {
                (tw / sw).max(th / sh)
            } else {
                (tw / sw).min(th / sh)
            };
            if s >= 1.0 {
                return None;
            }
            (
                ((sw * s).ceil() as u32).max(1),
                ((sh * s).ceil() as u32).max(1),
            )
        }
    };
    (size != src).then_some(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_crops_center() {
        // 4:3 이미지를 16:9 화면에: 위아래를 자른다.
        let p = place(Fit::Cover, (1600, 1200), (1920, 1080));
        assert_eq!(p.size, (1920, 1080));
        assert_eq!(p.position, (0, 0));
        let (x, y, w, h) = p.source;
        assert_eq!((x, w), (0.0, 1600.0));
        assert!((h - 900.0).abs() < 1e-9 && (y - 150.0).abs() < 1e-9);
    }

    #[test]
    fn contain_letterboxes() {
        let p = place(Fit::Contain, (1600, 1200), (1920, 1080));
        assert_eq!(p.size, (1440, 1080));
        assert_eq!(p.position, (240, 0));
        assert_eq!(p.source, (0.0, 0.0, 1600.0, 1200.0));
    }

    #[test]
    fn fill_stretches() {
        let p = place(Fit::Fill, (100, 100), (1920, 1080));
        assert_eq!(p.size, (1920, 1080));
        assert_eq!(p.source, (0.0, 0.0, 100.0, 100.0));
    }

    #[test]
    fn downscale() {
        assert_eq!(
            downscale_size(Fit::Cover, (7680, 4320), (1920, 1080)),
            Some((1920, 1080))
        );
        assert_eq!(downscale_size(Fit::Cover, (1000, 1000), (1920, 1080)), None);
        // cover는 짧은 변 기준이라 4:3 이미지는 가로가 화면 폭에 맞는다.
        assert_eq!(
            downscale_size(Fit::Cover, (4000, 3000), (1920, 1080)),
            Some((1920, 1440))
        );
        assert_eq!(
            downscale_size(Fit::Contain, (4000, 3000), (1920, 1080)),
            Some((1440, 1080))
        );
        assert_eq!(
            downscale_size(Fit::Fill, (4000, 500), (1920, 1080)),
            Some((1920, 500))
        );
    }
}
