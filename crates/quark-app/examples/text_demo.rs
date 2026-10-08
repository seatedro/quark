//! Rich text, spelling, themes, fonts, and locales in one window:
//!
//! - The editor formats text: the toolbar or Mod+B, Mod+I, Mod+U,
//!   Mod+Shift+X, and Mod+E toggle bold, italic, underline, strikethrough,
//!   and code. Copies carry the formatting as Markdown or HTML.
//! - "Spelling" checks the editor against the OS's Hunspell dictionary for
//!   the locale (none is bundled). Right click a marked word for
//!   corrections or to add it to the dictionary.
//! - "Theme" cycles quark's own theme, a sample theme, and any theme files
//!   in `$QUARK_THEMES_DIR` (default `~/.config/quark/themes`); "Mode"
//!   follows the OS or pins light or dark. Fonts, ligatures, and text size
//!   change live.
//! - "Locale" cycles English, German, Japanese, and Arabic: quark's own
//!   labels, a plural message, numbers, and dates follow it, and Arabic
//!   lays rows out right to left.

use std::path::PathBuf;

use accesskit::Role;
use quark::view;
use quark_app::quark_ui::element::{
    AnyElement, Binding, IntoAnyElement, ScrollActionBuilder, div, text,
};
use quark_app::quark_ui::i18n::{self, Arg, Localizer};
use quark_app::quark_ui::style::{Styled, set_layout_direction};
use quark_app::quark_ui::text_input::{
    Editor, InlineStyle, RichExport, SpellChecker, SpellDictionary, TextEditCommand,
    TextEditOutcome, text_editor_element,
};
use quark_app::quark_ui::theme::{Theme, ThemeFamily, ThemeMode, ThemeRegistry};
use quark_app::quark_ui::{Action, FocusId};
use quark_app::winit::event::{ElementState, MouseButton};
use quark_app::{InputEvent, UiApp, UiContext, ViewContext, WindowOptions};
use quark_components::{ContextMenuEntry, ContextMenuOutcome, ContextMenuState};
use quark_text::FontSettings;
use quark_text::fonts::{FontRole, bundled_font_options};

const INPUT: FocusId = FocusId::from_key("text.editor");
const PAD: f32 = 16.0;
const ROW_H: f32 = 28.0;
const GAP: f32 = 8.0;
/// Toolbar rows above the editor.
const ROWS: f32 = 5.0;
const EDITOR_Y: f32 = PAD + ROWS * (ROW_H + GAP);

const LOCALES: [&str; 4] = ["en-US", "de", "ja", "ar"];

/// The app's own messages; quark's come with quark-i18n.
const MESSAGES: [(&str, &str); 4] = [
    (
        "en-US",
        "words = { $count ->\n    [one] { $count } word\n   *[other] { $count } words\n}\n",
    ),
    (
        "de",
        "words = { $count ->\n    [one] { $count } Wort\n   *[other] { $count } Wörter\n}\n",
    ),
    ("ja", "words = { $count } 語\n"),
    (
        "ar",
        "words = { $count ->\n    [zero] لا كلمات\n    [one] كلمة واحدة\n    [two] كلمتان\n    [few] { $count } كلمات\n   *[other] { $count } كلمة\n}\n",
    ),
];

const SAMPLE_THEME: &str = r##"{
  "name": "Harbor",
  "dark": {
    "colors": {
      "background": "#0f1419", "surface": "#131a21", "editor_surface": "#0b0e12",
      "text": "#d9d7ce", "text_muted": "#6c7380", "accent": "#59c2ff",
      "text_accent": "#73d0ff", "border_soft": "#1f2630", "element_background": "#1b222b"
    }
  },
  "light": {
    "colors": {
      "background": "#fafafa", "surface": "#f0f0f0", "text": "#3d424d",
      "accent": "#399ee6", "text_accent": "#2b7fc0"
    }
  }
}"##;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Edit(TextEditCommand),
    CopyFormat,
    Spelling,
    NextTheme,
    NextMode,
    NextFont(FontRole),
    Ligatures,
    Size(f32),
    NextLocale,
    Replace(usize, String),
    Learn(String),
    Scroll(i32),
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

