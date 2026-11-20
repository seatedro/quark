use quark::scene::Primitive;
use quark_render::Scene;

use super::*;
use crate::animation::{AnimationTable, Curve};
use crate::theme::Theme;

const CARD: Color = Color::rgba(200, 40, 40, 255);
const IDLE: Color = Color::rgba(20, 40, 200, 255);
const HOVER: Color = Color::rgba(240, 180, 20, 255);
const OVER_CARD: Option<(f32, f32)> = Some((10.0, 10.0));
/// How far opacity may move in one 16 ms frame of [`Motion::smooth`]; a
/// pop moves it much further.
const MAX_STEP: f32 = 0.15;

fn card_key() -> AnimKey {
    AnimKey::from(&UiKey::from("card"))
}

/// A window's persistent state between test frames.
struct World {
    theme: Theme,
    text: TextSystem,
    layouts: LayoutCache,
    store: SignalStore,
    animations: AnimationTable,
    presence: PresenceTable,
    cache: ElementCache,
}

/// One painted frame.
struct Frame {
    scene: Scene,
    next_frame_ms: Option<u64>,
}

impl World {
    fn new() -> Self {
        Self::with_theme(Theme::default_dark())
    }

    fn with_theme(theme: Theme) -> Self {
        Self {
            theme,
            text: TextSystem::vendored_only(&Default::default()),
            layouts: LayoutCache::default(),
            store: SignalStore::new(),
            animations: AnimationTable::new(),
            presence: PresenceTable::new(),
            cache: ElementCache::new(),
        }
    }

    /// Paints `root` at 200x100 at `now`, as a window's frame does.
    fn frame(&mut self, root: impl IntoAnyElement, pointer: Option<(f32, f32)>, now: u64) -> Frame {
        self.animations.tick(now);
        let mut cx = ElementContext::new(
            &self.theme,
            1.0,
            &mut self.text,
            &mut self.layouts,
            pointer,
            &self.store,
        )
        .with_clock(now)
        .with_animations(&mut self.animations)
        .with_presence(&mut self.presence)
        .with_element_cache(&mut self.cache);
        let mut scene = Scene::default();
        render_element(&mut root.into_any(), &mut scene, &mut cx, 200.0, 100.0);
        cx.finish_frame();
        Frame {
            next_frame_ms: cx.next_frame_ms(),
            scene,
        }
    }
}

fn card(transition: Transition) -> Div {
    div()
        .key("card")
        .w(50.0)
        .h(40.0)
        .bg(CARD)
        .presence(transition)
}

fn view(card: Option<Div>) -> Div {
    div()
        .w(200.0)
        .h(100.0)
        .children(card.map(IntoAnyElement::into_any))
}

/// The layers the card's chunks are drawn through, in paint order, at any
/// depth of chunks.
fn card_layers(scene: &Scene) -> Vec<LayerTransform> {
    fn walk(primitives: &[Primitive], out: &mut Vec<LayerTransform>) {
        for primitive in primitives {
            let Primitive::Chunk(placed) = primitive else {
                continue;
            };
            let inner = placed.chunk.primitives();
            if inner
                .iter()
                .any(|p| matches!(p, Primitive::RoundedRect(r) if r.color == CARD))
            {
                out.push(placed.layer);
            }
            walk(inner, out);
        }
    }
    let mut out = Vec::new();
    walk(&scene.primitives, &mut out);
    out
}

fn card_opacity(scene: &Scene) -> Option<f32> {
    match card_layers(scene)[..] {
        [] => None,
        [layer] => Some(layer.opacity),
        ref many => panic!("card painted {} times", many.len()),
    }
}

/// Frames every 16 ms from `now` until nothing is pending, with the card's
/// opacity in each. Returns the opacities and the time after the last.
fn run(world: &mut World, card: impl Fn() -> Option<Div>, mut now: u64) -> (Vec<Option<f32>>, u64) {
    let mut opacities = Vec::new();
    loop {
        let frame = world.frame(view(card()), None, now);
        opacities.push(card_opacity(&frame.scene));
        now += 16;
        if frame.next_frame_ms.is_none() {
            return (opacities, now);
        }
        assert!(now < 5_000, "never settled: {opacities:?}");
    }
}

