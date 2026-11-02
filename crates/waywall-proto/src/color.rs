use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// `"#rrggbb"` 또는 `"#rrggbbaa"` 형식의 straight-alpha sRGB 색.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

#[derive(Debug, thiserror::Error)]
#[error("invalid color {0:?}: expected \"#rrggbb\" or \"#rrggbbaa\"")]
pub struct ParseColorError(String);

impl Color {
    /// wl_shm ARGB8888(리틀 엔디언 바이트 순서 B, G, R, A), premultiplied.
    pub fn to_argb8888_premultiplied(self) -> [u8; 4] {
        let pm = |c: u8| ((c as u16 * self.a as u16 + 127) / 255) as u8;
        [pm(self.b), pm(self.g), pm(self.r), self.a]
    }

    /// wp_single_pixel_buffer_v1용 premultiplied 32비트 채널 값.
    pub fn to_u32_premultiplied(self) -> [u32; 4] {
        let a = self.a as f64 / 255.0;
        let ch = |c: u8| ((c as f64 / 255.0) * a * u32::MAX as f64).round() as u32;
        [
            ch(self.r),
            ch(self.g),
            ch(self.b),
            (a * u32::MAX as f64).round() as u32,
        ]
    }

    /// 선형 RGB(sRGB 전달 함수를 푼 값), premultiplied. 씬 HDR 버퍼용.
    pub fn to_linear_premultiplied(self) -> [f64; 4] {
        let a = self.a as f64 / 255.0;
        let ch = |c: u8| srgb_to_linear(c as f64 / 255.0) * a;
        [ch(self.r), ch(self.g), ch(self.b), a]
    }
}

/// sRGB 전달 함수의 역 (IEC 61966-2-1).
pub fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// sRGB 전달 함수 (IEC 61966-2-1).
pub fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

impl FromStr for Color {
    type Err = ParseColorError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseColorError(s.to_owned());
        let hex = s.strip_prefix('#').ok_or_else(err)?;
        if !(hex.len() == 6 || hex.len() == 8) || !hex.is_ascii() {
            return Err(err());
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| err());
        Ok(Color {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
            a: if hex.len() == 8 { byte(6)? } else { u8::MAX },
        })
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)?;
        if self.a != u8::MAX {
            write!(f, "{:02x}", self.a)?;
        }
        Ok(())
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display() {
        let c: Color = "#102030".parse().unwrap();
        assert_eq!(
            c,
            Color {
                r: 0x10,
                g: 0x20,
                b: 0x30,
                a: 0xff
            }
        );
        assert_eq!(c.to_string(), "#102030");
        let c: Color = "#10203080".parse().unwrap();
        assert_eq!(c.a, 0x80);
        assert_eq!(c.to_string(), "#10203080");
        assert!("102030".parse::<Color>().is_err());
        assert!("#1020".parse::<Color>().is_err());
        assert!("#gg2030".parse::<Color>().is_err());
    }

    #[test]
    fn premultiply() {
        let c: Color = "#ff000080".parse().unwrap();
        assert_eq!(c.to_argb8888_premultiplied(), [0, 0, 0x80, 0x80]);
    }
}
