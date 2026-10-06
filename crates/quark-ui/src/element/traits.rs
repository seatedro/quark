use super::*;

// ---------------------------------------------------------------------------
// Element trait
// ---------------------------------------------------------------------------

/// Every UI node implements `Element`. The lifecycle is:
///
/// 1. **request_layout** — declare your Taffy style and children. Returns a
///    `LayoutId` and arbitrary per-element state.
/// 2. **prepaint** — given resolved bounds, register hitboxes and resolve
///    interaction state. Returns arbitrary prepaint state.
/// 3. **paint** — emit scene primitives using resolved bounds and prepaint state.
pub trait Element: 'static {
    type LayoutState: 'static;
    type PrepaintState: 'static;

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, Self::LayoutState);

    fn prepaint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        engine: &LayoutEngine,
        cx: &mut ElementContext,
    ) -> Self::PrepaintState;

    fn paint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        prepaint_state: &mut Self::PrepaintState,
        engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    );
}

// ---------------------------------------------------------------------------
// AnyElement — type-erased element
// ---------------------------------------------------------------------------

pub struct AnyElement {
    inner: Box<dyn AnyElementImpl>,
}

impl AnyElement {
    pub fn new<E: Element>(element: E) -> Self {
        Self {
            inner: Box::new(ElementHolder {
                element,
                layout_state: None,
                prepaint_state: None,
                layout_id: None,
            }),
        }
    }

    pub fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> LayoutId {
        self.inner.request_layout(engine, cx)
    }

    pub fn prepaint(&mut self, engine: &LayoutEngine, cx: &mut ElementContext) {
        self.inner.prepaint(engine, cx, 0.0, 0.0);
    }

    pub fn prepaint_with_offset(
        &mut self,
        engine: &LayoutEngine,
        cx: &mut ElementContext,
        offset_x: f32,
        offset_y: f32,
    ) {
        self.inner.prepaint(engine, cx, offset_x, offset_y);
    }

    pub fn paint(&mut self, engine: &LayoutEngine, scene: &mut Scene, cx: &mut ElementContext) {
        self.inner.paint(engine, scene, cx, 0.0, 0.0);
    }

    pub fn paint_with_offset(
        &mut self,
        engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
        offset_x: f32,
        offset_y: f32,
    ) {
        self.inner.paint(engine, scene, cx, offset_x, offset_y);
    }
}

trait AnyElementImpl {
    fn request_layout(&mut self, engine: &mut LayoutEngine, cx: &mut ElementContext) -> LayoutId;
    fn prepaint(
        &mut self,
        engine: &LayoutEngine,
        cx: &mut ElementContext,
        offset_x: f32,
        offset_y: f32,
    );
    fn paint(
        &mut self,
        engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
        offset_x: f32,
        offset_y: f32,
    );
}

struct ElementHolder<E: Element> {
    element: E,
    layout_state: Option<E::LayoutState>,
    prepaint_state: Option<E::PrepaintState>,
    layout_id: Option<LayoutId>,
}

impl<E: Element> AnyElementImpl for ElementHolder<E> {
    fn request_layout(&mut self, engine: &mut LayoutEngine, cx: &mut ElementContext) -> LayoutId {
        let (id, state) = self.element.request_layout(engine, cx);
        self.layout_id = Some(id);
        self.layout_state = Some(state);
        id
    }

    fn prepaint(
        &mut self,
        engine: &LayoutEngine,
        cx: &mut ElementContext,
        offset_x: f32,
        offset_y: f32,
    ) {
        let id = self
            .layout_id
            .expect("prepaint called before request_layout");
        let mut bounds = engine.layout_bounds(id);
        let (base_offset_x, base_offset_y) = cx.current_element_offset();
        let total_offset_x = base_offset_x + offset_x;
        let total_offset_y = base_offset_y + offset_y;
        bounds.x += total_offset_x;
        bounds.y += total_offset_y;
        let layout_state = self
            .layout_state
            .as_mut()
            .expect("prepaint called before request_layout");
        cx.push_element_offset(offset_x, offset_y);
        let prepaint_state = self.element.prepaint(bounds, layout_state, engine, cx);
        self.prepaint_state = Some(prepaint_state);
        cx.pop_element_offset();
    }

    fn paint(
        &mut self,
        engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
        offset_x: f32,
        offset_y: f32,
    ) {
        let id = self.layout_id.expect("paint called before request_layout");
        let mut bounds = engine.layout_bounds(id);
        let (base_offset_x, base_offset_y) = cx.current_element_offset();
        let total_offset_x = base_offset_x + offset_x;
        let total_offset_y = base_offset_y + offset_y;
        bounds.x += total_offset_x;
        bounds.y += total_offset_y;
        let layout_state = self
            .layout_state
            .as_mut()
            .expect("paint called before request_layout");
        let prepaint_state = self
            .prepaint_state
            .as_mut()
            .expect("paint called before prepaint");
        cx.push_element_offset(offset_x, offset_y);
        self.element
            .paint(bounds, layout_state, prepaint_state, engine, scene, cx);
        cx.pop_element_offset();
    }
}

// ---------------------------------------------------------------------------
// IntoAnyElement — conversion trait
// ---------------------------------------------------------------------------

pub trait IntoAnyElement {
    fn into_any(self) -> AnyElement;
}

impl IntoAnyElement for AnyElement {
    fn into_any(self) -> AnyElement {
        self
    }
}

// ---------------------------------------------------------------------------
// RenderOnce — component-level trait
// ---------------------------------------------------------------------------

/// Components implement `RenderOnce` to produce a tree of elements.
/// The component is consumed (moved) when rendered.
pub trait RenderOnce: 'static + Sized {
    fn render(self, cx: &ElementContext) -> AnyElement;
}

/// Adapter that wraps a `RenderOnce` component into an `Element`.
struct ComponentElement<C: RenderOnce> {
    component: Option<C>,
    rendered: Option<AnyElement>,
}

impl<C: RenderOnce> Element for ComponentElement<C> {
    type LayoutState = ();
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, ()) {
        let component = self
            .component
            .take()
            .expect("ComponentElement rendered twice");
        let mut any = component.render(cx);
        let id = any.request_layout(engine, cx);
        self.rendered = Some(any);
        (id, ())
    }

    fn prepaint(
        &mut self,
        _bounds: Bounds,
        _layout_state: &mut (),
        engine: &LayoutEngine,
        cx: &mut ElementContext,
    ) -> () {
        if let Some(ref mut rendered) = self.rendered {
            rendered.prepaint(engine, cx);
        }
    }

    fn paint(
        &mut self,
        _bounds: Bounds,
        _layout_state: &mut (),
        _prepaint_state: &mut (),
        engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        if let Some(ref mut rendered) = self.rendered {
            rendered.paint(engine, scene, cx);
        }
    }
}

/// Blanket impl: any `RenderOnce` can be converted into an `AnyElement`.
impl<C: RenderOnce> IntoAnyElement for C {
    fn into_any(self) -> AnyElement {
        AnyElement::new(ComponentElement {
            component: Some(self),
            rendered: None,
        })
    }
}

/// Helper to wrap any `Element` implementor into an `AnyElement`.
/// Use this for types that implement `Element` directly (not `RenderOnce`).
pub(super) fn element_into_any<E: Element>(element: E) -> AnyElement {
    AnyElement::new(element)
}
