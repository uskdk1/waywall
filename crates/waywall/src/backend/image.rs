//! 정적 이미지 디코딩. Wayland 버퍼 처리는 `wayland::output`이 한다.
//!
//! 디코딩/리사이즈는 메인 루프를 막지 않도록 별도 스레드에서 하고, 결과를 calloop 채널로 보낸다.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageReader};
use waywall_proto::config::{Fit, ResizeFilter};

use crate::fit::downscale_size;

#[derive(Debug, Clone)]
pub struct Request {
    pub path: PathBuf,
    /// 출력의 물리 픽셀 크기.
    pub target: (u32, u32),
    pub fit: Fit,
    pub downscale: Option<ResizeFilter>,
}

/// wl_shm ARGB8888(premultiplied) 픽셀.
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub argb: Vec<u8>,
}

impl std::fmt::Debug for Decoded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Decoded({}x{})", self.width, self.height)
    }
}

pub fn spawn<F>(req: Request, done: F)
where
    F: FnOnce(Result<Decoded>) + Send + 'static,
{
    std::thread::Builder::new()
        .name("waywall-image".into())
        .spawn(move || done(decode(&req)))
        .expect("failed to spawn image decoder thread");
}

pub fn decode(req: &Request) -> Result<Decoded> {
    let started = std::time::Instant::now();
    let mut img = open(&req.path)?;
    let src = (img.width(), img.height());
    if let Some(filter) = req.downscale
        && let Some((w, h)) = downscale_size(req.fit, src, req.target)
    {
        img = img.resize_exact(w, h, filter_type(filter));
    }
    let rgba = img.into_rgba8();
    let (width, height) = rgba.dimensions();
    let mut argb = rgba.into_raw();
    for px in argb.as_chunks_mut::<4>().0 {
        let [r, g, b, a] = [px[0], px[1], px[2], px[3]];
        let pm = |c: u8| ((c as u16 * a as u16 + 127) / 255) as u8;
        px.copy_from_slice(&[pm(b), pm(g), pm(r), a]);
    }
    tracing::debug!(
        path = %req.path.display(),
        ?src,
        decoded = ?(width, height),
        elapsed = ?started.elapsed(),
        "image decoded"
    );
    Ok(Decoded {
        width,
        height,
        argb,
    })
}

fn open(path: &Path) -> Result<DynamicImage> {
    let ctx = || format!("failed to decode {}", path.display());
    let mut decoder = ImageReader::open(path)
        .with_context(ctx)?
        .with_guessed_format()
        .with_context(ctx)?
        .into_decoder()
        .with_context(ctx)?;
    let orientation = decoder.orientation().with_context(ctx)?;
    let mut img = DynamicImage::from_decoder(decoder).with_context(ctx)?;
    img.apply_orientation(orientation);
    Ok(img)
}

fn filter_type(f: ResizeFilter) -> FilterType {
    match f {
        ResizeFilter::Nearest => FilterType::Nearest,
        ResizeFilter::Triangle => FilterType::Triangle,
        ResizeFilter::CatmullRom => FilterType::CatmullRom,
        ResizeFilter::Gaussian => FilterType::Gaussian,
        ResizeFilter::Lanczos3 => FilterType::Lanczos3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_downscale_premultiply() {
        let dir = std::env::temp_dir().join(format!("waywall-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.png");
        let img = image::RgbaImage::from_pixel(400, 200, image::Rgba([255, 0, 0, 128]));
        img.save(&path).unwrap();

        let req = Request {
            path: path.clone(),
            target: (100, 100),
            fit: Fit::Contain,
            downscale: Some(ResizeFilter::Triangle),
        };
        let d = decode(&req).unwrap();
        assert_eq!((d.width, d.height), (100, 50));
        assert_eq!(&d.argb[..4], &[0, 0, 128, 128]);

        let d = decode(&Request {
            downscale: None,
            ..req
        })
        .unwrap();
        assert_eq!((d.width, d.height), (400, 200));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
