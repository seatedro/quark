//! The composer: one editor shared by the home screen and threads, its
//! control row (add, approval, context ring, model, dictate, send/stop),
//! the context tray under it on the home screen, and the slash and `@`
//! suggestions its text opens.

use accesskit::Role;
use quark_app::ViewContext;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{
    Editor, TextEditCommand, TextEditOutcome, text_editor_element,
};

use crate::icons;
use crate::theme::{BODY, Pal, SMALL};
use crate::widgets::*;
use crate::{APPROVALS, COMPOSER_FOCUS, Codex, Menu, Msg};

pub const CARD_ID: &str = "composer.card";
/// One line of text at 14 points.
const LINE_H: f32 = 18.0;
const TEXT_MIN: f32 = 45.0;
const TEXT_MAX: f32 = 200.0;

/// Slash commands: name, description, icon.
pub const SLASH: &[(&str, &str, &str)] = &[
    ("Chat", "Don't work in a project", icons::CHAT),
    ("Cloud", "Run this chat in the cloud", icons::CLOUD),
    (
        "Code review",
        "Review unstaged changes or compare against a branch",
        icons::CODEX_MARK,
    ),
    ("Fast", "1.5x speed, increased usage", icons::BOLT),
    ("Feedback", "Send feedback about this chat", icons::FEEDBACK),
    (
        "Goal",
        "Set a goal that Codex will keep working towards",
        icons::TARGET,
    ),
    (
        "Init",
        "Create an AGENTS.md file with instructions for Codex",
        icons::DOC,
    ),
    ("MCP", "Show MCP server status", icons::MCP),
    ("Memories", "Use on, generate on", icons::MEMORY),
    ("Model", "GPT-5.4", icons::CUBE),
    ("Personality", "Friendly", icons::SMILE),
    ("Plan mode", "Turn plan mode on", icons::PLAN),
    ("Review", "Open the review panel", icons::REVIEW),
    ("Status", "Show session status", icons::GAUGE),
];

/// `@` file results for the demo project.
pub const FILES: &[&str] = &["cart.js", "cart.test.js", "package.json", "README.md"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Suggest {
    Slash(String),
    At(String),
}

pub struct State {
    pub editor: Editor,
    pub text_h: f32,
    /// Highlighted suggestion.
    pub hi: usize,
    pub focus_on_start: bool,
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl State {
    pub fn new() -> Self {
        let mut editor = Editor::default();
        editor.set_font_size(BODY);
        editor.set_line_height(Some(LINE_H));
        Self {
            editor,
            text_h: TEXT_MIN,
            hi: 0,
            focus_on_start: false,
        }
    }

    /// What the draft asks to suggest: a leading `/` word, or the `@` word
    /// at the end.
    pub fn suggest(&self) -> Option<Suggest> {
        let text = self.editor.text();
        if let Some(rest) = text.strip_prefix('/')
            && !rest.contains(char::is_whitespace)
        {
            return Some(Suggest::Slash(rest.to_lowercase()));
        }
        let word = text.rsplit(char::is_whitespace).next()?;
        word.strip_prefix('@')
            .map(|q| Suggest::At(q.to_lowercase()))
    }

    pub fn suggestions_open(&self) -> bool {
        self.suggest().is_some()
    }

    /// Slash commands matching the draft.
    pub fn slash_matches(filter: &str) -> Vec<&'static (&'static str, &'static str, &'static str)> {
        SLASH
            .iter()
            .filter(|(name, _, _)| name.to_lowercase().starts_with(filter))
            .collect()
    }

    pub fn pick(&mut self, index: usize) {
        match self.suggest() {
            Some(Suggest::Slash(filter)) => {
                if let Some((name, _, _)) = Self::slash_matches(&filter).get(index) {
                    self.editor.set_text(&format!("/{} ", name.to_lowercase()));
                }
            }
            Some(Suggest::At(query)) => {
                let files: Vec<&&str> = FILES
                    .iter()
                    .filter(|f| f.contains(query.as_str()))
                    .collect();
                if let Some(file) = files.get(index) {
                    let text = self.editor.text().to_owned();
                    let head = text.rsplit_once('@').map_or("", |(h, _)| h);
                    self.editor.set_text(&format!("{head}@{file} "));
                }
            }
            None => {}
        }
        self.hi = 0;
    }

    pub fn edit(&mut self, command: TextEditCommand, now_ms: u64) -> TextEditOutcome {
        self.editor.set_clock(now_ms);
        let outcome = self.editor.apply(command);
        if outcome.text_changed {
            self.hi = 0;
        }
        outcome
    }

    /// Lay the draft out at `width` and size the text area to it.
    pub fn fit(&mut self, vcx: &mut ViewContext, width: f32, min: f32) {
        self.editor
            .set_clock(vcx.frame.elapsed().as_millis() as u64);
        self.text_h = self
            .editor
            .flush_fit(&mut vcx.frame.text().system, width, min, TEXT_MAX);
    }
}

/// How much the control row shows at a width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Full,
    /// Under about 500 points the pills drop their labels (capture 63).
    Icons,
}

