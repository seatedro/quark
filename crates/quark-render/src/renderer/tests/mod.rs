//! Renderer tests: pixel probes of headless renders, allocation budgets,
//! and the flattening and batching they rest on.

mod atlas;
mod blur;
mod chunk;
mod compositing;
mod mask;
mod pattern;
mod perf;
mod render;
mod svg_text;
mod text_diff;
mod text_fill;

use super::encode::scissor_rect;
use super::flatten::{DrawStep, PrimKind, flatten_scene};
use super::gpu_context::GpuContext;
use super::images::{ImageCache, linear_to_srgb_byte, premultiply};
use super::surface::transparent_alpha_mode;
use super::text_runs::FrameText;
use super::*;
use crate::scene::{ClipPrimitive, Rect, RichTextPrimitive, Scene, TextPrimitive, Transform2D};
use std::sync::Arc;
