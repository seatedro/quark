use quark::view;
use quark_render::Rect;
use quark_ui::Action;
use quark_ui::accessibility::Politeness;
use quark_ui::animation::{AnimKey, AnimationTable, Curve, Motion, PropId};
use quark_ui::design::{Alpha, Ico, Rad, Shadow, Sp, Sz};
use quark_ui::element::CursorHint;
use quark_ui::element::*;
use quark_ui::icons::lucide;
use quark_ui::style::Styled;
use quark_ui::theme::{Color, Theme, ThemeColors};

/// A notification shown by [`ToastStack`]. Keep a list yourself or use
/// [`ToastQueue`], which also expires, pauses, and retires toasts.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Toast {
    /// Stable identity, used to key entrance and progress animations.
    pub id: u64,
    pub kind: ToastKind,
    pub message: String,
    pub description: Option<String>,
    pub created_at_ms: u64,
    /// Hovered toasts pause their lifetime; set it with
    /// [`Toast::set_hovered`] so the pause is timed.
    pub hovered: bool,
    /// When `Some`, the toast renders an externally driven progress bar in
    /// place of the time-based one.
    pub progress: Option<f32>,
    /// Buttons shown beside the message.
    pub actions: Vec<ToastAction>,
    /// Visible lifetime, not counting pauses. `None` keeps the toast until
    /// it is dismissed.
    pub duration_ms: Option<u64>,
    /// Paused time before the current pause.
    pub paused_ms: u64,
    /// When the current pause began.
    pub paused_at_ms: Option<u64>,
}

/// A button on a toast.
#[derive(Debug, Clone, PartialEq)]
pub struct ToastAction {
    pub label: String,
    pub action: Action,
    /// The undo button, which the app's undo shortcut also presses.
    pub undo: bool,
}