/// The composer card, `w` wide, for a thread (`thread`) or the home
/// screen. Returns the card and its height.
pub fn card(
    app: &mut Codex,
    p: &Pal,
    w: f32,
    placeholder: &str,
    vcx: &mut ViewContext,
) -> (Div, f32) {
    let density = if w < 500.0 {
        Density::Icons
    } else {
        Density::Full
    };
    let text_w = (w - 26.0).max(40.0);
    app.composer.fit(vcx, text_w, TEXT_MIN);
    let text_h = app.composer.text_h;
    let editor = text_editor_element(
        COMPOSER_FOCUS,
        ScrollActionBuilder::new(|lines| Msg::ComposerScroll(lines).into()),
    )
    .editor_snapshot(&app.composer.editor)
    .label(placeholder.to_owned())
    .placeholder(placeholder.to_owned())
    .focused(vcx.is_focused(COMPOSER_FOCUS))
    .font_size(BODY)
    .text_color(p.text)
    .w(text_w)
    .h(text_h);
    let h = 15.0 + text_h + 4.0 + 29.0 + 6.0;
    let card = div()
        .w(w)
        .h(h)
        .flex_col()
        .pt(15.0)
        .bg(p.composer)
        .rounded(20.0)
        .border(p.composer_border)
        .shadow(
            3.0,
            1.0,
            p.shadow
                .with_alpha(if p.mode == quark_app::quark_ui::theme::ThemeMode::Dark {
                    90
                } else {
                    18
                }),
        )
        .id(CARD_ID)
        .child(div().px(13.0).h(text_h).child(editor))
        .child(div().h(4.0))
        .child(controls(app, p, density, w));
    (card, h)
}

fn pill(p: &Pal, id: &str, label: &str, msg: Msg) -> Div {
    hrow()
        .h(29.0)
        .px(7.0)
        .gap(4.0)
        .rounded(14.0)
        .hover_bg(p.menu_hi)
        .id(id.to_owned())
        .accessibility_role(Role::Button)
        .accessibility_label(label.to_owned())
        .on_click(msg)
}

pub fn send_button(app: &Codex, p: &Pal) -> Div {
    let running = app.running();
    let has_text = !app.composer.editor.text().trim().is_empty();
    let (bg, fg, svg, label) = if running {
        (
            p.send_active,
            p.send_active_glyph,
            icons::STOP_GLYPH,
            "Stop",
        )
    } else if has_text {
        (p.send_active, p.send_active_glyph, icons::ARROW_UP, "Send")
    } else {
        (p.send_idle, p.send_idle_glyph, icons::ARROW_UP, "Send")
    };
    div()
        .w(28.0)
        .h(28.0)
        .flex_shrink_0()
        .rounded(14.0)
        .bg(bg)
        .items_center()
        .justify_center()
        .accessibility_role(Role::Button)
        .accessibility_label(label)
        .on_click(if running { Msg::Stop } else { Msg::Send })
        .child(ico(svg, if running { 14.0 } else { 17.0 }, fg))
}

/// The ring that shows how much of the context window is used.
pub fn context_ring(p: &Pal) -> Div {
    div()
        .w(13.0)
        .h(13.0)
        .rounded(7.0)
        .border(p.faint.with_alpha(160))
        .border_w(1.5)
        .accessibility_role(Role::Image)
        .accessibility_label("Context usage unavailable")
}

fn controls(app: &Codex, p: &Pal, density: Density, w: f32) -> Div {
    let in_thread = app.current_thread().is_some();
    let (model, effort) = app.model_label();
    let (approval, _, approval_icon) = APPROVALS[app.approval];
    let mut approval_pill = pill(p, "pill.approval", approval, Msg::Open(Menu::Approval))
        .child(ico(approval_icon, 16.0, p.muted));
    if density == Density::Full {
        approval_pill = approval_pill.child(txt(approval, BODY, p.muted));
    }
    approval_pill = approval_pill.child(ico(icons::CHEVRON_DOWN, 14.0, p.muted));
    let mut model_pill = pill(
        p,
        "pill.model",
        &format!("{model} {effort}"),
        Msg::Open(Menu::Model),
    )
    .gap(5.0)
    .child(text(model).size(BODY).medium().color(p.text_soft).no_wrap());
    if density == Density::Full {
        model_pill = model_pill.child(txt(effort, BODY, p.muted));
    }
    model_pill = model_pill.child(ico(icons::CHEVRON_DOWN, 14.0, p.muted));
    let _ = w;
    let mut row = hrow()
        .h(29.0)
        .pl(8.0)
        .pr(7.0)
        .gap(5.0)
        .child(
            icon_button(
                p,
                icons::PLUS,
                28.0,
                17.0,
                p.muted,
                "Add files and more",
                Msg::Open(Menu::Add),
            )
            .id("pill.add"),
        )
        .child(approval_pill)
        .child(div().flex_1());
    if in_thread {
        row = row.child(div().pr(4.0).child(context_ring(p)));
    }
    row.child(model_pill)
        .child(icon_button(
            p,
            icons::MIC,
            28.0,
            16.0,
            p.muted,
            "Dictate",
            Msg::Noop,
        ))
        .child(div().w(3.0))
        .child(send_button(app, p))
}

