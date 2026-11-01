//! `cursor.shape` 해석.

use std::str::FromStr;

use anyhow::{Result, anyhow};
use smithay_client_toolkit::seat::pointer::CursorIcon;

const HIDDEN: &str = "hidden";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Hidden,
    Icon(CursorIcon),
}

pub fn parse_shape(s: &str) -> Result<CursorShape> {
    if s == HIDDEN {
        return Ok(CursorShape::Hidden);
    }
    CursorIcon::from_str(s)
        .map(CursorShape::Icon)
        .map_err(|_| anyhow!("unknown cursor shape {s:?} (CSS cursor name or \"{HIDDEN}\")"))
}

pub fn validate_shape(s: &str) -> Result<()> {
    parse_shape(s).map(|_| ())
}
