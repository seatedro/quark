//! The agent's change to cart.js in quark's shared diff viewer
//! (`quark_components::diff_view`), in the two places Codex shows a diff:
//! the inline card under an edit row (a compact preview of at most twelve
//! rows that offers Open full diff), and the Changes panel, which the full
//! view widens (review presentation, automatic layout). Both read one
//! immutable snapshot of the file, so their text and counts agree with the
//! transcript's; each keeps its own scroll, focus, and expanded context.
//!
//! Product chrome stays here: the card header, the file header and fold
//! bar wording (through a [`DiffDecorator`]), the Changes toolbar and find
//! bar, and one in-memory review comment thread with Reply and Resolve.

use std::rc::Rc;
use std::sync::Arc;

use accesskit::Role;
use quark::view;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::icons::lucide;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome, TextField};
use quark_app::quark_ui::theme::Color;
use quark_app::quark_ui::{FocusId, quark_syntax};
use quark_app::{UiContext, ViewContext};
use quark_components::diff_view::decorator::{
    AnnotationContext, DiffDecorator, GutterContext, HeaderContext, SeparatorContext,
};
use quark_components::diff_view::presentation::{
    DiffAppearance, DiffColorOverrides, DiffLayout, DiffPresentation, FileHeaders,
};
use quark_components::{
    AnnotationId, CollectionEnv, CopyContent, DiffAnchor, DiffAnnotation, DiffEvent, DiffOutcome,
    DiffPreviewLimit, DiffSessionViewState, DiffStyle, FindOptions, RevealAlign, SearchDirection,
    diff_session_view, diff_session_view_with,
};
use quark_diff::{
    ComparisonOptions, ContextPolicy, DiffSession, DiffUpdate, FileDiffSnapshot, FileId,
    InlineMode, InlineOptions, Mode, REVEAL_STEP, Reveal, Revision, Side, WhitespaceMode,
    diff_texts,
};
use quark_syntax::{GrammarStore, HighlightKind, HighlightWorker};

use crate::theme::{BODY, CODE, DARK, LIGHT, Pal, SMALL};
use crate::widgets::*;
use crate::{Codex, Msg, Tab, icons};

pub const INLINE_FOCUS: FocusId = FocusId::from_key("codex.diff.inline");
pub const REVIEW_FOCUS: FocusId = FocusId::from_key("codex.diff.review");
pub const FIND_FOCUS: FocusId = FocusId::from_key("codex.diff.find");
/// The changed file's identity in the session.
pub const FILE: FileId = FileId(1);
const REVISION: Revision = Revision(1);
/// Code rows are 21.5 points tall in the captures: 12-point text at 1.8.
const LINE_HEIGHT: f32 = 1.8;
/// The find bar under the Changes toolbar.
pub const FIND_H: f32 = 36.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiffMsg {
    Inline(DiffEvent),
    Review(DiffEvent),
    /// The inline card's copy button and "Copy git apply command": the
    /// change as a unified patch.
    CopyPatch,
    ToggleWrap,
    /// The full view's split button: split if unified, unified if split.
    ToggleSplit,
    /// "Switch to Auto diff" in the options menu, or back to split.
    ToggleAuto,
    ExpandAll,
    ToggleWordDiffs,
    ToggleWhitespace,
    /// Open or close the find bar.
    Find(bool),
    /// Next (`true`) or previous match.
    FindStep(bool),
    Reply(u64),
    Resolve(u64),
}

/// A review comment thread, anchored to a new-side line.
#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    pub id: u64,
    /// Zero-based new-side source line.
    pub line: u32,
    /// Author and text of each message, oldest first.
    pub messages: Vec<(&'static str, String)>,
    pub resolved: bool,
    /// Bumped when the thread's content (and so its height) changes.
    pub revision: u64,
}

/// The shared change and the two views of it.
pub struct Changes {
    pub inline: DiffSessionViewState,
    pub review: DiffSessionViewState,
    pub comments: Vec<Comment>,
    comments_rev: u64,
    /// The find bar's field while it is open.
    pub find: Option<TextField>,
    path: Arc<str>,
    stats: (u32, u32),
}

