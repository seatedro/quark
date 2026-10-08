//! Form controls from quark-components: a select with grouped and
//! disabled options, a combobox whose options load asynchronously, a radio
//! group, a segmented control, a switch, a slider, and a theme select at
//! the bottom of the window whose list opens upward; its High contrast
//! choice follows the desktop's light or dark preference. Tab moves between
//! controls; inside a radio group or segmented control the arrow keys move
//! the choice. Escape closes an open list, or quits when none is open.
//! The layout and the controls are written with `view!`.

use std::rc::Rc;

use quark::view;
use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, div, text};
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome};
use quark_app::quark_ui::theme::Theme;
use quark_app::quark_ui::{Action, FocusId};
use quark_app::winit::keyboard::NamedKey;
use quark_app::{InputEvent, UiApp, UiContext, UiSender, ViewContext, WindowOptions};
use quark_components::{
    ComboboxMsg, ComboboxState, RadioOption, SegmentedControl, SegmentedItem, SelectMsg,
    SelectOption, SelectState, Switch, combobox, radio_focus_id, radio_group, segmented_focus_id,
    select, slider,
};

const CITIES: [&str; 12] = [
    "Amsterdam",
    "Athens",
    "Berlin",
    "Buenos Aires",
    "Cairo",
    "Lisbon",
    "London",
    "Los Angeles",
    "Madrid",
    "New York",
    "San Francisco",
    "Santiago",
];
const SIZES: [&str; 4] = ["Small", "Medium", "Large", "Huge"];
const VIEWS: [&str; 3] = ["Day", "Week", "Month"];

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Fruit(SelectMsg),
    Theme(SelectMsg),
    City(ComboboxMsg),
    Size(usize),
    View(usize),
    Wifi,
    Volume(f32),
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

/// Options the city combobox's loader found for a request.
struct Loaded {
    generation: u64,
    options: Vec<String>,
}

struct ControlsDemo {
    fruits: Rc<[SelectOption]>,
    fruit: SelectState,
    themes: Rc<[SelectOption]>,
    theme: SelectState,
    city: ComboboxState,
    size: Option<usize>,
    view: usize,
    wifi: bool,
    volume: f32,
    /// The input clock, for type-ahead and the field's undo grouping.
    now_ms: u64,
    sender: Option<UiSender<Loaded>>,
}

impl ControlsDemo {
    fn new() -> Self {
        let fruits: Rc<[SelectOption]> = [
            ("Apple", "Pome"),
            ("Pear", "Pome"),
            ("Quince", "Pome"),
            ("Banana", "Tropical"),
            ("Cherimoya", "Tropical"),
            ("Mango", "Tropical"),
            ("Cherry", "Stone"),
            ("Plum", "Stone"),
        ]
        .into_iter()
        .map(|(label, group)| {
            SelectOption::new(label)
                .group(group)
                .disabled(label == "Quince")
        })
        .collect();
        let themes: Rc<[SelectOption]> = ["System", "Light", "Dark", "High contrast"]
            .into_iter()
            .map(SelectOption::new)
            .collect();
        let mut fruit = SelectState::new("controls.fruit");
        fruit.set_selected(Some(0));
        let mut theme = SelectState::new("controls.theme");
        theme.set_selected(Some(0));
        Self {
            fruits,
            fruit,
            themes,
            theme,
            city: ComboboxState::new_async("controls.city"),
            size: Some(1),
            view: 0,
            wifi: true,
            volume: 40.0,
            now_ms: 0,
            sender: None,
        }
    }

    /// Hand a pending city query to the loader. A real app would search on
    /// a worker thread; this one answers at once, still through the
    /// sender, so results arrive as they would from a thread.
    fn load_cities(&mut self) {
        let (Some(request), Some(sender)) = (self.city.take_request(), &self.sender) else {
            return;
        };
        let query = request.query.to_lowercase();
        let options = CITIES
            .iter()
            .filter(|city| {
                let mut letters = city.to_lowercase().chars().collect::<Vec<_>>().into_iter();
                query.chars().all(|q| letters.any(|c| c == q))
            })
            .map(|city| city.to_string())
            .collect();
        sender.send(Loaded {
            generation: request.generation,
            options,
        });
    }

