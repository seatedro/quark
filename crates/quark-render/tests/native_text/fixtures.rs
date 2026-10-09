//! Deterministic text fixtures: vendored fonts, fixed locale, explicit
//! scale, black text on white. Each fixture is a stack of lines laid out
//! at one device scale; a control scene holds the same background without
//! text, so frame costs can be attributed to text.

use std::sync::Arc;

use quark::Color;
use quark_render::scene::{Primitive, Rect, RectPrimitive, Scene, ShapedText, TextPrimitive};
use quark_render::{FontKind, FontWeight};
use quark_text::fonts::{CJK_FAMILY, EMOJI_FAMILY, IBM_PLEX_SANS_FAMILY, INTER_FAMILY};
use quark_text::{FontSettings, TextParams, TextStyle, TextSystem};

/// Device scales every fixture renders at.
pub const SCALES: [f32; 3] = [1.0, 1.5, 2.0];

/// Logical canvas width; lines wrap inside its padding.
const WIDTH: f32 = 560.0;
const PAD: f32 = 8.0;
const GAP: f32 = 4.0;

pub const BACKGROUND: Color = Color::rgba(255, 255, 255, 255);
const INK: Color = Color::rgba(0, 0, 0, 255);

/// Which atlas a fixture's glyphs land in. Fixtures keep to one so atlas
/// growth can be attributed to a texture kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content {
    Mask,
    Color,
}

pub struct Line {
    pub text: &'static str,
    pub style: TextStyle,
}

pub struct Fixture {
    pub name: &'static str,
    pub content: Content,
    pub lines: Vec<Line>,
}

/// One fixture laid out at one scale.
pub struct Prepared {
    pub scene: Scene,
    /// The background alone.
    pub control: Scene,
    pub width: u32,
    pub height: u32,
    pub glyphs: usize,
}

/// The vendored fonts plus the bundled CJK face, which this crate's
/// dev-dependencies leave out of the default font list.
pub fn text_system() -> TextSystem {
    let mut text = TextSystem::vendored_only(&FontSettings::default());
    let has_cjk = text
        .font_system()
        .db()
        .faces()
        .any(|face| face.families.iter().any(|(name, _)| name == CJK_FAMILY));
    if !has_cjk {
        static CJK: &[u8] =
            include_bytes!("../../../quark-text/assets/fonts/QuarkCJKFallback-Regular.otf");
        text.load_font_data(Arc::new(CJK));
    }
    text
}

fn ui(size: f32) -> TextStyle {
    TextStyle::new(size)
}

fn mono(size: f32) -> TextStyle {
    TextStyle::new(size).kind(FontKind::Mono)
}

fn family(name: &'static str, size: f32, weight: FontWeight) -> TextStyle {
    TextStyle::new(size).family(Some(name)).weight(weight)
}

