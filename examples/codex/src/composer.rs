//! The composer: one editor shared by the home screen and threads, its
//! control row (add, approval, context ring, model, dictate, send/stop),
//! the context tray under it on the home screen, and the slash and `@`
//! suggestions its text opens.

use quark::view;
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

/// Slash commands: name, description, icon, and whether it opens a
/// submenu (u75).
pub const SLASH: &[(&str, &str, &str, bool)] = &[
    ("Chat", "Don't work in a project", icons::CHAT, false),
    (
        "Code review",
        "Review uncommitted changes or compare against a branch",
        icons::CODEX_MARK,
        true,
    ),
    ("Fast", "1.5x speed, increased usage", icons::BOLT, false),
    (
        "Feedback",
        "Send feedback about this chat",
        icons::FEEDBACK,
        false,
    ),
    ("Goal", "Set a goal to keep pursuing", icons::TARGET, false),
    (
        "Init",
        "Create an AGENTS.md file with instructions for Codex",
        icons::DOC,
        false,
    ),
    ("MCP", "Show MCP server status", icons::MCP, true),
    ("Model", "", icons::CUBE, true),
    (
        "Pet",
        "Wake or tuck away the desktop pet",
        icons::PET,
        false,
    ),
    ("Plan mode", "Turn plan mode on", icons::BULB, false),
    ("Reasoning", "Light", icons::BRAIN, true),
    ("Review", "Open the Changes tab", icons::REVIEW, false),
    ("Status", "Show session status", icons::GAUGE, false),
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
    pub fn slash_matches(
        filter: &str,
    ) -> Vec<&'static (&'static str, &'static str, &'static str, bool)> {
        SLASH
            .iter()
            .filter(|(name, ..)| name.to_lowercase().starts_with(filter))
            .collect()
    }

    pub fn pick(&mut self, index: usize) {
        match self.suggest() {
            Some(Suggest::Slash(filter)) => {
                if let Some((name, ..)) = Self::slash_matches(&filter).get(index) {
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
    /// Narrow composers drop the pills' labels (u25, capture 63).
    Icons,
}

/// The composer card, `w` wide. Returns the card and its height.
pub fn card(
    app: &mut Codex,
    p: &Pal,
    w: f32,
    placeholder: &str,
    vcx: &mut ViewContext,
) -> (AnyElement, f32) {
    let density = if w < 420.0 {
        Density::Icons
    } else {
        Density::Full
    };
    let text_w = (w - 24.0).max(40.0);
    app.composer.fit(vcx, text_w, TEXT_MIN - 1.0);
    let text_h = app.composer.text_h;
    let h = 14.0 + text_h + 4.0 + 28.0 + 8.0;
    let light = p.mode == quark_app::quark_ui::theme::ThemeMode::Light;
    let scroll = ScrollActionBuilder::new(|lines| Msg::ComposerScroll(lines).into());
    let card = view! {
        <div w={w} h={h} class="flex-col pt-[14] rounded-[20]" bg={p.composer}
             border={p.composer_border} @when {light} { shadow={(10.0, 2.0, p.shadow)} }
             id={CARD_ID}>
            <div class="px-3" h={text_h}>
                <text_editor_element(COMPOSER_FOCUS, scroll)
                    editor_snapshot={&app.composer.editor} label={placeholder.to_owned()}
                    placeholder={placeholder.to_owned()} focused={vcx.is_focused(COMPOSER_FOCUS)}
                    font_size={BODY} text_color={p.text} w={text_w} h={text_h} />
            </div>
            <div class="h-1" />
            {controls(app, p, density)}
        </div>
    };
    (card, h)
}

fn pill(p: &Pal, id: &str, label: &str, msg: Msg, open: bool) -> Div {
    view! { -> Div,
        <div class="flex-row items-center h-7 px-[7] gap-1.5 rounded-[14]" hover_bg={p.menu_hi}
             bg={if open { p.menu_hi }} id={id.to_owned()} role="button"
             aria-label={label.to_owned()} on:click={msg} />
    }
}

/// The send slot: a voice button while empty, send with text, stop while
/// a turn runs.
pub fn send_button(app: &Codex, p: &Pal) -> AnyElement {
    let running = app.running();
    let has_text = !app.composer.editor.text().trim().is_empty();
    let (svg, label, size) = if running {
        (icons::STOP_GLYPH, "Stop", 13.0)
    } else if has_text {
        (icons::ARROW_UP, "Send", 16.0)
    } else {
        (icons::VOICE, "Start voice chat", 16.0)
    };
    view! {
        <div class="w-7 h-7 shrink-0 rounded-[14] items-center justify-center"
             bg={p.send_active} role="button" aria-label={label}
             on:click={if running { Msg::Stop } else { Msg::Send }}>
            <icon svg={svg} size={size} color={p.send_active_glyph} />
        </div>
    }
}

fn controls(app: &Codex, p: &Pal, density: Density) -> AnyElement {
    let (approval, _, approval_icon) = APPROVALS[app.approval];
    let full_access = app.approval == 2;
    let tint = if full_access { p.orange } else { p.muted };
    let full = density == Density::Full;
    let model_open = app.menu == Some(Menu::Model);
    let model_label = format!("{} {}", crate::MODEL, crate::EFFORTS[app.effort]);
    view! {
        <div class="flex-row items-center h-7 pl-2 pr-2 gap-[5]">
            <icon_button(p, icons::PLUS, 28.0, 16.0, p.text_soft, "Add files and more",
                         Msg::Open(Menu::Add)) id="pill.add" />
            <pill(p, "pill.permissions", "Change permissions", Msg::Open(Menu::Permissions),
                  app.menu == Some(Menu::Permissions))>
                <icon svg={approval_icon} size={15.0} color={tint} />
                if full {
                    <txt(approval, SMALL, tint) />
                }
            </pill>
            <div class="flex-1" />
            <pill(p, "pill.model", &model_label, Msg::Open(Menu::Model), model_open)
                  @when {full && model_open} { class="w-[146] justify-center" }
                  @when {full && !model_open} { class="gap-1" }>
                if !full {
                    <icon svg={icons::BRAIN} size={15.0} color={p.muted} />
                } else if model_open {
                    <txt("Select effort", SMALL, p.muted) />
                    <div class="w-[18]" />
                    <icon svg={icons::CHEVRON_DOWN} size={11.0} color={p.muted} />
                } else {
                    <txt(crate::MODEL, SMALL, p.text_soft) />
                    <txt(crate::EFFORTS[app.effort], SMALL, p.muted) />
                    <div class="w-0.5" />
                    <icon svg={icons::CHEVRON_DOWN} size={11.0} color={p.muted} />
                }
            </pill>
            <icon_button(p, icons::MIC, 28.0, 15.0, p.text_soft, "Dictate", Msg::Noop) />
            <div class="w-2" />
            {send_button(app, p)}
        </div>
    }
}

/// The floating composer pill of the full view (u29).
pub fn compact(app: &mut Codex, p: &Pal, w: f32, vcx: &mut ViewContext) -> AnyElement {
    let text_w = (w - 200.0).max(80.0);
    app.composer.fit(vcx, text_w, LINE_H);
    let scroll = ScrollActionBuilder::new(|lines| Msg::ComposerScroll(lines).into());
    view! {
        <div class="flex-row items-center h-11 pl-2 pr-2 gap-1 rounded-[22]" w={w}
             bg={p.composer} border={p.composer_border} id={CARD_ID}>
            <icon_button(p, icons::PLUS, 28.0, 16.0, p.text_soft, "Add files and more",
                         Msg::Open(Menu::Add)) />
            <div class="pl-1">
                <text_editor_element(COMPOSER_FOCUS, scroll)
                    editor_snapshot={&app.composer.editor} label="Work with Codex"
                    placeholder="Work with Codex" focused={vcx.is_focused(COMPOSER_FOCUS)}
                    font_size={BODY} text_color={p.text} w={text_w} h={LINE_H} />
            </div>
            <div class="flex-1" />
            <icon_button(p, icons::BRAIN, 28.0, 15.0, p.muted, "Select effort",
                         Msg::Open(Menu::Model)) />
            <icon_button(p, icons::HAND, 28.0, 15.0, p.muted, "Change permissions",
                         Msg::Open(Menu::Permissions)) />
            <icon_button(p, icons::MIC, 28.0, 15.0, p.text_soft, "Dictate", Msg::Noop) />
            <div class="w-1" />
            {send_button(app, p)}
        </div>
    }
}

/// The tray attached above the home composer: project, where to work,
/// and the local environment's settings.
pub fn tray(app: &Codex, p: &Pal, w: f32) -> AnyElement {
    let item = |id: &str, svg: &'static str, label: &str, msg: Msg| {
        view! {
            <div class="flex-row items-center h-7 px-1.5 gap-[7] rounded-[7]" hover_bg={p.menu_hi}
                 id={id.to_owned()} role="button" aria-label={label.to_owned()} on:click={msg}>
                <icon svg={svg} size={15.0} color={p.text_soft} />
                <txt(label, SMALL, p.text_soft) />
            </div>
        }
    };
    let project = app
        .project
        .and_then(|id| app.data.project(id))
        .map_or("No project", |p| p.name);
    view! {
        <div w={w} class="h-[42]" bg={p.tray} rounded_corners={[12.0, 12.0, 0.0, 0.0]}>
            <div class="flex-row items-center h-[38] pl-1 pr-1 gap-[10]" w={w}>
                {item("tray.project", icons::FOLDER, project, Msg::Open(Menu::ProjectPicker))}
                {item("tray.location", icons::LAPTOP, "This computer", Msg::Open(Menu::WorkIn))}
                <div class="flex-1" />
                <icon_button(p, icons::SETTINGS, 28.0, 15.0, p.muted,
                             "Configure local environment", Msg::Noop) />
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::{State, Suggest};

    // Catches the slash and `@` lists opening on the wrong drafts: a slash
    // only counts at the start and until a space, `@` only as the last word.
    #[test]
    fn draft_opens_the_matching_suggestion_list() {
        let cases = [
            ("/", Some(Suggest::Slash(String::new()))),
            ("/Co", Some(Suggest::Slash("co".into()))),
            ("/code review", None),
            ("fix @car", Some(Suggest::At("car".into()))),
            ("@cart.js then", None),
            ("plain text", None),
            ("a/b", None),
        ];
        for (draft, want) in cases {
            let mut state = State::new();
            state.editor.set_text(draft);
            assert_eq!(state.suggest(), want, "{draft:?}");
        }
    }

    // Catches picking from the filtered list: the index is into the
    // matches, not the whole table.
    #[test]
    fn picking_a_filtered_command_fills_its_name() {
        let mut state = State::new();
        state.editor.set_text("/p");
        state.pick(1);
        assert_eq!(state.editor.text(), "/plan mode ");
    }
}