impl Changes {
    /// `old` against `new` at `path`, one line of context as Codex shows.
    pub fn new(path: &'static str, old: &str, new: &str) -> Self {
        let doc = diff_texts(Some(path), Some(path), Some(old), Some(new), 1);
        let stats = (doc.files().additions[0], doc.files().deletions[0]);
        let snapshot = FileDiffSnapshot::new(FILE, REVISION, Arc::new(doc)).expect("one file");
        let session = || {
            let mut session = DiffSession::new();
            session
                .apply(DiffUpdate::Upsert {
                    file: snapshot.clone(),
                    remap: None,
                })
                .expect("a fresh session takes the file");
            session
        };
        let style = DiffStyle {
            font_size: CODE,
            line_height: LINE_HEIGHT,
            ..DiffStyle::default()
        };
        // Codex folds even a single unchanged line.
        let policy = ContextPolicy {
            reveal_step: REVEAL_STEP,
            min_hidden: 0,
        };
        let mut inline = DiffSessionViewState::new("codex.diff.inline", INLINE_FOCUS, session())
            .with_label("Edited file diff")
            .with_style(style);
        inline.set_presentation(DiffPresentation::compact());
        inline.set_preview_limit(Some(DiffPreviewLimit::default()));
        inline.set_context_policy(policy);
        inline.set_appearance(appearance(true));
        let mut review = DiffSessionViewState::new("codex.diff.review", REVIEW_FOCUS, session())
            .with_label("Changes")
            .with_style(style)
            .with_scrollbar_auto_hide();
        review.set_presentation(DiffPresentation {
            headers: FileHeaders::Custom,
            sticky_headers: true,
            ..DiffPresentation::review()
        });
        review.set_context_policy(policy);
        review.set_appearance(appearance(false));
        Self {
            inline,
            review,
            comments: Vec::new(),
            comments_rev: 0,
            find: None,
            path: Arc::from(path),
            stats,
        }
    }

    /// The cart.js fix of turn 2.
    pub fn cart() -> Self {
        Self::new("cart.js", crate::data::CART_JS_OLD, crate::data::CART_JS)
    }