/// Whether the theme follows the OS or is pinned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    System,
    Light,
    Dark,
}

struct TextDemo {
    editor: Editor,
    export: RichExport,
    menu: ContextMenuState,
    /// The checker while spelling is on, and why it is off otherwise.
    spelling: Result<Option<SpellChecker>, String>,
    themes: ThemeRegistry,
    theme_index: usize,
    mode: Mode,
    fonts: FontSettings,
    font_size: f32,
    locale: usize,
    /// A spelling checker's wake for the window, set at init.
    waker: Option<quark_app::Waker>,
}

fn themes_dir() -> Option<PathBuf> {
    std::env::var_os("QUARK_THEMES_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/quark/themes")))
}

impl TextDemo {
    fn new() -> Self {
        let mut themes = ThemeRegistry::default();
        if let Ok(sample) = ThemeFamily::from_json(SAMPLE_THEME) {
            themes.insert(sample);
        }
        if let Some(dir) = themes_dir() {
            for error in themes.load_dir(&dir) {
                eprintln!("{error}");
            }
        }
        let mut editor = Editor::default();
        editor.set_text("Select some text and press Mod+B. Teh spelling checker marks typos.");
        Self {
            editor,
            export: RichExport::Markdown,
            menu: ContextMenuState::default(),
            spelling: Ok(None),
            themes,
            theme_index: 0,
            mode: Mode::System,
            fonts: FontSettings::default(),
            font_size: 15.0,
            locale: 0,
            waker: None,
        }
    }

    fn family(&self) -> &ThemeFamily {
        &self.themes.families()[self.theme_index % self.themes.families().len()]
    }

    /// Hand the current family, mode, and text size to the adapter.
    fn apply_theme(&mut self, cx: &mut UiContext) {
        let sized = |mut theme: Theme| {
            theme.metrics.ui_font_size = self.font_size;
            theme.metrics.ui_small_font_size = self.font_size - 2.0;
            theme.metrics.mono_font_size = self.font_size - 1.0;
            theme
        };
        let family = self.family();
        let (light, dark) = (
            sized(family.theme(ThemeMode::Light)),
            sized(family.theme(ThemeMode::Dark)),
        );
        match self.mode {
            Mode::System => cx.set_themes(light, dark),
            Mode::Light => cx.set_theme(light),
            Mode::Dark => cx.set_theme(dark),
        }
        self.editor.set_font_size(self.font_size);
    }

    fn apply_locale(&mut self) {
        let locale = LOCALES[self.locale % LOCALES.len()];
        let mut builder = Localizer::builder().requested([locale]);
        for (tag, ftl) in MESSAGES {
            builder = builder.messages(tag, ftl);
        }
        let localizer = builder.build().unwrap_or_else(|_| Localizer::english());
        set_layout_direction(localizer.direction());
        i18n::install(localizer);
        if self.spelling.as_ref().is_ok_and(Option::is_some) {
            self.toggle_spelling();
            self.toggle_spelling();
        }
    }

    fn toggle_spelling(&mut self) {
        if let Ok(Some(_)) = self.spelling {
            self.spelling = Ok(None);
            self.editor.set_spellcheck(None);
            return;
        }
        let locale = i18n::current().locale().to_string();
        let checker = SpellDictionary::find(&locale, &SpellDictionary::system_dirs())
            .map_err(|e| e.to_string())
            .and_then(|dictionary| {
                let waker = self.waker.clone();
                SpellChecker::spawn(dictionary.into(), move || {
                    if let Some(waker) = &waker {
                        waker.wake();
                    }
                })
                .map_err(|e| e.to_string())
            });
        match checker {
            Ok(checker) => {
                self.editor.set_spellcheck(Some(checker.clone()));
                self.spelling = Ok(Some(checker));
            }
            Err(why) => self.spelling = Err(why),
        }
    }