    fn row(label: &str, control: impl IntoAnyElement, cx: &ViewContext) -> AnyElement {
        view! {
            <div class="flex-row items-center gap-4">
                <div class="w-24">
                    <text class="text-sm" color={cx.theme.colors.text_muted}>{label.to_owned()}</text>
                </div>
                {control}
            </div>
        }
    }
}

impl UiApp for ControlsDemo {
    type Action = Msg;
    type Message = Loaded;

    fn init(&mut self, cx: &mut UiContext) {
        self.sender = Some(cx.sender());
    }

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let viewport = cx.frame.size();
        let (width, height) = viewport;
        let colors = &cx.theme.colors;
        let fruit = view! {
            <select(&self.fruit, self.fruits.clone(), |m| Msg::Fruit(m).into())
                label="Fruit" viewport={viewport} />
        };
        let city = view! {
            <combobox(&self.city, "City", |m| Msg::City(m).into())
                placeholder="Search cities" viewport={viewport} />
        };
        let sizes = SIZES
            .iter()
            .map(|s| RadioOption::new(*s).disabled(*s == "Huge"))
            .collect();
        let size = view! {
            <radio_group("controls.size", "Size", sizes, self.size, |i| Msg::Size(i).into()) />
        };
        let views = VIEWS
            .iter()
            .enumerate()
            .map(|(i, v)| SegmentedItem::new(*v, Msg::View(i), i == self.view))
            .collect();
        let volume = view! {
            <slider("controls.volume", "Volume", self.volume, 0.0, 100.0, |v| Msg::Volume(v).into())
                step={5.0} />
        };
        let theme = view! {
            <select(&self.theme, self.themes.clone(), |m| Msg::Theme(m).into())
                label="Theme" viewport={viewport} />
        };
        // Builder values and markup mix freely: rows take either.
        view! {
            <div w={width} h={height} class="p-6 flex-col justify-between bg-[colors.background]">
                <div class="flex-col gap-[14]">
                    {Self::row("Fruit", fruit, cx)}
                    {Self::row("City", city, cx)}
                    {Self::row("Size", size, cx)}
                    {Self::row(
                        "View",
                        view! { <SegmentedControl(views) id="controls.view" /> },
                        cx,
                    )}
                    {Self::row(
                        "Network",
                        view! { <Switch on={self.wifi} label="Wi-Fi" on:toggle={Msg::Wifi} /> },
                        cx,
                    )}
                    {Self::row("Volume", volume, cx)}
                </div>
                {Self::row("Theme", theme, cx)}
            </div>
        }
    }

    fn update(&mut self, msg: Msg, cx: &mut UiContext) {
        let now_ms = cx.window.elapsed().as_millis() as u64;
        let focus = match msg {
            Msg::Fruit(m) => self.fruit.update(m, &self.fruits, now_ms).focus,
            Msg::Theme(m) => {
                let before = self.theme.selected();
                let focus = self.theme.update(m, &self.themes, now_ms).focus;
                if self.theme.selected() != before {
                    use quark_app::winit::window::Theme as WindowTheme;
                    let window_theme = match self.theme.selected() {
                        Some(1) => {
                            cx.set_theme(Theme::default_light());
                            Some(WindowTheme::Light)
                        }
                        Some(2) => {
                            cx.set_theme(Theme::default_dark());
                            Some(WindowTheme::Dark)
                        }
                        Some(3) => {
                            cx.set_themes(
                                Theme::high_contrast_light(),
                                Theme::high_contrast_dark(),
                            );
                            None
                        }
                        _ => {
                            cx.set_themes(Theme::default_light(), Theme::default_dark());
                            None
                        }
                    };
                    // The title bar follows a fixed choice; `None` hands it
                    // back to the desktop's preference.
                    if let Some(window) = cx.window.window() {
                        window.set_theme(window_theme);
                    }
                }
                focus
            }
            Msg::City(m) => self.city.update(m).focus,
            Msg::Size(i) => {
                self.size = Some(i);
                Some(radio_focus_id("controls.size", i))
            }
            Msg::View(i) => {
                self.view = i;
                Some(segmented_focus_id("controls.view", i))
            }
            Msg::Wifi => {
                self.wifi = !self.wifi;
                None
            }
            Msg::Volume(v) => {
                self.volume = v;
                None
            }
        };
        if let Some(focus) = focus {
            cx.set_focus(Some(focus));
        }
        cx.window.request_redraw();
    }

    fn message(&mut self, loaded: Loaded, cx: &mut UiContext) {
        if self.city.resolve_options(loaded.generation, loaded.options) {
            cx.window.request_redraw();
        }
    }

    fn edit_text(&mut self, target: FocusId, command: TextEditCommand) -> TextEditOutcome {
        if target != self.city.focus_id() {
            return TextEditOutcome::default();
        }
        let outcome = self.city.edit(command, self.now_ms);
        self.load_cities();
        outcome
    }

    fn set_preedit(&mut self, target: FocusId, text: String, cursor: Option<(usize, usize)>) {
        if target == self.city.focus_id() {
            self.city.set_preedit(text, cursor);
        }
    }

    fn set_text_value(&mut self, target: FocusId, value: String, _cx: &mut UiContext) {
        if target == self.city.focus_id() {
            self.city.set_text(value);
            self.load_cities();
        }
    }

    fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        self.now_ms = cx.window.elapsed().as_millis() as u64;
        let open = self.fruit.is_open() || self.theme.is_open() || self.city.is_open();
        match event {
            InputEvent::KeyPress(chord) if chord.named() == Some(NamedKey::Escape) && !open => {
                cx.window.exit();
                true
            }
            _ => false,
        }
    }
}