fn lines(style: impl Fn(usize) -> TextStyle, texts: &[&'static str]) -> Vec<Line> {
    texts
        .iter()
        .enumerate()
        .map(|(i, &text)| Line {
            text,
            style: style(i),
        })
        .collect()
}

pub fn fixtures() -> Vec<Fixture> {
    let ui_strings = [
        "Settings",
        "Open Recent…",
        "Save changes to “Untitled 3” before closing?",
        "1,234 files · 56.7 MB · modified 3 minutes ago",
        "The quick brown fox jumps over the lazy dog, then naps for a while \
         under the old oak tree while the wind rustles its leaves.",
        "fi fl ffi 0O1lI| ([{ }]) @#%&*",
    ];
    let code = [
        "fn main() -> Result<(), Box<dyn Error>> {",
        "    let total: u64 = items.iter().map(|x| x.len() as u64).sum();",
        "    if total != 0 && total >= LIMIT { return Err(\"over\".into()); }",
        "    // ==> <= >= != === !== :: ... |> <|",
        "    println!(\"{total:>8} bytes in {} files\", items.len());",
        "}",
    ];
    let cjk = [
        "文字渲染测试：简体中文的常用汉字。",
        "日本語のテキスト、ひらがなとカタカナ。",
        "한국어 텍스트 렌더링 시험입니다.",
        "混排 Mixed 中英文 text 与标点，。！？",
    ];
    let emoji = ["😀😃😄😁😆😅🤣😂🙂🙃", "👍🏽👋🏿🚀🎉❤️✨🔥🌈", "👨‍👩‍👧‍👦🏳️‍🌈🧑🏻‍💻"];
    let weights: [u16; 9] = [100, 200, 300, 400, 500, 600, 700, 800, 900];
    let mut variable = Vec::new();
    for name in [INTER_FAMILY, IBM_PLEX_SANS_FAMILY] {
        for weight in weights {
            variable.push(Line {
                text: "Hamburgefonstiv 0123 Quark",
                style: family(name, 15.0, FontWeight::Numeric(weight)),
            });
        }
    }
    for weight in [
        FontWeight::Light,
        FontWeight::Normal,
        FontWeight::Medium,
        FontWeight::Semibold,
        FontWeight::Bold,
    ] {
        variable.push(Line {
            text: "Hamburgefonstiv 0123 Quark",
            style: ui(15.0).weight(weight),
        });
    }
    vec![
        Fixture {
            name: "ui",
            content: Content::Mask,
            lines: lines(|i| ui([11.0, 12.0, 13.0, 14.0, 13.0, 17.0][i]), &ui_strings),
        },
        Fixture {
            name: "code",
            content: Content::Mask,
            lines: lines(|i| mono(if i % 2 == 0 { 12.0 } else { 13.0 }), &code),
        },
        Fixture {
            name: "cjk",
            content: Content::Mask,
            lines: lines(
                |i| family(CJK_FAMILY, [14.0, 14.0, 16.0, 13.0][i], FontWeight::Normal),
                &cjk,
            ),
        },
        Fixture {
            name: "emoji",
            content: Content::Color,
            lines: lines(
                |i| family(EMOJI_FAMILY, [16.0, 20.0, 28.0][i], FontWeight::Normal),
                &emoji,
            ),
        },
        Fixture {
            name: "weights",
            content: Content::Mask,
            lines: variable,
        },
    ]
}

impl Fixture {
    pub fn prepare(&self, text: &mut TextSystem, scale: f32) -> Prepared {
        let wrap = WIDTH - 2.0 * PAD;
        let mut placed = Vec::with_capacity(self.lines.len());
        let mut y = PAD;
        let mut glyphs = 0;
        for line in &self.lines {
            let params = TextParams::new(line.text, line.style)
                .wrap_width(Some(wrap))
                .scale_factor(scale);
            let layout = text.layout(&params).expect("fixture layout");
            glyphs += layout
                .buffer()
                .layout_runs()
                .map(|run| run.glyphs.len())
                .sum::<usize>();
            let height = layout.size().1;
            placed.push((y, height, layout));
            y += height.ceil() + GAP;
        }
        let width = (WIDTH * scale).ceil() as u32;
        let height = ((y + PAD) * scale).ceil() as u32;
        let background = RectPrimitive {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: width as f32,
                height: height as f32,
            },
            color: BACKGROUND,
        };
        let mut scene = Scene::default();
        let mut control = Scene::default();
        scene.rect(background);
        control.rect(background);
        for (y, height, layout) in placed {
            // Whole device pixels, as a laid out element's origin is.
            scene.push(Primitive::TextRun(TextPrimitive {
                rect: Rect {
                    x: (PAD * scale).round(),
                    y: (y * scale).round(),
                    width: wrap * scale,
                    height: height * scale,
                },
                layout: ShapedText::new(Arc::new(layout)),
                color: INK,
            }));
        }
        Prepared {
            scene,
            control,
            width,
            height,
            glyphs,
        }
    }
}
