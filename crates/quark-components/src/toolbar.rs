use quark::view;

use quark_ui::design::Sp;
use quark_ui::element::*;
use quark_ui::style::Styled;

pub struct Toolbar {
    left: Vec<AnyElement>,
    right: Vec<AnyElement>,
}

impl Default for Toolbar {
    fn default() -> Self {
        Self::new()
    }
}

impl Toolbar {
    pub fn new() -> Self {
        Self {
            left: Vec::new(),
            right: Vec::new(),
        }
    }

    pub fn left_child(mut self, child: impl IntoAnyElement) -> Self {
        self.left.push(child.into_any());
        self
    }

    pub fn right_child(mut self, child: impl IntoAnyElement) -> Self {
        self.right.push(child.into_any());
        self
    }
}

impl RenderOnce for Toolbar {
    fn render(self, cx: &ElementContext) -> AnyElement {
        let tc = &cx.theme.colors;
        let scale = cx.theme.metrics.ui_scale();

        view! { scale,
            <div class="w-full flex-row items-center" h={cx.theme.metrics.ui_row_height} px={Sp::MD}
                 border_b={tc.border_variant}>
                <div class="flex-row items-center" gap_1>{...self.left}</div>
                <spacer />
                <div class="flex-row items-center" gap_1>{...self.right}</div>
            </div>
        }
    }
}
