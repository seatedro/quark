//! Which Wayland seat a drag starts from, apart from the protocol objects.
//!
//! `wl_data_device.start_drag` must name the seat whose pointer holds the
//! implicit grab, the surface the grab started on, and that press's serial.
//! A compositor can have several seats, each with its own pointer and
//! presses, so every seat (keyed by its registry name) keeps its focus and
//! its held primary press. A drag picks the one press held on the
//! requesting window, and refuses when there are none or several.
//!
//! Serials are opaque: they wrap and need not grow, so presses are never
//! ordered by serial. `sequence` orders them instead, and `generation`
//! tells a window apart from an earlier one whose `wl_surface` had the same
//! address.

use std::collections::{BTreeMap, HashMap};

/// `BTN_LEFT` from `linux/input-event-codes.h`, the primary button.
pub(super) const PRIMARY: u32 = 0x110;

/// A window's `wl_surface`, by proxy address.
pub(super) type Surface = usize;

/// The press holding a seat's implicit grab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Press {
    pub serial: u32,
    pub button: u32,
    pub origin_surface: Surface,
    /// The origin window's generation when pressed.
    pub generation: u64,
    /// Order among all presses on all seats.
    pub sequence: u64,
}

/// One seat, with the protocol objects `T` that go with it.
pub(super) struct Seat<T> {
    pub objects: T,
    /// `wl_seat.name`, which an explicit seat request matches.
    pub name: Option<String>,
    pointer: bool,
    focus: Option<Surface>,
    press: Option<Press>,
}

/// Why no press was selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Refusal {
    /// No seat holds the primary button on the window.
    NoPress,
    /// Several seats do; the caller must name one.
    Ambiguous,
    /// No seat has the requested name.
    UnknownSeat(String),
}

/// The seat and press a drag starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Selected {
    pub seat: u32,
    pub press: Press,
}

pub(super) struct Seats<T> {
    seats: BTreeMap<u32, Seat<T>>,
    /// Each open window's generation.
    windows: HashMap<Surface, u64>,
    next_generation: u64,
    next_sequence: u64,
}

impl<T> Default for Seats<T> {
    fn default() -> Self {
        Self {
            seats: BTreeMap::new(),
            windows: HashMap::new(),
            next_generation: 1,
            next_sequence: 0,
        }
    }
}

impl<T> Seats<T> {
    /// A seat global appeared.
    pub fn add(&mut self, seat: u32, objects: T) {
        self.seats.insert(
            seat,
            Seat {
                objects,
                name: None,
                pointer: false,
                focus: None,
                press: None,
            },
        );
    }

    /// A seat global went away, with its press.
    pub fn remove(&mut self, seat: u32) -> Option<T> {
        self.seats.remove(&seat).map(|seat| seat.objects)
    }