    /// Corrections for the misspelled word under a right click.
    fn open_spelling_menu(&mut self, (x, y): (f32, f32)) -> bool {
        let Some(layout) = self.editor.layout() else {
            return false;
        };
        let at = layout
            .hit(x - PAD, y - EDITOR_Y + self.editor.scroll_y)
            .get();
        let Some(issue) = self.editor.spelling_at(at) else {
            return false;
        };
        let mut entries: Vec<ContextMenuEntry> = issue
            .suggestions
            .iter()
            .take(5)
            .map(|s| ContextMenuEntry::item(s.clone(), Msg::Replace(at, s.clone())))
            .collect();
        if entries.is_empty() {
            entries.push(
                ContextMenuEntry::item(i18n::tr("quark-spelling-no-suggestions"), Msg::Size(0.0))
                    .disabled(),
            );
        }
        entries.push(ContextMenuEntry::separator());
        entries.push(ContextMenuEntry::item(
            i18n::tr("quark-spelling-add"),
            Msg::Learn(issue.word),
        ));
        self.menu.open(entries, x, y);
        true
    }

    fn button(label: impl Into<String>, msg: Msg, on: bool, theme: &Theme) -> AnyElement {
        let label = label.into();
        let colors = &theme.colors;
        view! {
            <div accessibility_role={Role::Button} aria-label={label.clone()} on:click={msg}
                 class="px-[10] h-[ROW_H] items-center justify-center rounded-[6]"
                 bg={if on { colors.accent } else { colors.element_background }}
                 hover_bg={colors.element_hover}>
                <text class="text-sm" color={if on { colors.text_strong } else { colors.text }}>
                    {label}
                </text>
            </div>
        }
    }

    fn row(children: Vec<AnyElement>) -> AnyElement {
        view! {
            <div class="flex-row items-center gap-[GAP] h-[ROW_H]">{...children}</div>
        }
    }

    fn toolbar(&self, theme: &Theme) -> Vec<AnyElement> {
        let format = self.editor.typing_format();
        let toggle = |label: &str, style: InlineStyle| {
            let msg = Msg::Edit(TextEditCommand::ToggleStyle(style));
            Self::button(label, msg, format.style.contains(style), theme)
        };
        let link = Msg::Edit(TextEditCommand::SetLink(Some("https://example.com".into())));
        let export = match self.export {
            RichExport::Markdown => "Copy as Markdown",
            RichExport::Html => "Copy as HTML",
        };
        let spelling = match &self.spelling {
            Ok(Some(_)) => "Spelling: on".to_owned(),
            Ok(None) => "Spelling: off".to_owned(),
            Err(why) => format!("Spelling: {why}"),
        };
        let mode = match self.mode {
            Mode::System => "Mode: system",
            Mode::Light => "Mode: light",
            Mode::Dark => "Mode: dark",
        };
        let localizer = i18n::current();
        let count = self.editor.text().split_whitespace().count();
        let words = i18n::tr_args("words", [("count", Arg::from(count))]);
        let sample = format!(
            "{words} · {} · {} · {}",
            localizer.number(1234567.891, 2),
            localizer.date(2026, 10, 7),
            i18n::tr("quark-find"),
        );
        let colors = &theme.colors;
        vec![
            Self::row(vec![
                toggle("B", InlineStyle::BOLD),
                toggle("I", InlineStyle::ITALIC),
                toggle("U", InlineStyle::UNDERLINE),
                toggle("S", InlineStyle::STRIKE),
                toggle("Code", InlineStyle::CODE),
                Self::button("Link", link, format.link.is_some(), theme),
                Self::button(export, Msg::CopyFormat, false, theme),
            ]),
            Self::row(vec![
                Self::button(
                    format!("Theme: {}", self.family().name),
                    Msg::NextTheme,
                    false,
                    theme,
                ),
                Self::button(mode, Msg::NextMode, false, theme),
            ]),
            Self::row(vec![
                Self::button(
                    format!("UI: {}", self.fonts.ui_family),
                    Msg::NextFont(FontRole::Ui),
                    false,
                    theme,
                ),
                Self::button(
                    format!("Code: {}", self.fonts.mono_family),
                    Msg::NextFont(FontRole::Mono),
                    false,
                    theme,
                ),
                Self::button("Ligatures", Msg::Ligatures, self.fonts.ligatures, theme),
                Self::button("A-", Msg::Size(-1.0), false, theme),
                Self::button("A+", Msg::Size(1.0), false, theme),
            ]),
            Self::row(vec![
                Self::button(
                    format!("Locale: {}", i18n::current().locale()),
                    Msg::NextLocale,
                    false,
                    theme,
                ),
                Self::button(spelling, Msg::Spelling, false, theme),
            ]),
            Self::row(vec![view! {
                <text class="text-sm" color={colors.text_muted}>{sample}</text>
            }]),
        ]
    }
}