/// The single-line composer used while the side panel fills the window
/// (captures 38 and 40), with the "Latest turn" bar above it.
pub fn compact(app: &mut Codex, p: &Pal, w: f32, vcx: &mut ViewContext) -> Div {
    let text_w = (w - 420.0).max(80.0);
    app.composer.fit(vcx, text_w, LINE_H);
    let editor = text_editor_element(
        COMPOSER_FOCUS,
        ScrollActionBuilder::new(|lines| Msg::ComposerScroll(lines).into()),
    )
    .editor_snapshot(&app.composer.editor)
    .label("Ask for follow-up changes")
    .placeholder("Ask for follow-up changes")
    .focused(vcx.is_focused(COMPOSER_FOCUS))
    .font_size(BODY)
    .text_color(p.text)
    .w(text_w)
    .h(LINE_H);
    let (model, effort) = app.model_label();
    let latest = hrow()
        .w(w - 44.0)
        .h(36.0)
        .px(12.0)
        .bg(p.composer)
        .rounded_corners([14.0, 14.0, 0.0, 0.0])
        .border(p.hairline)
        .child(txt("Latest turn", BODY, p.muted))
        .child(div().flex_1())
        .child(ico(icons::CHEVRON_RIGHT, 14.0, p.muted));
    let bar = hrow()
        .w(w)
        .h(44.0)
        .pl(10.0)
        .pr(8.0)
        .gap(6.0)
        .bg(p.composer)
        .rounded(22.0)
        .border(p.composer_rim_bottom)
        .id(CARD_ID)
        .child(icon_button(
            p,
            icons::PLUS,
            28.0,
            17.0,
            p.muted,
            "Add files and more",
            Msg::Open(Menu::Add),
        ))
        .child(div().pl(6.0).child(editor))
        .child(div().flex_1())
        .child(context_ring(p))
        .child(div().w(6.0))
        .child(text(model).size(BODY).medium().color(p.text_soft).no_wrap())
        .child(txt(effort, BODY, p.muted))
        .child(ico(icons::CHEVRON_DOWN, 14.0, p.muted))
        .child(div().w(14.0))
        .child(icon_button(
            p,
            icons::HAND,
            28.0,
            16.0,
            p.muted,
            "Ask for approval",
            Msg::Open(Menu::Approval),
        ))
        .child(icon_button(
            p,
            icons::MIC,
            28.0,
            16.0,
            p.muted,
            "Dictate",
            Msg::Noop,
        ))
        .child(send_button(app, p));
    div()
        .flex_col()
        .items_center()
        .child(latest)
        .child(div().h(0.0))
        .child(bar)
}

/// The context tray under the home composer: "Choose project", or the
/// project, work location, and branch once a project is chosen.
pub fn tray(app: &Codex, p: &Pal, w: f32) -> Div {
    let item = |id: &str, svg: &'static str, label: &str, strong: bool, chevron: bool, msg: Msg| {
        let color = if strong { p.text_soft } else { p.muted };
        let mut d = hrow()
            .h(29.0)
            .px(7.0)
            .gap(6.0)
            .rounded(8.0)
            .hover_bg(p.menu_hi)
            .id(id.to_owned())
            .accessibility_role(Role::Button)
            .accessibility_label(label.to_owned())
            .on_click(msg)
            .child(ico(svg, 15.0, color))
            .child(txt(label, SMALL, color));
        if chevron {
            d = d.child(ico(icons::CHEVRON_DOWN, 13.0, p.muted));
        }
        d
    };
    let mut row = hrow().w(w).h(48.0).pt(13.0).pl(8.0).gap(10.0);
    match app.project.and_then(|id| app.data.project(id)) {
        Some(project) => {
            row = row
                .child(item(
                    "tray.project",
                    icons::DOC,
                    project.name,
                    true,
                    false,
                    Msg::Open(Menu::ChooseProject),
                ))
                .child(item(
                    "tray.location",
                    icons::LAPTOP,
                    "Work locally",
                    false,
                    true,
                    Msg::Open(Menu::WorkLocation),
                ))
                .child(item(
                    "tray.branch",
                    icons::BRANCH,
                    "main",
                    false,
                    true,
                    Msg::Open(Menu::Branch),
                ));
        }
        None => {
            row = row.child(item(
                "tray.project",
                icons::DOC,
                "Choose project",
                true,
                false,
                Msg::Open(Menu::ChooseProject),
            ));
        }
    }
    div()
        .w(w)
        .h(48.0)
        .bg(p.tray)
        .rounded_corners([0.0, 0.0, 16.0, 16.0])
        .child(row)
}

pub fn small_text(s: &str, p: &Pal) -> TextElement {
    txt(s, SMALL, p.muted)
}