fn main() -> Result<(), quark_app::RunError> {
    quark_app::run_ui(
        ControlsDemo::new(),
        WindowOptions {
            title: "Quark Controls".into(),
            size: (640.0, 560.0),
            ..WindowOptions::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use accesskit::Role;
    use quark_app::quark_ui::accessibility::dump_accessibility_states;
    use quark_app::testing::{By, Node, UiTestHarness};

    use quark_app::quark_ui::theme::{Color, ThemeContrast, ThemeMode, contrast_ratio};
    use quark_app::testing::Pixels;

    use super::*;

    fn demo() -> UiTestHarness<ControlsDemo> {
        UiTestHarness::new(ControlsDemo::new(), (640.0, 560.0), 1.0)
    }

    fn fruit(ui: &UiTestHarness<ControlsDemo>) -> Node {
        ui.find(By::role_name(Role::ComboBox, "Fruit"))
    }

    fn focused_name(ui: &UiTestHarness<ControlsDemo>) -> Option<String> {
        ui.focused().and_then(|node| node.name)
    }

    /// `role | name | states` lines of the nodes whose line contains
    /// `filter`, without author ids.
    fn states(ui: &UiTestHarness<ControlsDemo>, filter: &str) -> String {
        dump_accessibility_states(&ui.accessibility_update())
            .lines()
            .filter_map(|line| line.split_once(" | ").map(|(_, rest)| rest))
            .filter(|line| line.contains(filter))
            .map(|line| format!("{line}\n"))
            .collect()
    }

    /// Focus the Fruit select from the keyboard: it is the first Tab stop.
    fn tab_to_fruit(ui: &mut UiTestHarness<ControlsDemo>) {
        ui.key("tab");
        assert_eq!(focused_name(ui).as_deref(), Some("Fruit"));
    }

    #[test]
    fn select_opens_moves_past_disabled_options_and_commits_from_the_keyboard() {
        let mut ui = demo();
        tab_to_fruit(&mut ui);
        ui.key("enter");
        // Opening highlights and focuses the chosen option.
        assert_eq!(focused_name(&ui).as_deref(), Some("Apple"));
        // Quince is disabled: Down goes Pear, then Banana.
        ui.key("arrowdown");
        ui.key("arrowdown");
        assert_eq!(focused_name(&ui).as_deref(), Some("Banana"));
        ui.key("enter");
        assert_eq!(fruit(&ui).value.as_deref(), Some("Banana"));
        assert!(ui.try_find(By::role(Role::ListBox)).is_none());
        assert_eq!(focused_name(&ui).as_deref(), Some("Fruit"));
    }

    #[test]
    fn typing_on_a_closed_select_jumps_to_the_matching_option() {
        let mut ui = demo();
        tab_to_fruit(&mut ui);
        // "ch" prefers Cherimoya (first "ch") over Cherry; a pause and "c"
        // again cycles to the next "c" option.
        ui.type_text("ch");
        assert_eq!(fruit(&ui).value.as_deref(), Some("Cherimoya"));
        ui.advance(1500);
        ui.type_text("c");
        assert_eq!(fruit(&ui).value.as_deref(), Some("Cherry"));
        assert!(ui.try_find(By::role(Role::ListBox)).is_none());
    }

    #[test]
    fn escape_closes_the_list_and_returns_focus_to_the_select() {
        let mut ui = demo();
        ui.click_node(By::role_name(Role::ComboBox, "Fruit"));
        assert!(ui.try_find(By::role(Role::ListBox)).is_some());
        ui.key("arrowdown");
        ui.key("escape");
        assert!(ui.try_find(By::role(Role::ListBox)).is_none());
        assert_eq!(focused_name(&ui).as_deref(), Some("Fruit"));
        assert_eq!(fruit(&ui).value.as_deref(), Some("Apple"));
    }

    #[test]
    fn a_click_on_an_option_chooses_it() {
        let mut ui = demo();
        ui.click_node(By::role_name(Role::ComboBox, "Fruit"));
        ui.click_node(By::role_name(Role::ListBoxOption, "Plum"));
        assert_eq!(fruit(&ui).value.as_deref(), Some("Plum"));
        assert_eq!(focused_name(&ui).as_deref(), Some("Fruit"));
    }

    #[test]
    fn a_list_opens_below_its_select_and_flips_above_at_the_window_bottom() {
        let mut ui = demo();
        ui.click_node(By::role_name(Role::ComboBox, "Fruit"));
        let trigger = fruit(&ui).bounds;
        let list = ui.find(By::role(Role::ListBox)).bounds;
        assert!(
            list.y >= trigger.y + trigger.height,
            "{list:?} below {trigger:?}"
        );
        ui.key("escape");

        ui.click_node(By::role_name(Role::ComboBox, "Theme"));
        let trigger = ui.find(By::role_name(Role::ComboBox, "Theme")).bounds;
        let list = ui.find(By::role(Role::ListBox)).bounds;
        assert!(
            list.y + list.height <= trigger.y,
            "{list:?} above {trigger:?}"
        );
        assert!(list.y >= 0.0);
        // The flipped list is where clicks land.
        ui.click_node(By::role_name(Role::ListBoxOption, "Dark"));
        assert_eq!(
            ui.find(By::role_name(Role::ComboBox, "Theme"))
                .value
                .as_deref(),
            Some("Dark")
        );
    }

    fn choose_theme(ui: &mut UiTestHarness<ControlsDemo>, name: &str) {
        ui.click_node(By::role_name(Role::ComboBox, "Theme"));
        ui.click_node(By::role_name(Role::ListBoxOption, name));
    }

    #[test]
    fn choosing_a_theme_repaints_in_it() {
        let mut ui = demo();
        let cases = [
            ("Light", ThemeMode::Light, ThemeContrast::Standard),
            ("Dark", ThemeMode::Dark, ThemeContrast::Standard),
            // The harness's desktop has no preference, which reads as dark.
            ("High contrast", ThemeMode::Dark, ThemeContrast::High),
        ];
        for (name, mode, contrast) in cases {
            choose_theme(&mut ui, name);
            let theme = ui.theme();
            assert_eq!(
                (theme.mode, theme.contrast),
                (mode, contrast),
                "after {name}"
            );
        }
    }

    /// The physical pixel under point `(x, y)` at the harness's scale 2.
    fn pixel_at(pixels: &Pixels, (x, y): (f32, f32)) -> Color {
        let [r, g, b, _] = pixels.pixel((x * 2.0) as u32, (y * 2.0) as u32);
        Color::rgba(r, g, b, 255)
    }

    // Catches a switch to high contrast leaving cached controls in the old
    // colors, or a focus ring or highlighted option fading into what is
    // behind it. Measured on rendered pixels: the ring two points outside
    // the focused control against the page or list just beyond it, and the
    // highlighted option's fill against an unhighlighted one's.
    #[test]
    fn high_contrast_focus_rings_and_highlighted_options_stay_visible() {
        let mut ui = UiTestHarness::new(ControlsDemo::new(), (640.0, 560.0), 2.0);
        choose_theme(&mut ui, "High contrast");
        let theme = ui.theme().clone();
        assert_eq!(focused_name(&ui).as_deref(), Some("Theme"));
        let render = |ui: &mut UiTestHarness<ControlsDemo>| match ui.render_rgba() {
            Ok(pixels) => Some(pixels),
            Err(quark_render::RenderError::NoAdapter) => {
                assert!(
                    std::env::var_os("QUARK_REQUIRE_GPU").is_none(),
                    "QUARK_REQUIRE_GPU is set but no wgpu adapter is available"
                );
                eprintln!("skipping: no wgpu adapter available");
                None
            }
            Err(error) => panic!("headless render failed: {error}"),
        };
        let Some(pixels) = render(&mut ui) else {
            return;
        };
        let trigger = ui.find(By::role_name(Role::ComboBox, "Theme")).bounds;
        let mid = trigger.x + trigger.width / 2.0;
        let ring = pixel_at(&pixels, (mid, trigger.y - 1.0));
        let page = pixel_at(&pixels, (mid, trigger.y - 4.0));
        assert_eq!(ring, theme.colors.focus_border, "the trigger's ring");
        assert!(contrast_ratio(ring, page) >= 3.0, "{ring:?} on {page:?}");

        // Opening Fruit highlights and focuses its chosen option, Apple.
        ui.click_node(By::role_name(Role::ComboBox, "Fruit"));
        let Some(pixels) = render(&mut ui) else {
            return;
        };
        let apple = ui.find(By::role_name(Role::ListBoxOption, "Apple")).bounds;
        let pear = ui.find(By::role_name(Role::ListBoxOption, "Pear")).bounds;
        let ring = pixel_at(&pixels, (apple.x - 1.0, apple.y + apple.height / 2.0));
        let list = pixel_at(&pixels, (apple.x - 4.0, apple.y + apple.height / 2.0));
        assert_eq!(ring, theme.colors.focus_border, "the option's ring");
        assert!(contrast_ratio(ring, list) >= 3.0, "{ring:?} on {list:?}");
        let fill = |b: quark::Rect| pixel_at(&pixels, (b.x + 3.0, b.y + b.height / 2.0));
        let (highlighted, plain) = (fill(apple), fill(pear));
        assert_eq!(highlighted, theme.colors.ghost_element_selected);
        assert!(
            contrast_ratio(highlighted, plain) >= 1.3,
            "{highlighted:?} beside {plain:?}"
        );
    }

    #[test]
    fn radio_arrows_move_the_choice_and_focus_and_the_group_is_one_tab_stop() {
        let mut ui = demo();
        ui.click_node(By::role_name(Role::RadioButton, "Medium"));
        ui.key("arrowdown");
        assert_eq!(focused_name(&ui).as_deref(), Some("Large"));
        // Huge is disabled, so Down wraps to Small.
        ui.key("arrowdown");
        assert_eq!(focused_name(&ui).as_deref(), Some("Small"));
        // The size radios (the segmented control's segments follow).
        let radios: String = states(&ui, "RadioButton")
            .lines()
            .take(4)
            .map(|l| format!("{l}\n"))
            .collect();
        assert_eq!(
            radios,
            "RadioButton | Small | checked\n\
             RadioButton | Medium | unchecked\n\
             RadioButton | Large | unchecked\n\
             RadioButton | Huge | disabled | unchecked\n"
        );
        // Tab leaves the group for the segmented control's chosen segment,
        // and Shift+Tab comes back to the chosen radio only.
        ui.key("tab");
        assert_eq!(focused_name(&ui).as_deref(), Some("Day"));
        ui.key("shift+tab");
        assert_eq!(focused_name(&ui).as_deref(), Some("Small"));
    }

    #[test]
    fn segmented_arrows_move_the_choice_and_wrap() {
        let mut ui = demo();
        ui.click_node(By::role_name(Role::RadioButton, "Day"));
        ui.key("arrowleft");
        assert_eq!(focused_name(&ui).as_deref(), Some("Month"));
        ui.key("arrowright");
        assert_eq!(focused_name(&ui).as_deref(), Some("Day"));
        assert_eq!(ui.app().view, 0);
    }

    #[test]
    fn slider_keys_step_page_and_jump_to_the_ends() {
        let mut ui = demo();
        ui.click_node(By::role_name(Role::Slider, "Volume"));
        // The click landed mid-track; start from a known value.
        ui.key("home");
        let cases = [
            ("arrowright", "5"),
            ("arrowup", "10"),
            ("pageup", "60"),
            ("arrowleft", "55"),
            ("end", "100"),
            ("arrowright", "100"),
            ("pagedown", "50"),
            ("home", "0"),
            ("arrowdown", "0"),
        ];
        for (key, expected) in cases {
            ui.key(key);
            let value = ui.find(By::role_name(Role::Slider, "Volume")).value;
            assert_eq!(value.as_deref(), Some(expected), "after {key}");
        }
    }

    #[test]
    fn dragging_the_slider_follows_the_pointer_in_steps() {
        let mut ui = demo();
        let track = ui.find(By::role_name(Role::Slider, "Volume")).bounds;
        let y = track.y + track.height / 2.0;
        ui.drag((track.x + 1.0, y), (track.x + track.width * 0.75, y));
        // The value maps over the track less the thumb's width, so 75% of
        // the track is a little over 75; it snaps to the 5 step.
        assert_eq!(ui.app().volume, 75.0);
        ui.drag(
            (track.x + track.width * 0.5, y),
            (track.x + track.width + 50.0, y),
        );
        assert_eq!(ui.app().volume, 100.0);
    }

    #[test]
    fn combobox_filters_by_fuzzy_match_and_commits_with_enter() {
        let mut ui = demo();
        ui.click_node(By::role_name(Role::TextInput, "City"));
        ui.type_text("sf");
        let options: Vec<_> = ui
            .find_all(By::role(Role::ListBoxOption))
            .into_iter()
            .filter_map(|node| node.name)
            .collect();
        assert_eq!(options, ["San Francisco"]);
        ui.key("enter");
        assert!(ui.try_find(By::role(Role::ListBox)).is_none());
        assert_eq!(ui.app().city.query(), "San Francisco");
        assert_eq!(focused_name(&ui).as_deref(), Some("City"));
    }

    #[test]
    fn combobox_arrows_move_the_highlight_and_escape_closes() {
        let mut ui = demo();
        ui.click_node(By::role_name(Role::TextInput, "City"));
        ui.type_text("lo");
        // London and Los Angeles start with "Lo"; Lisbon matches loosely.
        assert_eq!(ui.app().city.highlighted_label(), Some("London"));
        ui.key("arrowdown");
        assert_eq!(ui.app().city.highlighted_label(), Some("Los Angeles"));
        ui.key("escape");
        assert!(ui.try_find(By::role(Role::ListBox)).is_none());
        assert_eq!(ui.app().city.query(), "lo");
    }

    #[test]
    fn controls_publish_roles_states_and_values() {
        let mut ui = demo();
        ui.click_node(By::role_name(Role::ComboBox, "Fruit"));
        let tree = states(&ui, "");
        let expect = [
            "ComboBox | Fruit | expanded | value=\"Apple\"",
            "ListBox | Fruit",
            "Group | Pome",
            "ListBoxOption | Apple | selected",
            "ListBoxOption | Quince | disabled | unselected",
            "ComboBox | City | collapsed",
            "RadioGroup | Size",
            "RadioGroup | -",
            "Switch | Wi-Fi | checked",
            "Slider | Volume | range=40/0..100 | value=\"40\"",
            "ComboBox | Theme | collapsed | value=\"System\"",
        ];
        for line in expect {
            assert!(
                tree.lines().any(|l| l == line),
                "missing {line:?} in:\n{tree}"
            );
        }
    }
}