fn assert_smooth(opacities: &[Option<f32>]) {
    let painted: Vec<f32> = opacities.iter().flatten().copied().collect();
    for pair in painted.windows(2) {
        assert!(
            (pair[1] - pair[0]).abs() <= MAX_STEP,
            "jumped {pair:?} in {opacities:?}"
        );
    }
}

#[test]
fn divs_in_the_first_frame_show_without_entering() {
    let mut world = World::new();
    let frame = world.frame(view(Some(card(Transition::fade()))), None, 0);
    assert_eq!(
        (card_opacity(&frame.scene), frame.next_frame_ms),
        (Some(1.0), None)
    );
    assert_eq!(
        world.presence.phase(card_key()),
        Some(PresencePhase::Present)
    );
}

#[test]
fn div_mounted_later_fades_in_from_its_enter_layer_until_present() {
    let mut world = World::new();
    world.frame(view(None), None, 0);
    let (opacities, _) = run(&mut world, || Some(card(Transition::fade())), 10);
    assert_eq!(opacities[0], Some(0.0));
    assert_eq!(opacities.last(), Some(&Some(1.0)));
    assert!(opacities.windows(2).all(|w| w[0] < w[1]), "{opacities:?}");
    assert_smooth(&opacities);
    assert_eq!(
        world.presence.phase(card_key()),
        Some(PresencePhase::Present)
    );
}

#[test]
fn removed_div_fades_out_from_its_last_frame_then_is_gone() {
    let mut world = World::new();
    world.frame(view(Some(card(Transition::fade()))), None, 0);
    let (opacities, now) = run(&mut world, || None, 16);
    assert_eq!(
        opacities[0],
        Some(1.0),
        "the exit starts from the frame on screen"
    );
    assert!(
        opacities.iter().all(Option::is_some),
        "vanished before its exit settled: {opacities:?}"
    );
    assert!(opacities.windows(2).all(|w| w[0] > w[1]), "{opacities:?}");
    assert_smooth(&opacities);
    let after = world.frame(view(None), None, now);
    assert_eq!(card_opacity(&after.scene), None);
    assert!(world.presence.is_empty() && world.animations.is_empty());
}

// The root shows its hover color only when no blocking card is hit above
// it, so an exiting card must be invisible to the pointer.
#[test]
fn exiting_div_takes_no_pointer_input() {
    let root = |with_card: bool| {
        let card = with_card.then(|| card(Transition::fade()).block_mouse());
        view(card).bg(IDLE).hover_bg(HOVER)
    };
    let backgrounds = |scene: &Scene| -> Vec<Color> {
        scene
            .primitives
            .iter()
            .filter_map(|p| match p {
                Primitive::RoundedRect(r) if r.color != CARD => Some(r.color),
                _ => None,
            })
            .collect()
    };
    let mut world = World::new();
    let present = world.frame(root(true), OVER_CARD, 0);
    assert_eq!(backgrounds(&present.scene), vec![IDLE]);
    let exiting = world.frame(root(false), OVER_CARD, 16);
    assert_eq!(card_opacity(&exiting.scene), Some(1.0));
    assert_eq!(backgrounds(&exiting.scene), vec![HOVER]);
}