    /// Lines added and removed: what every count in the app shows.
    pub fn stats(&self) -> (u32, u32) {
        self.stats
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// Highlights both views on one worker with `store`'s grammars.
    pub fn enable_syntax(&mut self, store: GrammarStore) {
        let worker = HighlightWorker::new(store.clone());
        self.inline.enable_syntax_shared(&worker, store.clone());
        self.review.enable_syntax_shared(&worker, store);
    }

    /// Calls `wake` when highlights are ready to show.
    pub fn set_syntax_wake(&mut self, wake: impl Fn() + Send + Sync + Clone + 'static) {
        self.inline.set_syntax_wake(wake.clone());
        self.review.set_syntax_wake(wake);
    }

    /// The review thread the comment scene shows: a question on
    /// `applyDiscount`'s return.
    pub fn seed_comment(&mut self) {
        self.comments = vec![Comment {
            id: 1,
            line: 6,
            messages: vec![(
                crate::data::ACCOUNT_NAME,
                "Should percent be clamped to 0..100 before it is applied?".to_owned(),
            )],
            resolved: false,
            revision: 1,
        }];
        self.sync_comments();
    }

    /// Hands the threads to the review view, with their heights.
    fn sync_comments(&mut self) {
        self.comments_rev += 1;
        let annotations = self
            .comments
            .iter()
            .map(|c| DiffAnnotation {
                id: AnnotationId(c.id),
                anchor: DiffAnchor {
                    file: FILE,
                    revision: REVISION,
                    side: Side::New,
                    lines: c.line..c.line + 1,
                },
                revision: c.revision,
            })
            .collect();
        self.review.set_annotations(annotations);
        for c in &self.comments {
            self.review
                .set_annotation_height(AnnotationId(c.id), c.revision, thread_height(c));
        }
    }

    fn comment_mut(&mut self, id: u64) -> Option<&mut Comment> {
        self.comments.iter_mut().find(|c| c.id == id)
    }

    /// Edits the find field; a new query searches at once.
    pub fn edit_find(&mut self, command: TextEditCommand, now_ms: u64) -> TextEditOutcome {
        let Some(field) = &mut self.find else {
            return TextEditOutcome::default();
        };
        let outcome = field.apply_at(command, now_ms);
        if outcome.text_changed {
            let query = field.text().to_owned();
            self.review.set_find_query(&query, FindOptions::default());
            self.review.next_match(SearchDirection::Forward);
        }
        outcome
    }

    /// The inline card under an edit row, `w` points wide.
    pub fn inline_card(&mut self, p: &Pal, w: f32, vcx: &mut ViewContext) -> AnyElement {
        let body_w = (w - 2.0).max(0.0);
        let scale = vcx.frame.scale_factor();
        let now_ms = vcx.frame.elapsed().as_millis() as u64;
        // The card is as tall as its rows: measuring the font can change
        // that once, on the first frame.
        for _ in 0..2 {
            let h = self.inline.content_height();
            self.inline.set_viewport(body_w, h);
            let text = vcx.frame.text();
            self.inline
                .prepare(&mut text.system, &mut text.layouts, scale, now_ms);
            if self.inline.content_height() == h {
                break;
            }
        }
        let env = CollectionEnv {
            focused: vcx.is_focused(INLINE_FOCUS),
            accessible: vcx.frame.accessibility_active(),
        };
        let body = diff_session_view(&mut self.inline, vcx.theme, env, |e| {
            Msg::Diff(DiffMsg::Inline(e)).into()
        });
        let (adds, dels) = self.stats;
        let path = self.path.to_string();
        view! {
            <div class="w-full flex-col rounded-[8] overflow-hidden" bg={p.change_card}
                 border={p.shell_border} accessibility_role={Role::Group}
                 aria-label={format!("{path} diff")}>
                <div class="flex-row items-center h-7 pl-[10] pr-1 gap-1.5" bg={p.shell}>
                    <div role="link" aria-label={format!("Open {path} in Changes")}
                         on:click={Msg::OpenTab(Tab::Changes)}>
                        <txt(path.clone(), SMALL, p.text_soft) />
                    </div>
                    <txt(format!("+{adds}"), SMALL, p.success) />
                    <txt(format!("-{dels}"), SMALL, p.error) />
                    <div class="flex-1" />
                    <icon_button(p, icons::COPY, 22.0, 13.0, p.muted, "Copy diff",
                                 Msg::Diff(DiffMsg::CopyPatch)) />
                </div>
                {body}
            </div>
        }
    }

    /// The Changes panel's diff, `w` by `h`.
    pub fn review_view(&mut self, w: f32, h: f32, vcx: &mut ViewContext) -> AnyElement {
        let p = crate::theme::pal(vcx.theme.mode);
        self.review.set_viewport(w, h);
        let scale = vcx.frame.scale_factor();
        let now_ms = vcx.frame.elapsed().as_millis() as u64;
        let text = vcx.frame.text();
        self.review
            .prepare(&mut text.system, &mut text.layouts, scale, now_ms);
        let env = CollectionEnv {
            focused: vcx.is_focused(REVIEW_FOCUS),
            accessible: vcx.frame.accessibility_active(),
        };
        let deco: Rc<dyn DiffDecorator> = Rc::new(Review {
            p,
            comments: self.comments.clone(),
            revision: self.comments_rev * 2 + u64::from(p.mode == LIGHT.mode),
        });
        diff_session_view_with(
            &mut self.review,
            vcx.theme,
            env,
            |e| Msg::Diff(DiffMsg::Review(e)).into(),
            Some(deco),
        )
    }

    /// The find bar under the Changes toolbar, while open.
    pub fn find_bar(&self, p: &Pal, w: f32, vcx: &ViewContext) -> Option<AnyElement> {
        let field = self.find.as_ref()?;
        let summary = self.review.search_summary();
        let count = match (summary.matches, summary.active) {
            (0, _) if field.text().is_empty() => String::new(),
            (0, _) => "No results".to_owned(),
            (n, Some(i)) => format!("{} of {n}", i + 1),
            (n, None) => format!("{n} results"),
        };
        Some(view! {
            <div class="flex-row items-center px-3 gap-2" w={w} h={FIND_H} border_b={p.hairline}
                 accessibility_role={Role::Search} aria-label="Find in changes">
                <icon svg={icons::SEARCH} size={14.0} color={p.muted} />
                <text_input("Find in changes", "") placeholder="Find in changes"
                    focus_target={FIND_FOCUS} focused={vcx.is_focused(FIND_FOCUS)} field={field}
                    bare w={(w - 170.0).max(60.0)} class="h-5" />
                <div class="flex-1" />
                <div role="status" aria-label={count.clone()}>
                    <txt(count, SMALL, p.muted) />
                </div>
                <icon_button(p, lucide::CHEVRON_UP, 22.0, 13.0, p.icon, "Previous match",
                             Msg::Diff(DiffMsg::FindStep(false))) />
                <icon_button(p, icons::CHEVRON_DOWN, 22.0, 13.0, p.icon, "Next match",
                             Msg::Diff(DiffMsg::FindStep(true))) />
                <icon_button(p, icons::CLOSE, 22.0, 11.0, p.icon, "Close find",
                             Msg::Diff(DiffMsg::Find(false))) />
            </div>
        })
    }

    pub fn wraps(&self) -> bool {
        self.review.style().wrap
    }

    pub fn word_diffs(&self) -> bool {
        self.review.inline_options().mode != InlineMode::Off
    }

    pub fn hides_whitespace(&self) -> bool {
        self.review.comparison().whitespace != WhitespaceMode::Exact
    }

    pub fn auto_layout(&self) -> bool {
        matches!(self.review.presentation().layout, DiffLayout::Auto { .. })
    }

    /// Shows the review view split or unified, as the user chose.
    pub fn set_layout(&mut self, layout: DiffLayout) {
        self.review.set_presentation(DiffPresentation {
            layout,
            ..self.review.presentation()
        });
    }
}

pub fn update(app: &mut Codex, msg: DiffMsg, cx: &mut UiContext) {
    let changes = &mut app.changes;
    let outcome = match msg {
        DiffMsg::Inline(event) => changes.inline.handle(event),
        DiffMsg::Review(event) => changes.review.handle(event),
        DiffMsg::CopyPatch => match changes.review.copy(CopyContent::Patch) {
            Some(patch) => DiffOutcome::Copy(patch),
            None => DiffOutcome::Unchanged,
        },
        DiffMsg::ToggleWrap => {
            let style = changes.review.style();
            changes.review.set_style(DiffStyle {
                wrap: !style.wrap,
                ..style
            });
            DiffOutcome::Changed
        }
        DiffMsg::ToggleSplit => {
            let layout = match changes.review.mode() {
                Mode::Split => DiffLayout::Unified,
                Mode::Unified => DiffLayout::Split,
            };
            changes.set_layout(layout);
            DiffOutcome::Changed
        }
        DiffMsg::ToggleAuto => {
            let layout = if changes.auto_layout() {
                DiffLayout::Split
            } else {
                DiffLayout::AUTO
            };
            changes.set_layout(layout);
            DiffOutcome::Changed
        }
        DiffMsg::ExpandAll => {
            changes.review.expand_all();
            DiffOutcome::Changed
        }
        DiffMsg::ToggleWordDiffs => {
            let mode = if changes.word_diffs() {
                InlineMode::Off
            } else {
                InlineMode::Word
            };
            changes.review.set_inline_options(InlineOptions {
                mode,
                ..changes.review.inline_options()
            });
            DiffOutcome::Changed
        }
        DiffMsg::ToggleWhitespace => {
            let whitespace = if changes.hides_whitespace() {
                WhitespaceMode::Exact
            } else {
                WhitespaceMode::IgnoreAllSpace
            };
            changes.review.set_comparison(ComparisonOptions {
                whitespace,
                ..changes.review.comparison()
            });
            DiffOutcome::Changed
        }
        DiffMsg::Find(open) => {
            if open {
                changes.find.get_or_insert_with(|| TextField::new(""));
                cx.set_focus(Some(FIND_FOCUS));
            } else {
                changes.find = None;
                changes.review.set_find_query("", FindOptions::default());
                cx.set_focus(Some(REVIEW_FOCUS));
            }
            DiffOutcome::Changed
        }
        DiffMsg::FindStep(forward) => {
            let direction = if forward {
                SearchDirection::Forward
            } else {
                SearchDirection::Backward
            };
            changes.review.next_match(direction);
            DiffOutcome::Changed
        }
        DiffMsg::Reply(id) => {
            if let Some(c) = changes.comment_mut(id) {
                c.messages.push((
                    "Codex",
                    "Good catch: I'll clamp it and add a test for 0 and 100.".to_owned(),
                ));
                c.revision += 1;
            }
            changes.sync_comments();
            DiffOutcome::Changed
        }
        DiffMsg::Resolve(id) => {
            if let Some(c) = changes.comment_mut(id) {
                c.resolved = !c.resolved;
                c.revision += 1;
            }
            changes.sync_comments();
            DiffOutcome::Changed
        }
    };
    match outcome {
        DiffOutcome::Copy(text) if !text.is_empty() => cx.window.set_clipboard_text(&text),
        DiffOutcome::OpenFull { target } => {
            app.side_panel = true;
            if !app.tabs.contains(&Tab::Changes) {
                app.tabs.push(Tab::Changes);
            }
            app.tab = Tab::Changes;
            app.changes.review.reveal_target(target, RevealAlign::Top);
            cx.set_focus(Some(REVIEW_FOCUS));
        }
        DiffOutcome::Annotate { anchor } => {
            let changes = &mut app.changes;
            let id = changes.comments.iter().map(|c| c.id).max().unwrap_or(0) + 1;
            changes.comments.push(Comment {
                id,
                line: anchor.lines.start,
                messages: vec![(crate::data::ACCOUNT_NAME, "New comment".to_owned())],
                resolved: false,
                revision: 1,
            });
            changes.sync_comments();
        }
        _ => {}
    }
}

/// A thread's height: a row per message and the action row, or one row
/// once resolved.
fn thread_height(c: &Comment) -> f32 {
    if c.resolved {
        36.0
    } else {
        16.0 + 42.0 * c.messages.len() as f32 + 32.0
    }
}

/// Codex's diff colors over the theme's, for the inline card (`card`) or
/// the panel.
fn appearance(card: bool) -> DiffAppearance {
    DiffAppearance {
        light: overrides(&LIGHT, card),
        dark: overrides(&DARK, card),
    }
}

fn overrides(p: &Pal, card: bool) -> DiffColorOverrides {
    let surface = if card { p.change_card } else { p.bg };
    let mut o = DiffColorOverrides {
        surface: Some(surface),
        text: Some(p.code),
        add_line: Some(p.add_code),
        del_line: Some(p.del_code),
        add_word: Some(p.word_add),
        del_word: Some(p.word_del),
        add_marker: Some(p.add_bar),
        del_marker: Some(p.del_bar),
        add_number: Some(p.add_num),
        del_number: Some(p.del_num),
        gutter: Some(surface),
        gutter_text: Some(p.line_num),
        empty_side: Some(surface),
        hatch: Some(p.hairline),
        separator: Some(p.fold),
        selection: Some(p.accent.with_alpha(70)),
        focused_row: Some(p.accent),
        ..DiffColorOverrides::default()
    };
    let syntax = [
        (HighlightKind::Keyword, p.syn_keyword),
        (HighlightKind::Preprocessor, p.syn_keyword),
        (HighlightKind::Function, p.syn_function),
        (HighlightKind::Type, p.syn_function),
        (HighlightKind::Property, p.syn_ident),
        (HighlightKind::Operator, p.syn_ident),
        (HighlightKind::Builtin, p.syn_ident),
        (HighlightKind::Number, p.syn_number),
        (HighlightKind::Constant, p.syn_number),
        (HighlightKind::String, p.syn_string),
        (HighlightKind::Comment, p.syn_comment),
        (HighlightKind::Punctuation, p.syn_punct),
        (HighlightKind::Variable, p.code),
    ];
    for (kind, color) in syntax {
        o = o.with_syntax(kind, color);
    }
    o
}

/// The panel's decorator: Codex's file header, fold bars, comment
/// threads, and the focused row's Add comment button.
struct Review {
    p: &'static Pal,
    comments: Vec<Comment>,
    revision: u64,
}

impl DiffDecorator for Review {
    fn revision(&self) -> u64 {
        self.revision
    }