impl UiApp for TextDemo {
    type Action = Msg;
    type Message = ();

    fn init(&mut self, cx: &mut UiContext) {
        self.waker = Some(cx.window.waker().clone());
        self.apply_locale();
        self.apply_theme(cx);
        cx.set_focus(Some(INPUT));
    }

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let (width, height) = cx.frame.size();
        let theme = cx.theme;
        let colors = &theme.colors;
        let editor_w = (width - 2.0 * PAD).max(100.0);
        let editor_h = (height - EDITOR_Y - PAD).max(60.0);
        self.editor.set_clock(cx.frame.elapsed().as_millis() as u64);
        self.editor.sync_size(editor_w, editor_h);
        self.editor.flush(&mut cx.frame.text().system);

        view! {
            <div w={width} h={height} class="flex-col p-[PAD] gap-[GAP] bg-[colors.background]">
                {...self.toolbar(theme)}
                <div class="bg-[colors.editor_surface] rounded-[8]">
                    <text_editor_element(
                        INPUT,
                        ScrollActionBuilder::new(|lines| Msg::Scroll(lines).into()),
                    )
                        editor_snapshot={&self.editor}
                        placeholder="Write something"
                        focused={cx.is_focused(INPUT)}
                        text_color={colors.text}
                        font_size={self.font_size}
                        w={editor_w}
                        h={editor_h} />
                </div>
                if let Some(menu) = self.menu.render((width, height), theme) {
                    {menu}
                }
            </div>
        }
    }

    fn update(&mut self, msg: Msg, cx: &mut UiContext) {
        self.menu.close();
        match msg {
            Msg::Edit(command) => drop(self.editor.apply(command)),
            Msg::CopyFormat => {
                self.export = match self.export {
                    RichExport::Markdown => RichExport::Html,
                    RichExport::Html => RichExport::Markdown,
                };
                self.editor.set_rich_export(self.export);
            }
            Msg::Spelling => self.toggle_spelling(),
            Msg::NextTheme => {
                self.theme_index = (self.theme_index + 1) % self.themes.families().len();
                self.apply_theme(cx);
            }
            Msg::NextMode => {
                self.mode = match self.mode {
                    Mode::System => Mode::Light,
                    Mode::Light => Mode::Dark,
                    Mode::Dark => Mode::System,
                };
                self.apply_theme(cx);
            }
            Msg::NextFont(role) => {
                let options = bundled_font_options(role);
                let current = match role {
                    FontRole::Ui => &mut self.fonts.ui_family,
                    FontRole::Mono => &mut self.fonts.mono_family,
                };
                let at = options.iter().position(|o| o.family == current.as_str());
                let next = options[at.map_or(0, |i| (i + 1) % options.len())];
                *current = next.family.to_owned();
                cx.set_fonts(&self.fonts);
            }
            Msg::Ligatures => {
                self.fonts.ligatures = !self.fonts.ligatures;
                cx.set_fonts(&self.fonts);
            }
            Msg::Size(step) => {
                self.font_size = (self.font_size + step).clamp(10.0, 28.0);
                self.apply_theme(cx);
            }
            Msg::NextLocale => {
                self.locale = (self.locale + 1) % LOCALES.len();
                self.apply_locale();
            }
            Msg::Replace(at, with) => {
                if let Some(issue) = self.editor.spelling_at(at) {
                    let insertion = quark_app::quark_ui::text_input::Insertion::Text(with);
                    self.editor.replace_range(issue.range, &insertion);
                }
            }
            Msg::Learn(word) => self.editor.learn_word(&word),
            Msg::Scroll(lines) => self
                .editor
                .scroll(lines as f32 * self.editor.scroll_line_height_px()),
        }
        cx.set_focus(Some(INPUT));
        cx.window.request_redraw();
    }

    fn wake(&mut self, cx: &mut UiContext) {
        // A spelling result arrived.
        cx.window.request_redraw();
    }

    fn edit_text(&mut self, target: FocusId, command: TextEditCommand) -> TextEditOutcome {
        if target == INPUT {
            self.editor.apply(command)
        } else {
            TextEditOutcome::default()
        }
    }

    fn set_preedit(&mut self, target: FocusId, text: String, cursor: Option<(usize, usize)>) {
        if target == INPUT {
            self.editor.set_preedit(text, cursor);
        }
    }

    fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        let taken = match event {
            InputEvent::PointerButton {
                button: MouseButton::Right,
                state: ElementState::Pressed,
            } => cx
                .window
                .pointer_position()
                .is_some_and(|at| self.open_spelling_menu(at)),
            InputEvent::PointerMoved { x, y } => {
                if self.menu.pointer_moved(*x, *y) {
                    cx.window.request_redraw();
                }
                false
            }
            InputEvent::KeyPress(chord) => {
                let Some(binding): Option<Binding> = chord.binding() else {
                    return false;
                };
                match self.menu.handle_key(&binding) {
                    Some(ContextMenuOutcome::Activate(action)) => {
                        if let Some(msg) = action.downcast_ref::<Msg>() {
                            self.update(msg.clone(), cx);
                        }
                        true
                    }
                    Some(_) => true,
                    None => false,
                }
            }
            _ => false,
        };
        if taken {
            cx.window.request_redraw();
        }
        taken
    }
}

