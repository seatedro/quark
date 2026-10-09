use quark::view;

use quark_ui::Action;
use quark_ui::design::Sp;
use quark_ui::element::{
    AnyElement, ElementContext, IntoAnyElement, RenderOnce, div, svg_icon, text,
};
use quark_ui::icons::lucide;
use quark_ui::style::Styled;

pub struct Breadcrumb {
    segments: Vec<String>,
    on_click_segment: Option<Box<dyn Fn(usize) -> Action>>,
}

pub fn breadcrumb(segments: impl IntoIterator<Item = impl Into<String>>) -> Breadcrumb {
    Breadcrumb {
        segments: segments.into_iter().map(|s| s.into()).collect(),
        on_click_segment: None,
    }
}

impl Breadcrumb {
    pub fn on_click_segment(mut self, f: impl Fn(usize) -> Action + 'static) -> Self {
        self.on_click_segment = Some(Box::new(f));
        self
    }
}

impl RenderOnce for Breadcrumb {
    fn render(self, cx: &ElementContext) -> AnyElement {
        let tc = &cx.theme.colors;
        let m = &cx.theme.metrics;
        let scale = m.ui_scale();
        let icon_size = (m.ui_small_font_size - Sp::XXS * scale).max(Sp::SM * scale);
        let last = self.segments.len().saturating_sub(1);

        view! { scale,
            <div class="flex-row items-center" gap={m.spacing_xs}>
                for (i, segment) in self.segments.into_iter().enumerate() {
                    <>
                        if i > 0 {
                            <icon
                                svg={lucide::CHEVRON_RIGHT}
                                size={icon_size}
                                color={tc.text_muted}
                            />
                        }
                        <div
                            px={m.spacing_xs}
                            py={Sp::XXS}
                            rounded={m.control_radius - Sp::XS * scale}
                            @when {i != last && self.on_click_segment.is_some()} {
                                on:click={(self.on_click_segment.as_ref().unwrap())(i)}
                            }
                            @when {i != last && self.on_click_segment.is_some()} {
                                id={format!("breadcrumb:{i}:{segment}")}
                                key={segment.clone()}
                                test-id="breadcrumb-segment"
                                role="button"
                                accessibility_id={format!("breadcrumb:{i}:{segment}")}
                                aria-label={segment.clone()}
                            }
                            @when {i != last} { hover_bg={tc.ghost_element_hover} }
                        >
                            <text
                                class="text-sm"
                                color={if i == last {
                                    tc.text_strong
                                } else {
                                    tc.text_muted
                                }}
                                @when {i == last} { medium }
                            >
                                {segment}
                            </text>
                        </div>
                    </>
                }
            </div>
        }
    }
}