    pub fn get_mut(&mut self, seat: u32) -> Option<&mut Seat<T>> {
        self.seats.get_mut(&seat)
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (u32, &mut Seat<T>)> {
        self.seats.iter_mut().map(|(&key, seat)| (key, seat))
    }

    pub fn into_objects(self) -> impl Iterator<Item = T> {
        self.seats.into_values().map(|seat| seat.objects)
    }

    pub fn named(&mut self, seat: u32, name: String) {
        if let Some(seat) = self.seats.get_mut(&seat) {
            seat.name = Some(name);
        }
    }

    /// The seat gained or lost its pointer. A lost pointer ends its press.
    pub fn pointer(&mut self, seat: u32, present: bool) {
        if let Some(seat) = self.seats.get_mut(&seat) {
            seat.pointer = present;
            if !present {
                seat.focus = None;
                seat.press = None;
            }
        }
    }

    /// The seat's pointer entered `surface` (`None` if it was already gone).
    pub fn enter(&mut self, seat: u32, surface: Option<Surface>) {
        if let Some(seat) = self.seats.get_mut(&seat) {
            seat.focus = surface;
            seat.press = None;
        }
    }

    /// The seat's pointer left its surface, which ends any grab: a drag
    /// that started takes the pointer away like this.
    pub fn leave(&mut self, seat: u32) {
        if let Some(seat) = self.seats.get_mut(&seat) {
            seat.focus = None;
            seat.press = None;
        }
    }

    pub fn button(&mut self, seat: u32, serial: u32, button: u32, pressed: bool) {
        let Some(state) = self.seats.get_mut(&seat) else {
            return;
        };
        if button != PRIMARY {
            return;
        }
        if !pressed {
            state.press = None;
            return;
        }
        state.press = state.focus.map(|origin_surface| {
            self.next_sequence += 1;
            Press {
                serial,
                button,
                origin_surface,
                generation: self.windows.get(&origin_surface).copied().unwrap_or(0),
                sequence: self.next_sequence,
            }
        });
    }

    /// A window opened on `surface`. Presses from a closed window whose
    /// surface had the same address no longer count.
    pub fn window_created(&mut self, surface: Surface) {
        self.windows.insert(surface, self.next_generation);
        self.next_generation += 1;
    }

    pub fn window_destroyed(&mut self, surface: Surface) {
        self.windows.remove(&surface);
        for seat in self.seats.values_mut() {
            if seat.focus == Some(surface) {
                seat.focus = None;
            }
            if seat
                .press
                .is_some_and(|press| press.origin_surface == surface)
            {
                seat.press = None;
            }
        }
    }

    /// The press a drag from `surface` starts from: the named seat's, or
    /// else the only one held on that window.
    pub fn select(&self, surface: Surface, seat: Option<&str>) -> Result<Selected, Refusal> {
        let generation = self.windows.get(&surface).copied();
        let held_here =
            |press: &Press| press.origin_surface == surface && Some(press.generation) == generation;
        let mut held = self.seats.iter().filter_map(|(&key, state)| {
            let press = state.press.filter(held_here)?;
            Some((key, state, press))
        });
        if let Some(name) = seat {
            if !self
                .seats
                .values()
                .any(|state| state.name.as_deref() == Some(name))
            {
                return Err(Refusal::UnknownSeat(name.to_owned()));
            }
            return held
                .find(|(_, state, _)| state.name.as_deref() == Some(name))
                .map(|(seat, _, press)| Selected { seat, press })
                .ok_or(Refusal::NoPress);
        }
        match (held.next(), held.next()) {
            (Some((seat, _, press)), None) => Ok(Selected { seat, press }),
            (None, _) => Err(Refusal::NoPress),
            (Some(_), Some(_)) => Err(Refusal::Ambiguous),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Surface = 0xa000;
    const B: Surface = 0xb000;

    #[derive(Clone, Copy)]
    enum Step {
        /// A seat with a pointer, named `seat{n}`.
        Seat(u32),
        RemoveSeat(u32),
        Pointer(u32, bool),
        Enter(u32, Surface),
        Leave(u32),
        Press(u32, u32, u32),
        Release(u32, u32),
        Open(Surface),
        Close(Surface),
    }

    /// Open windows A and B, run `script`, then select for a drag from A.
    fn select(script: &[Step], seat: Option<&str>) -> String {
        let mut seats = Seats::<()>::default();
        seats.window_created(A);
        seats.window_created(B);
        for step in script {
            match *step {
                Step::Seat(n) => {
                    seats.add(n, ());
                    seats.named(n, format!("seat{n}"));
                    seats.pointer(n, true);
                }
                Step::RemoveSeat(n) => {
                    seats.remove(n);
                }
                Step::Pointer(n, present) => seats.pointer(n, present),
                Step::Enter(n, surface) => seats.enter(n, Some(surface)),
                Step::Leave(n) => seats.leave(n),
                Step::Press(n, serial, button) => seats.button(n, serial, button, true),
                Step::Release(n, button) => seats.button(n, 0, button, false),
                Step::Open(surface) => seats.window_created(surface),
                Step::Close(surface) => seats.window_destroyed(surface),
            }
        }
        match seats.select(A, seat) {
            Ok(Selected { seat, press }) => format!("seat{seat} serial {}", press.serial),
            Err(refusal) => format!("{refusal:?}"),
        }
    }

    #[test]
    fn a_drag_starts_from_the_one_primary_press_held_on_its_window() {
        use Step::*;
        const RIGHT: u32 = 0x111;
        let cases: [(&str, &[Step], Option<&str>, &str); 14] = [
            (
                "one seat holding the button",
                &[Seat(1), Enter(1, A), Press(1, 40, PRIMARY)],
                None,
                "seat1 serial 40",
            ),
            (
                "released",
                &[
                    Seat(1),
                    Enter(1, A),
                    Press(1, 40, PRIMARY),
                    Release(1, PRIMARY),
                ],
                None,
                "NoPress",
            ),
            (
                "only the secondary button",
                &[Seat(1), Enter(1, A), Press(1, 40, RIGHT)],
                None,
                "NoPress",
            ),
            (
                "pressed on another window",
                &[Seat(1), Enter(1, B), Press(1, 40, PRIMARY)],
                None,
                "NoPress",
            ),
            (
                "a second seat pressing elsewhere does not interfere",
                &[
                    Seat(1),
                    Seat(2),
                    Enter(1, A),
                    Press(1, 40, PRIMARY),
                    Enter(2, B),
                    Press(2, 41, PRIMARY),
                ],
                None,
                "seat1 serial 40",
            ),
            (
                "two seats holding on the window are ambiguous",
                &[
                    Seat(1),
                    Seat(2),
                    Enter(1, A),
                    Press(1, 40, PRIMARY),
                    Enter(2, A),
                    Press(2, 41, PRIMARY),
                ],
                None,
                "Ambiguous",
            ),
            (
                "naming a seat settles it",
                &[
                    Seat(1),
                    Seat(2),
                    Enter(1, A),
                    Press(1, 40, PRIMARY),
                    Enter(2, A),
                    Press(2, 41, PRIMARY),
                ],
                Some("seat2"),
                "seat2 serial 41",
            ),
            (
                "a named seat that holds nothing",
                &[Seat(1), Seat(2), Enter(1, A), Press(1, 40, PRIMARY)],
                Some("seat2"),
                "NoPress",
            ),
            (
                "a seat unplugged mid press",
                &[Seat(1), Enter(1, A), Press(1, 40, PRIMARY), RemoveSeat(1)],
                Some("seat1"),
                "UnknownSeat(\"seat1\")",
            ),
            (
                "the pointer capability went away",
                &[
                    Seat(1),
                    Enter(1, A),
                    Press(1, 40, PRIMARY),
                    Pointer(1, false),
                ],
                None,
                "NoPress",
            ),
            (
                "the pointer left, as when a drag already took it",
                &[Seat(1), Enter(1, A), Press(1, 40, PRIMARY), Leave(1)],
                None,
                "NoPress",
            ),
            (
                "a press on a closed window whose surface address came back",
                &[Seat(1), Enter(1, A), Press(1, 40, PRIMARY), Open(A)],
                None,
                "NoPress",
            ),
            (
                "a press on a window since closed",
                &[Seat(1), Enter(1, A), Press(1, 40, PRIMARY), Close(A)],
                None,
                "NoPress",
            ),
            (
                "a newer press with a wrapped, smaller serial",
                &[
                    Seat(1),
                    Enter(1, A),
                    Press(1, u32::MAX, PRIMARY),
                    Release(1, PRIMARY),
                    Press(1, 2, PRIMARY),
                ],
                None,
                "seat1 serial 2",
            ),
        ];
        for (name, script, seat, expected) in cases {
            assert_eq!(select(script, seat), expected, "{name}");
        }
    }
}