    fn header(&self, cx: &HeaderContext) -> Option<AnyElement> {
        let p = self.p;
        let title = cx.title.to_owned();
        Some(view! {
            <div class="flex-row items-center pl-4 pr-[10] gap-[9]" w={cx.width} h={cx.height}
                 bg={p.bg} border_b={p.hairline} border_t={p.hairline} role="heading"
                 aria-label={title.clone()}>
                {js_badge()}
                <txt(title, BODY, p.text_soft) />
                <div class="flex-1" />
                <txt(format!("+{}", cx.additions), BODY, p.add_num) />
                <txt(format!("-{}", cx.deletions), BODY, p.del_num) />
                <div class="w-0.5" />
                <icon_button(p, icons::OPEN_EXTERNAL, 24.0, 13.0, p.icon, "Open in",
                             Msg::OpenFile("cart.js")) />
                <icon_button(p, icons::ELLIPSIS, 24.0, 13.0, p.icon, "File options", Msg::Noop) />
            </div>
        })
    }

    fn separator(&self, cx: &SeparatorContext) -> Option<AnyElement> {
        let gap = cx.gap?;
        let p = self.p;
        let label = format!(
            "{} unmodified line{}",
            cx.hidden,
            if cx.hidden == 1 { "" } else { "s" }
        );
        // Fold bars cover the text columns; the gutters stay clear.
        let bars: Vec<(f32, f32, bool)> = cx
            .columns
            .sides
            .iter()
            .flatten()
            .enumerate()
            .map(|(i, c)| (c.text_x, c.text_w - 2.0, i == 0))
            .collect();
        let h = (cx.height - 4.0).max(0.0);
        Some(view! {
            <div class="relative" w={cx.width} h={cx.height} role="button"
                 aria-label={format!("Show {label}")}
                 on:click={Msg::Diff(DiffMsg::Review(DiffEvent::Expand(gap, Reveal::All)))}>
                for (x, w, first) in bars {
                    <div class="absolute top-0.5 flex-row items-center pl-2" left={x} w={w} h={h}
                         bg={p.fold} hover_bg={p.fold.lerp(p.text, 0.06)}>
                        if first {
                            <txt(label.clone(), SMALL, p.muted) />
                        }
                    </div>
                }
            </div>
        })
    }

