use super::*;

// ---------------------------------------------------------------------------
// SvgIcon — renders an SVG string as a rasterized image
// ---------------------------------------------------------------------------

pub struct SvgIcon {
    svg: &'static str,
    size: f32,
    color: Option<Color>,
}

/// `size` is a BASE (logical, pre-`ui_scale`) value — pass an `Ico::*` token or a base
/// pixel size. `SvgIcon` multiplies it by `ui_scale` internally so it matches scaled
/// text/spacing. Do NOT pass an already-scaled value like `theme.metrics.ui_small_font_size`
/// (which is post-scale) — that double-scales and renders jumbo. To match `text-sm`,
/// use `Ico::SM`, not `ui_small_font_size`.
pub fn svg_icon(svg: &'static str, size: f32) -> SvgIcon {
    SvgIcon {
        svg,
        size,
        color: None,
    }
}

impl SvgIcon {
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }

    pub fn size(mut self, s: f32) -> Self {
        self.size = s;
        self
    }
}

impl Element for SvgIcon {
    type LayoutState = ();
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, ()) {
        let scale = cx.theme.metrics.ui_scale();
        let effective = self.size * scale;
        let id = engine.request_layout(
            taffy::Style {
                size: taffy::Size {
                    width: taffy::Dimension::length(effective),
                    height: taffy::Dimension::length(effective),
                },
                flex_shrink: 0.0,
                ..Default::default()
            },
            &[],
        );
        (id, ())
    }

    fn prepaint(
        &mut self,
        _bounds: Bounds,
        _layout_state: &mut (),
        _engine: &LayoutEngine,
        _cx: &mut ElementContext,
    ) {
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut (),
        _prepaint_state: &mut (),
        _engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        let color = cx
            .icon_color_override()
            .unwrap_or_else(|| self.color.unwrap_or(cx.theme.colors.icon));
        let scale = cx.theme.metrics.ui_scale();
        let px_size = (self.size * scale).ceil() as u32;
        let key = quark_render::icons::cache_key(self.svg, px_size, color);
        let (rgba, w, h) = quark_render::icons::rasterize_svg(self.svg, px_size, color);
        let snapped = Bounds {
            x: bounds.x.round(),
            y: bounds.y.round(),
            width: bounds.width.round(),
            height: bounds.height.round(),
        };
        scene.image(quark_render::ImagePrimitive {
            rect: snapped,
            width: w,
            height: h,
            rgba,
            cache_key: key,
        });
    }
}

impl IntoAnyElement for SvgIcon {
    fn into_any(self) -> AnyElement {
        element_into_any(self)
    }
}

// ---------------------------------------------------------------------------
// Raster image — pre-decoded RGBA bitmap sized to `size` x `size`.
// ---------------------------------------------------------------------------

pub struct RasterImage {
    rgba: std::sync::Arc<[u8]>,
    src_width: u32,
    src_height: u32,
    cache_key: u64,
    size: f32,
}

pub fn raster_image(
    rgba: std::sync::Arc<[u8]>,
    src_width: u32,
    src_height: u32,
    cache_key: u64,
    size: f32,
) -> RasterImage {
    RasterImage {
        rgba,
        src_width,
        src_height,
        cache_key,
        size,
    }
}

impl Element for RasterImage {
    type LayoutState = ();
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, ()) {
        let scale = cx.theme.metrics.ui_scale();
        let effective = self.size * scale;
        let id = engine.request_layout(
            taffy::Style {
                size: taffy::Size {
                    width: taffy::Dimension::length(effective),
                    height: taffy::Dimension::length(effective),
                },
                flex_shrink: 0.0,
                ..Default::default()
            },
            &[],
        );
        (id, ())
    }

    fn prepaint(
        &mut self,
        _bounds: Bounds,
        _layout_state: &mut (),
        _engine: &LayoutEngine,
        _cx: &mut ElementContext,
    ) {
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut (),
        _prepaint_state: &mut (),
        _engine: &LayoutEngine,
        scene: &mut Scene,
        _cx: &mut ElementContext,
    ) {
        let snapped = Bounds {
            x: bounds.x.round(),
            y: bounds.y.round(),
            width: bounds.width.round(),
            height: bounds.height.round(),
        };
        scene.image(quark_render::ImagePrimitive {
            rect: snapped,
            width: self.src_width,
            height: self.src_height,
            rgba: self.rgba.clone(),
            cache_key: self.cache_key,
        });
    }
}

impl IntoAnyElement for RasterImage {
    fn into_any(self) -> AnyElement {
        element_into_any(self)
    }
}