fn main() -> Result<(), quark_app::RunError> {
    quark_app::run_ui(
        TextDemo::new(),
        WindowOptions {
            title: "Text".into(),
            size: (820.0, 560.0),
            ..WindowOptions::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard};

    use quark_app::testing::{By, UiTestHarness};

    use super::*;

    /// The locale is process-wide, so these tests take turns.
    fn harness() -> (MutexGuard<'static, ()>, UiTestHarness<TextDemo>) {
        static SERIAL: Mutex<()> = Mutex::new(());
        let turn = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        (
            turn,
            UiTestHarness::new(TextDemo::new(), (820.0, 560.0), 1.0),
        )
    }

    // Catches the style bindings not reaching the editor through the
    // adapter's key routing, or copies losing the formatting.
    #[test]
    fn mod_b_bolds_the_selection_and_copies_as_markdown() {
        let (_turn, mut ui) = harness();
        ui.key("mod+a");
        ui.key("backspace");
        ui.type_text("plain ");
        ui.key("mod+b");
        ui.type_text("bold");
        ui.key("mod+a");
        ui.key("mod+c");
        assert_eq!(ui.clipboard_text().as_deref(), Some("plain **bold**"));
    }

    // Catches a locale switch that leaves quark's own labels or the
    // layout direction behind.
    #[test]
    fn the_locale_button_relabels_and_mirrors() {
        let (_turn, mut ui) = harness();
        for _ in 0..3 {
            ui.click_node(By::role_name(
                Role::Button,
                format!("Locale: {}", i18n::current().locale()),
            ));
        }
        assert_eq!(i18n::tr("quark-find"), "بحث");
        assert_eq!(
            quark_app::quark_ui::style::layout_direction(),
            quark_app::quark_ui::style::Direction::RightToLeft
        );
        ui.find(By::role_name(Role::Button, "Locale: ar"));
    }
}