// Coming back mid-exit retargets from the opacity and velocity on screen:
// the first frame back shows exactly what the exit would have, and no
// frame after jumps.
#[test]
fn div_back_mid_exit_returns_from_where_its_exit_got_to() {
    let exit_until = |until: u64| {
        let mut world = World::new();
        world.frame(view(Some(card(Transition::fade()))), None, 0);
        let mut opacities = Vec::new();
        let mut now = 16;
        while now <= until {
            let frame = world.frame(view(None), None, now);
            opacities.push(card_opacity(&frame.scene));
            now += 16;
        }
        (world, opacities)
    };
    let (_, exit) = exit_until(96);
    let (mut world, mut opacities) = exit_until(80);
    let (back, _) = run(&mut world, || Some(card(Transition::fade())), 96);
    assert_eq!(back[0], exit[5], "first frame back differs from the exit's");
    opacities.extend(back);
    assert_smooth(&opacities);
    assert_eq!(opacities.last(), Some(&Some(1.0)));
    assert_eq!(
        world.presence.phase(card_key()),
        Some(PresencePhase::Present)
    );
}

#[test]
fn reduced_motion_exit_fades_in_place_instead_of_moving() {
    let theme = Theme {
        reduced_motion: true,
        ..Theme::default_dark()
    };
    let moving = Transition::slide(0.0, 30.0).and(Transition::scale(0.5));
    let mut world = World::with_theme(theme);
    world.frame(view(Some(card(moving))), None, 0);
    let mut now = 16;
    let mut layers = Vec::new();
    loop {
        let frame = world.frame(view(None), None, now);
        layers.extend(card_layers(&frame.scene));
        if frame.next_frame_ms.is_none() {
            break;
        }
        now += 16;
    }
    assert!(now <= 16 + 128, "a reduced fade is short; took until {now}");
    assert!(
        layers.iter().all(|l| l.moves_only() || l.opacity < 1.0),
        "{layers:?}"
    );
    assert!(
        layers
            .iter()
            .all(|l| (l.offset_x, l.offset_y, l.scale_x, l.scale_y) == (0.0, 0.0, 1.0, 1.0)),
        "{layers:?}"
    );
    assert!(layers.windows(2).all(|w| w[0].opacity > w[1].opacity));
}

// A presence div inside a cache boundary that replays paints nothing
// itself; the replay must keep its row from being taken for gone, which
// would paint it a second time as an exit.
#[test]
fn presence_div_inside_a_replayed_boundary_is_not_taken_for_gone() {
    let boundary = || cached("list", 0, || view(Some(card(Transition::fade()))));
    let mut world = World::new();
    for now in [0, 16, 32, 48] {
        let frame = world.frame(boundary(), None, now);
        assert_eq!(card_layers(&frame.scene).len(), 1, "at {now} ms");
        assert_eq!(
            world.presence.phase(card_key()),
            Some(PresencePhase::Present),
            "at {now} ms"
        );
    }
}

// The boundary's recording replays unchanged (same chunk, same
// generation) while its layer fades it in frame by frame.
#[test]
fn cached_subtree_replays_its_recording_through_an_animated_layer() {
    let panel = AnimKey::from_str_key("panel");
    let mut world = World::new();
    let hidden = LayerTransform {
        opacity: 0.0,
        ..LayerTransform::IDENTITY
    };
    world.animations.set_layer(panel, hidden, 0);
    let fade_in = Motion::tween(100, Curve::Linear);
    world
        .animations
        .animate_layer(panel, LayerTransform::IDENTITY, fade_in, 0);
    let boundary = || {
        let content = || div().w(50.0).h(40.0).bg(CARD);
        cached("panel", 0, content).animated_layer(panel)
    };
    let mut replays = Vec::new();
    for now in [0, 16, 32, 48, 64] {
        let frame = world.frame(view(None).child(boundary()), None, now);
        replays.extend(frame.scene.primitives.iter().filter_map(|p| match p {
            Primitive::Chunk(placed) => Some((
                placed.chunk.id(),
                placed.chunk.generation(),
                placed.layer.opacity,
            )),
            _ => None,
        }));
    }
    assert_eq!(replays.len(), 4, "every frame after the first replays");
    assert!(
        replays
            .windows(2)
            .all(|w| w[0].0 == w[1].0 && w[0].1 == w[1].1),
        "recorded again: {replays:?}"
    );
    assert!(replays.windows(2).all(|w| w[0].2 < w[1].2), "{replays:?}");
}