    fn annotation(&self, cx: &AnnotationContext) -> Option<AnyElement> {
        let c = self.comments.iter().find(|c| c.id == cx.id.0)?;
        let p = self.p;
        let id = c.id;
        let w = (cx.width - 24.0).max(0.0);
        let line = c.line + 1;
        if c.resolved {
            return Some(view! {
                <div class="flex-row items-center px-3 py-1" w={cx.width}>
                    <div class="flex-row items-center px-3 gap-2 rounded-[8]" w={w} h={28.0}
                         bg={p.shell} border={p.shell_border} accessibility_role={Role::Group}
                         aria-label={format!("Resolved comment on line {line}")}>
                        <icon svg={icons::CHECK} size={13.0} color={p.success} />
                        <txt(format!("Resolved comment on line {line}"), SMALL, p.muted) />
                        <div class="flex-1" />
                        <div role="button" aria-label="Reopen" on:click={Msg::Diff(DiffMsg::Resolve(id))}>
                            <txt("Reopen", SMALL, p.text_soft) />
                        </div>
                    </div>
                </div>
            });
        }
        let messages = c.messages.clone();
        Some(view! {
            <div class="flex-row px-3 py-2" w={cx.width}>
                <div class="flex-col px-3 py-1 rounded-[10]" w={w} bg={p.shell}
                     border={p.shell_border} accessibility_role={Role::Group}
                     aria-label={format!("Comment thread on line {line}")}>
                    for (author, body) in messages {
                        <div class="flex-col justify-center gap-0.5 overflow-hidden" h={42.0}>
                            <txt(author, SMALL, p.text) class="font-semibold" />
                            <txt(body, BODY, p.text_soft) class="truncate" />
                        </div>
                    }
                    <div class="flex-row items-center h-[30] gap-1.5">
                        <div class="flex-row items-center h-6 px-2.5 rounded-[7]"
                             border={p.shell_border} role="button" aria-label="Reply"
                             on:click={Msg::Diff(DiffMsg::Reply(id))}>
                            <txt("Reply", SMALL, p.text_soft) />
                        </div>
                        <div class="flex-row items-center h-6 px-2.5 rounded-[7]"
                             border={p.shell_border} role="button" aria-label="Resolve"
                             on:click={Msg::Diff(DiffMsg::Resolve(id))}>
                            <txt("Resolve", SMALL, p.text_soft) />
                        </div>
                    </div>
                </div>
            </div>
        })
    }

    fn gutter_utility(&self, cx: &GutterContext) -> Option<AnyElement> {
        let p = self.p;
        let event = DiffEvent::Annotate {
            file: cx.file,
            side: cx.side,
            line: cx.line,
        };
        Some(view! {
            <div class="flex-row items-center justify-start pl-1.5" w={cx.width} h={cx.height}>
                <div class="w-4 h-4 rounded-[4] items-center justify-center" bg={p.accent}
                     role="button" aria-label={format!("Add comment on line {}", cx.line + 1)}
                     on:click={Msg::Diff(DiffMsg::Review(event))}>
                    <icon svg={icons::PLUS} size={11.0} color={Color::rgba(255, 255, 255, 255)} />
                </div>
            </div>
        })
    }
}