impl Toast {
    /// A toast with the default lifetime. [`ToastQueue::push`] assigns the
    /// id and creation time.
    pub fn new(kind: ToastKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            duration_ms: Some(TOAST_LIFETIME_MS),
            ..Self::default()
        }
    }

    pub fn info(message: impl Into<String>) -> Self {
        Self::new(ToastKind::Info, message)
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::new(ToastKind::Error, message)
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Add a button that emits `action`.
    pub fn action(mut self, label: impl Into<String>, action: impl Into<Action>) -> Self {
        self.actions.push(ToastAction {
            label: label.into(),
            action: action.into(),
            undo: false,
        });
        self
    }

    /// Offer to undo what the toast reports: an Undo button that emits
    /// `action`, live for `timeout_ms`. After that the toast leaves and
    /// the undo can no longer run.
    pub fn undo(mut self, action: impl Into<Action>, timeout_ms: u64) -> Self {
        self.actions.push(ToastAction {
            label: "Undo".to_owned(),
            action: action.into(),
            undo: true,
        });
        self.duration_ms = Some(timeout_ms);
        self
    }

    pub fn duration(mut self, ms: u64) -> Self {
        self.duration_ms = Some(ms);
        self
    }

    /// Keep the toast until it is dismissed.
    pub fn sticky(mut self) -> Self {
        self.duration_ms = None;
        self
    }

    /// Time shown at `now_ms`, not counting pauses.
    pub fn elapsed_ms(&self, now_ms: u64) -> u64 {
        let end = self.paused_at_ms.unwrap_or(now_ms);
        end.saturating_sub(self.created_at_ms)
            .saturating_sub(self.paused_ms)
    }

    /// Lifetime left at `now_ms`; `None` for sticky toasts.
    pub fn remaining_ms(&self, now_ms: u64) -> Option<u64> {
        self.duration_ms
            .map(|duration| duration.saturating_sub(self.elapsed_ms(now_ms)))
    }

    /// Pause (hovered) or resume the lifetime at `now_ms`.
    pub fn set_hovered(&mut self, hovered: bool, now_ms: u64) {
        match (self.paused_at_ms, hovered) {
            (None, true) => self.paused_at_ms = Some(now_ms),
            (Some(since), false) => {
                self.paused_ms += now_ms.saturating_sub(since);
                self.paused_at_ms = None;
            }
            _ => {}
        }
        self.hovered = hovered;
    }

    /// Fraction of the lifetime used, 0 (fresh) to 1 (about to leave).
    fn lifetime_used(&self, now_ms: u64) -> f32 {
        match self.duration_ms {
            Some(duration) if duration > 0 => {
                (self.elapsed_ms(now_ms) as f32 / duration as f32).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToastKind {
    #[default]
    Info,
    Error,
}

/// Entrance slide, 0 (below its resting place) to 1 (resting).
const ENTRANCE: PropId = PropId(0);
/// Displayed external progress, eased toward [`Toast::progress`].
const PROGRESS: PropId = PropId(1);
/// Stack spread, 0 (collapsed) to 1 (fanned).
const FAN: PropId = PropId(2);
const ENTER_MS: u32 = 300;
const EXIT_MS: u32 = 200;
const FAN_MS: u32 = 250;
const PROGRESS_MS: u32 = 200;

fn toast_key(id: u64) -> AnimKey {
    AnimKey::from_str_key(&format!("quark.toast.{id}"))
}

fn stack_key() -> AnimKey {
    AnimKey::from_str_key("quark.toast-stack")
}

fn ease(duration_ms: u32) -> Motion {
    Motion::tween(duration_ms, Curve::EaseOutQuint)
}

/// Slide toast `id` in from below. Call once when the toast is shown.
pub fn animate_toast_in(table: &mut AnimationTable, id: u64, now_ms: u64) {
    let key = toast_key(id);
    table.set(key, ENTRANCE, 0.0, now_ms);
    table.animate_to(key, ENTRANCE, 1.0, ease(ENTER_MS), now_ms);
}

/// Slide toast `id` out. Keep it in the app's list until [`retire_toast`]
/// reports the exit finished.
pub fn animate_toast_out(table: &mut AnimationTable, id: u64, now_ms: u64) {
    let key = toast_key(id);
    if table.get(key, ENTRANCE).is_none() {
        table.set(key, ENTRANCE, 1.0, now_ms);
    }
    table.animate_to(key, ENTRANCE, 0.0, ease(EXIT_MS), now_ms);
}

/// Fan the stack out (hovered) or collapse it.
pub fn animate_toast_fan(table: &mut AnimationTable, fanned: bool, now_ms: u64) {
    let key = stack_key();
    if table.get(key, FAN).is_none() {
        table.set(key, FAN, 0.0, now_ms);
    }
    table.animate_to(
        key,
        FAN,
        if fanned { 1.0 } else { 0.0 },
        ease(FAN_MS),
        now_ms,
    );
}

/// Ease toast `id`'s progress bar toward `progress`. The first call snaps.
pub fn animate_toast_progress(table: &mut AnimationTable, id: u64, progress: f32, now_ms: u64) {
    table.animate_to(toast_key(id), PROGRESS, progress, ease(PROGRESS_MS), now_ms);
}

/// Remove toast `id`'s settled rows; the stack then draws it at rest.
/// Returns `true` once its exit has finished, when the app should drop it.
pub fn retire_toast(table: &mut AnimationTable, id: u64) -> bool {
    let key = toast_key(id);
    let exited = table.target(key, ENTRANCE) == Some(0.0) && !table.is_animating(key, ENTRANCE);
    for prop in [ENTRANCE, PROGRESS] {
        if table.get(key, prop).is_some() && !table.is_animating(key, prop) {
            table.remove(key, prop);
        }
    }
    exited
}

/// Sonner-style stacking constants (unscaled).
const STACK_PEEK: f32 = 10.0;
const MAX_VISIBLE_BEHIND: usize = 2;
const TOAST_Z_BASE: i32 = 300;
/// Vertical gap between toasts when fanned out.
const FAN_GAP: f32 = 10.0;
/// Lifetime the time-based progress bar counts down; apps should
/// auto-dismiss on the same schedule.
pub const TOAST_LIFETIME_MS: u64 = 5_000;

/// Wide enough for actionable command/error detail without dominating the app.
pub const TOAST_WIDTH: f32 = 460.0;
pub const BADGE_SIZE: f32 = 26.0;
pub const CLOSE_SIZE: f32 = 22.0;
const PROGRESS_H: f32 = 2.0;
/// Height of an action button.
const ACTION_H: f32 = 24.0;
const CORNER_RADIUS: f32 = Rad::XL;
/// Max wrapped lines for title and description.
pub const TITLE_MAX_LINES: usize = 2;
pub const DESC_MAX_LINES: usize = 5;
/// Vertical padding inside the toast (top and bottom).
const PAD_Y: f32 = 12.0;
/// Gap between title and description lines.
const DESC_GAP: f32 = 2.0;

/// Horizontal chrome (left pad, badge, gap, gap, close, right pad) — the
/// remaining width is available for wrapped text.
pub const CHROME_W: f32 = Sp::MD + BADGE_SIZE + Sp::MD + Sp::MD + CLOSE_SIZE + Sp::MD;

/// Width used by the stack after applying the current window constraints.
pub fn toast_stack_width(window_width: f32, ui_scale: f32) -> f32 {
    let side_margin = (Sp::XL * ui_scale).round();
    let available = (window_width - side_margin * 2.0).max(1.0);
    available
        .min(TOAST_WIDTH)
        .max(available.min(Sz::TOAST_MIN_W))
}

/// Inner content width available for wrapped title / description.
pub fn toast_inner_text_width(toast_width: f32) -> f32 {
    (toast_width - CHROME_W).max(80.0)
}

/// Laid-out per-toast dimensions, computed in the shell where the font system
/// is available for wrapping. Parallel to `Toasts`.
#[derive(Debug, Clone)]
pub struct ToastLayout {
    pub title_lines: Vec<String>,
    pub description_lines: Vec<String>,
    pub height: f32,
}

/// Compute total height for a wrapped title + optional description.
pub fn compute_toast_height(theme: &Theme, title_lines: usize, desc_lines: usize) -> f32 {
    let title_lh = line_height(theme.metrics.ui_small_font_size);
    let desc_lh = line_height(theme.metrics.ui_small_font_size - 1.0);
    let title_h = title_lines.max(1) as f32 * title_lh;
    let desc_h = if desc_lines == 0 {
        0.0
    } else {
        DESC_GAP + desc_lines as f32 * desc_lh
    };
    let content_h = title_h + desc_h;
    let min_h = BADGE_SIZE + PAD_Y * 2.0;
    (content_h + PAD_Y * 2.0).max(min_h)
}

fn line_height(font_size: f32) -> f32 {
    (font_size * 1.35).ceil()
}

fn severity_color(kind: ToastKind, tc: &ThemeColors) -> Color {
    match kind {
        ToastKind::Info => tc.status_info,
        ToastKind::Error => tc.status_error,
    }
}

fn severity_icon(kind: ToastKind) -> &'static str {
    match kind {
        ToastKind::Info => lucide::INFO,
        ToastKind::Error => lucide::ALERT_CIRCLE,
    }
}

/// Per-toast visual props, built by the stack.
struct ToastVisuals {
    index: usize,
    id: u64,
    /// Unwrapped text, for assistive tech.
    message: String,
    description: Option<String>,
    dismiss: Action,
    /// Button labels with the actions they emit.
    actions: Vec<(String, Action)>,
    kind: ToastKind,
    title_lines: Vec<String>,
    description_lines: Vec<String>,
    /// Fraction of lifetime consumed (0.0 = fresh, 1.0 = about to dismiss) —
    /// used when `external_progress` is `None`.
    time_progress: f32,
    /// When set, renders as an actual progress bar instead of a lifetime bar.
    /// The bar fills left-to-right as `external_progress` climbs from 0 to 1.
    external_progress: Option<f32>,
    bottom: f32,
    left: f32,
    width: f32,
    height: f32,
    z: i32,
}

impl RenderOnce for ToastVisuals {
    fn render(self, cx: &ElementContext) -> AnyElement {
        let tc = &cx.theme.colors;
        let scale = cx.theme.metrics.ui_scale();
        let accent = severity_color(self.kind, tc);
        let icon_svg = severity_icon(self.kind);

        let progress_inset = CORNER_RADIUS;
        let progress_track_w = (self.width - progress_inset * 2.0).max(0.0);
        let fill_fraction = self
            .external_progress
            .map(|p| p.clamp(0.0, 1.0))
            .unwrap_or_else(|| self.time_progress.clamp(0.0, 1.0));
        let progress_fill_w = progress_track_w * fill_fraction;

        let badge_bg = accent.with_alpha(Alpha::TINT);
        let track_bg = tc.border.with_alpha(Alpha::SOFT);

        let title_children: Vec<AnyElement> = self
            .title_lines
            .into_iter()
            .map(|line| {
                text(line)
                    .text_sm()
                    .medium()
                    .truncate()
                    .color(tc.text_strong)
                    .into_any()
            })
            .collect();

        let desc_children: Vec<AnyElement> = self
            .description_lines
            .into_iter()
            .map(|line| {
                text(line)
                    .text_xs()
                    .truncate()
                    .color(tc.text_muted)
                    .into_any()
            })
            .collect();

        let has_description = !desc_children.is_empty();
        // A live region: screen readers speak the message when the toast
        // appears, interrupting for errors.
        let (role, politeness) = match self.kind {
            ToastKind::Info => (accesskit::Role::Status, Politeness::Polite),
            ToastKind::Error => (accesskit::Role::Alert, Politeness::Assertive),
        };
        let toast_id = self.id;
        let action_buttons: Vec<AnyElement> = self
            .actions
            .into_iter()
            .map(|(label, action)| {
                div()
                    .flex_row()
                    .items_center()
                    .flex_shrink_0()
                    .h(ACTION_H)
                    .px(Sp::SM)
                    .rounded(Rad::MD)
                    .border(tc.border)
                    .hover_bg(tc.ghost_element_hover)
                    .on_click(action)
                    .cursor(CursorHint::Pointer)
                    .accessibility_id(format!("toast-action:{toast_id}:{label}"))
                    .accessibility_role(accesskit::Role::Button)
                    .accessibility_label(label.clone())
                    .child(text(label).text_xs().medium().color(tc.text_strong))
                    .into_any()
            })
            .collect();

        view! { scale,
            <div class="absolute"
                h={self.height}
                w={self.width}
                bottom={self.bottom}
                left={self.left}
                bg={tc.elevated_surface}
                rounded={CORNER_RADIUS}
                border={tc.border}
                shadow_preset={Shadow::TOAST}
                on_click={self.dismiss.clone()}
                hit_identity={HitIdentity::Toast(self.index)}
                cursor={CursorHint::Pointer}
                z_index={self.z}
                accessibility_id={format!("toast:{toast_id}")}
                accessibility_role={role}
                accessibility_label={self.message}
                accessibility_description={self.description.unwrap_or_default()}
                live={politeness}
            >
                // Main row: leading badge | stacked title/description | close.
                <div class="flex-row items-center h-full w-full"
                    pl={Sp::MD}
                    pr={Sp::MD}
                    py={PAD_Y}
                    gap={Sp::MD}
                >
                    <div class="flex-row items-center justify-center shrink-0"
                        w={BADGE_SIZE} h={BADGE_SIZE}
                        rounded={BADGE_SIZE / 2.0}
                        bg={badge_bg}
                    >
                        <icon svg={icon_svg} size={Ico::SM} color={accent} />
                    </div>

                    <div class="flex-1 flex-col" min_w={0.0}>
                        {...title_children}
                        if has_description {
                            <div class="flex-col" pt={DESC_GAP} min_w={0.0}>
                                {...desc_children}
                            </div>
                        }
                    </div>

                    {...action_buttons}

                    <div class="flex-row items-center justify-center shrink-0"
                        w={CLOSE_SIZE} h={CLOSE_SIZE}
                        rounded={Rad::MD}
                        hover_bg={tc.ghost_element_hover}
                        on_click={self.dismiss.clone()}
                        hit_identity={HitIdentity::Toast(self.index)}
                        cursor={CursorHint::Pointer}
                        accessibility_id={format!("toast-dismiss:{toast_id}")}
                        accessibility_role={accesskit::Role::Button}
                        accessibility_label={"Dismiss"}
                    >
                        <icon svg={lucide::X} size={Ico::XS} color={tc.text_muted} />
                    </div>
                </div>

                // Time-remaining progress bar — fills left→right.
                <div class="absolute"
                    bottom={3.0} left={progress_inset}
                    h={PROGRESS_H}
                    w={progress_track_w}
                    rounded={PROGRESS_H / 2.0}
                    bg={track_bg}
                    overflow_hidden
                >
                    <div h_full w={progress_fill_w} bg={accent} />
                </div>
            </div>
        }
    }
}

pub struct ToastStack<'a> {
    pub toasts: &'a [Toast],
    pub animation: &'a AnimationTable,
    pub window_width: f32,
    pub window_height: f32,
    pub ui_scale: f32,
    pub status_bar_height: f32,
    pub clock_ms: u64,
    /// Parallel to `toasts`: pre-wrapped lines + total height per toast.
    pub layouts: &'a [ToastLayout],
    /// Builds the action emitted when the toast at an index is clicked.
    pub on_dismiss: Box<dyn Fn(usize) -> Action + 'a>,
    /// Builds the action a toast's button emits from the toast's id and the
    /// button's index. `None` emits the button's own action.
    pub on_action: Option<Box<dyn Fn(u64, usize) -> Action + 'a>>,
}

impl<'a> ToastStack<'a> {
    // Positional to match the struct's pub fields; callers that prefer names
    // can build the struct literally.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        toasts: &'a [Toast],
        animation: &'a AnimationTable,
        window_width: f32,
        window_height: f32,
        ui_scale: f32,
        status_bar_height: f32,
        clock_ms: u64,
        layouts: &'a [ToastLayout],
        on_dismiss: impl Fn(usize) -> Action + 'a,
    ) -> Self {
        Self {
            toasts,
            animation,
            window_width,
            window_height,
            ui_scale,
            status_bar_height,
            clock_ms,
            layouts,
            on_dismiss: Box::new(on_dismiss),
            on_action: None,
        }
    }

    /// Route button presses through the app, as [`ToastQueue::activate`]
    /// needs, instead of emitting the buttons' actions directly.
    pub fn on_action(mut self, f: impl Fn(u64, usize) -> Action + 'a) -> Self {
        self.on_action = Some(Box::new(f));
        self
    }

    fn container_left_width(&self) -> (f32, f32) {
        let scale = self.ui_scale;
        let container_w = toast_stack_width(self.window_width, scale);
        let side_margin = (Sp::XL * scale).round();
        let container_left = (self.window_width - container_w - side_margin).max(side_margin);
        (container_left, container_w)
    }

    fn container_bottom(&self) -> f32 {
        self.status_bar_height + (Sp::LG * self.ui_scale).round()
    }

    fn height_of(&self, index: usize) -> f32 {
        self.layouts
            .get(index)
            .map(|l| l.height)
            .unwrap_or(Sz::TOAST)
    }

    /// Height of the stack at the current fan state.
    fn stack_height(&self) -> f32 {
        let count = self.toasts.len();
        if count == 0 {
            return 0.0;
        }
        let scale = self.ui_scale;
        let peek = (STACK_PEEK * scale).round();
        let fan_gap = (FAN_GAP * scale).round();
        let fan_t = self.animation.get(stack_key(), FAN).unwrap_or(0.0);
        let visible = count.min(MAX_VISIBLE_BEHIND + 1);
        // Collapsed: front-toast height + MAX_VISIBLE_BEHIND peeks.
        let collapsed = self.height_of(count - 1) + (MAX_VISIBLE_BEHIND as f32) * peek;
        // Fanned: visible toast heights + inter-toast gaps.
        let fanned = (0..visible)
            .map(|d| self.height_of(count - 1 - d))
            .sum::<f32>()
            + (visible.saturating_sub(1) as f32) * fan_gap;
        collapsed + fan_t * (fanned - collapsed)
    }

    /// The stack's rect in the window, for hover tracking.
    pub fn bounds(&self) -> Rect {
        let (left, width) = self.container_left_width();
        let height = self.stack_height();
        Rect {
            x: left,
            y: self.window_height - self.container_bottom() - height,
            width,
            height,
        }
    }

    pub fn build(self) -> Div {
        let scale = self.ui_scale;
        let peek = (STACK_PEEK * scale).round();
        let fan_gap = (FAN_GAP * scale).round();
        let (container_left, container_w) = self.container_left_width();
        let fan_t = self.animation.get(stack_key(), FAN).unwrap_or(0.0);
        let count = self.toasts.len();
        let visible = count.min(MAX_VISIBLE_BEHIND + 1);

        let mut container = div()
            .absolute()
            .bottom(self.container_bottom())
            .left(container_left)
            .w(container_w)
            .h(self.stack_height())
            .z_index(TOAST_Z_BASE);

        // Pre-compute cumulative fanned offsets from front (depth 0) upward.
        // fanned_bottom[d] = sum of heights of depths [0..d] + d gaps.
        let mut fanned_bottoms = Vec::with_capacity(visible);
        let mut running = 0.0_f32;
        for d in 0..visible {
            fanned_bottoms.push(running);
            running += self.height_of(count - 1 - d) + fan_gap;
        }

        // Deepest first so the front toast paints last.
        for depth in (0..visible).rev() {
            let toast_idx = count - 1 - depth;
            let toast = &self.toasts[toast_idx];
            let layout = self.layouts.get(toast_idx).cloned().unwrap_or(ToastLayout {
                title_lines: vec![toast.message.clone()],
                description_lines: toast
                    .description
                    .clone()
                    .map(|d| vec![d])
                    .unwrap_or_default(),
                height: Sz::TOAST,
            });

            // Fixed width across all depths — matches Sonner. Back toasts
            // only differ by vertical offset (peek when collapsed, cumulative
            // height when fanned).
            let width = container_w;
            let collapsed_bottom = (depth as f32) * peek;
            let fanned_bottom = fanned_bottoms[depth];
            let bottom_raw = collapsed_bottom + fan_t * (fanned_bottom - collapsed_bottom);
            let left = 0.0;

            let bottom = if depth == 0 {
                let anim_t = self
                    .animation
                    .get(toast_key(toast.id), ENTRANCE)
                    .unwrap_or(1.0);
                bottom_raw - (1.0 - anim_t) * layout.height
            } else {
                bottom_raw
            };

            let time_progress = toast.lifetime_used(self.clock_ms);
            let actions = toast
                .actions
                .iter()
                .enumerate()
                .map(|(i, button)| {
                    let action = match &self.on_action {
                        Some(on_action) => on_action(toast.id, i),
                        None => button.action.clone(),
                    };
                    (button.label.clone(), action)
                })
                .collect();
            let external_progress = toast.progress.map(|raw| {
                self.animation
                    .get(toast_key(toast.id), PROGRESS)
                    .unwrap_or(raw)
            });

            let z = TOAST_Z_BASE + (visible - depth) as i32;

            container = container.child(ToastVisuals {
                index: toast_idx,
                id: toast.id,
                message: toast.message.clone(),
                description: toast.description.clone(),
                dismiss: (self.on_dismiss)(toast_idx),
                actions,
                kind: toast.kind,
                title_lines: layout.title_lines,
                description_lines: layout.description_lines,
                time_progress,
                external_progress,
                bottom,
                left,
                width,
                height: layout.height,
                z,
            });
        }

        container
    }
}

/// The app's toasts with their lifecycle: ids, entrance and exit, timed
/// expiry that pauses while the stack is hovered, action buttons that work
/// only while the toast is live, and the undo shortcut.
///
/// Each frame, call [`ToastQueue::tick`] and schedule a frame at the time
/// it returns, then render [`ToastQueue::stack`]. Pass pointer moves to
/// [`ToastQueue::pointer_moved`], and key presses to
/// [`ToastQueue::undo_shortcut`].
#[derive(Debug, Default)]
pub struct ToastQueue {
    toasts: Vec<Toast>,
    /// Ids of toasts sliding out; their buttons no longer work.
    exiting: Vec<u64>,
    next_id: u64,
    hovered: bool,
    /// The stack's rect from the last [`Self::stack`].
    bounds: Option<Rect>,
}

impl ToastQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Oldest first; the newest is in front.
    pub fn toasts(&self) -> &[Toast] {
        &self.toasts
    }

    /// Show `toast`, assigning its id and creation time. Returns the id.
    pub fn push(&mut self, mut toast: Toast, table: &mut AnimationTable, now_ms: u64) -> u64 {
        self.next_id += 1;
        toast.id = self.next_id;
        toast.created_at_ms = now_ms;
        toast.paused_ms = 0;
        toast.paused_at_ms = None;
        toast.hovered = false;
        if self.hovered {
            toast.set_hovered(true, now_ms);
        }
        animate_toast_in(table, toast.id, now_ms);
        let id = toast.id;
        self.toasts.push(toast);
        id
    }

    fn is_live(&self, toast: &Toast, now_ms: u64) -> bool {
        !self.exiting.contains(&toast.id) && toast.remaining_ms(now_ms) != Some(0)
    }

    /// Start toast `id` sliding out.
    pub fn dismiss(&mut self, id: u64, table: &mut AnimationTable, now_ms: u64) {
        if self.toasts.iter().any(|t| t.id == id) && !self.exiting.contains(&id) {
            self.exiting.push(id);
            animate_toast_out(table, id, now_ms);
        }
    }

    /// Dismiss the toast at `index` in [`Self::toasts`], as the stack's
    /// `on_dismiss` reports.
    pub fn dismiss_index(&mut self, index: usize, table: &mut AnimationTable, now_ms: u64) {
        if let Some(id) = self.toasts.get(index).map(|t| t.id) {
            self.dismiss(id, table, now_ms);
        }
    }

    /// Button `index` of toast `id` was pressed: dismiss the toast and
    /// return the button's action, or `None` when the toast already timed
    /// out or left.
    pub fn activate(
        &mut self,
        id: u64,
        index: usize,
        table: &mut AnimationTable,
        now_ms: u64,
    ) -> Option<Action> {
        let toast = self.toasts.iter().find(|t| t.id == id)?;
        if !self.is_live(toast, now_ms) {
            return None;
        }
        let action = toast.actions.get(index)?.action.clone();
        self.dismiss(id, table, now_ms);
        Some(action)
    }

    /// The app's undo shortcut: `mod+z` presses the newest live toast's
    /// Undo button. Pass `in_text_field` as whether a text field has focus;
    /// there `mod+z` undoes typing instead, so this returns `None`.
    pub fn undo_shortcut(
        &mut self,
        pressed: &Binding,
        in_text_field: bool,
        table: &mut AnimationTable,
        now_ms: u64,
    ) -> Option<Action> {
        let m = pressed.mods;
        let primary = (m.cmd || m.ctrl) && !(m.cmd && m.ctrl);
        if in_text_field || pressed.key != "z" || !primary || m.alt || m.shift {
            return None;
        }
        let (id, index) = self
            .toasts
            .iter()
            .rev()
            .filter(|t| self.is_live(t, now_ms))
            .find_map(|t| Some((t.id, t.actions.iter().position(|a| a.undo)?)))?;
        self.activate(id, index, table, now_ms)
    }

    /// Pause every toast while the pointer is on the stack (and fan it
    /// out). Returns whether hover changed.
    pub fn set_hovered(&mut self, hovered: bool, table: &mut AnimationTable, now_ms: u64) -> bool {
        if self.hovered == hovered {
            return false;
        }
        self.hovered = hovered;
        for toast in &mut self.toasts {
            toast.set_hovered(hovered, now_ms);
        }
        animate_toast_fan(table, hovered, now_ms);
        true
    }

    /// The pointer moved to `pointer` (`None`: it left the window).
    /// Returns whether hover changed.
    pub fn pointer_moved(
        &mut self,
        pointer: Option<(f32, f32)>,
        table: &mut AnimationTable,
        now_ms: u64,
    ) -> bool {
        let over = !self.toasts.is_empty()
            && pointer
                .zip(self.bounds)
                .is_some_and(|((x, y), r)| r.contains(x, y));
        self.set_hovered(over, table, now_ms)
    }

    /// Slide out toasts whose time ran out and drop those whose exit
    /// finished. Returns when the next toast times out, for scheduling a
    /// frame; animations schedule their own.
    pub fn tick(&mut self, table: &mut AnimationTable, now_ms: u64) -> Option<u64> {
        for i in 0..self.toasts.len() {
            let toast = &self.toasts[i];
            if !self.exiting.contains(&toast.id) && toast.remaining_ms(now_ms) == Some(0) {
                let id = toast.id;
                self.dismiss(id, table, now_ms);
            }
        }
        let exiting = &mut self.exiting;
        self.toasts.retain(|t| {
            let gone = exiting.contains(&t.id) && retire_toast(table, t.id);
            if gone {
                exiting.retain(|id| *id != t.id);
            }
            !gone
        });
        if self.toasts.is_empty() && self.hovered {
            self.hovered = false;
            animate_toast_fan(table, false, now_ms);
        }
        self.toasts
            .iter()
            .filter(|t| !self.exiting.contains(&t.id) && t.paused_at_ms.is_none())
            .filter_map(|t| Some(now_ms + t.remaining_ms(now_ms)?))
            .min()
    }

    /// The stack for this frame. Remembers its rect for
    /// [`Self::pointer_moved`]. `on_action` should route to
    /// [`Self::activate`].
    #[allow(clippy::too_many_arguments)]
    pub fn stack<'a>(
        &mut self,
        animation: &'a AnimationTable,
        window: (f32, f32),
        ui_scale: f32,
        status_bar_height: f32,
        clock_ms: u64,
        layouts: &'a [ToastLayout],
        on_dismiss: impl Fn(usize) -> Action + 'a,
        on_action: impl Fn(u64, usize) -> Action + 'a,
    ) -> Div {
        let stack = ToastStack::new(
            &self.toasts,
            animation,
            window.0,
            window.1,
            ui_scale,
            status_bar_height,
            clock_ms,
            layouts,
            on_dismiss,
        )
        .on_action(on_action);
        let bounds = stack.bounds();
        let built = stack.build();
        self.bounds = Some(bounds);
        built
    }
}
