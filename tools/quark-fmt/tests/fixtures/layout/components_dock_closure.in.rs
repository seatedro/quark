// crates/quark-components/src/dock.rs, line 2097
fn excerpt() {
    view! {
        <div class="flex-none relative" bg={colors.border_variant}
             @when {horizontal} { w={DIVIDER_THICKNESS} class="h-full" }
             @when {!horizontal} { h={DIVIDER_THICKNESS} class="w-full" }>
            <div class="absolute" z_index={1}
                 accessibility_id={format!("dock:split:{}:{divider}", id.0)}
                 accessibility_role={Role::Splitter} role="separator"
                 aria-label={quark_ui::i18n::tr_args(
                     "quark-resize-named",
                     [("name", self.area_label(region).into())],
                 )}
                 aria-valuetext={format!("{at:.0}")}
                 accessibility_numeric={NumericValue {
                     value: f64::from(at),
                     min: f64::from(lo),
                     max: f64::from(hi),
                     step: Some(f64::from(NUDGE_STEP)),
                 }}
                 accessibility_numeric_actions={numeric_actions}
                 // A line between side by side groups stands upright.
                 accessibility_orientation={if horizontal {
                     Orientation::Vertical
                 } else {
                     Orientation::Horizontal
                 }}
                 focus_ring={Self::divider_focus(id, divider)}
                 on_key={(back, nudge(-NUDGE_STEP))}
                 on_key={(forward, nudge(NUDGE_STEP))}
                 on_key={(format!("shift+{back}"), nudge(-NUDGE_STEP_LARGE))}
                 on_key={(format!("shift+{forward}"), nudge(NUDGE_STEP_LARGE))}
                 // Home and End: the ends of the range it may move in.
                 on_key={("home", to(lo))} on_key={("end", to(hi))}
                 test_id="dock-pane-divider" cursor={cursor} hover_bg={colors.accent}
                 on:drag={move |press: ClickEvent| {
                     Box::new(PaneDividerDrag {
                         map: drag_map.clone(),
                         split: id,
                         divider,
                         horizontal,
                         origin: if horizontal { press.x } else { press.y },
                         extent,
                         cursor,
                     }) as Box<dyn DragHandler>
                 }}
                 @when {horizontal} { class="top-0 bottom-0" left={offset} w={grip} }
                 @when {!horizontal} { class="left-0 right-0" top={offset} h={grip} } />
        </div>
    }
}
