/// Typed interaction state flags (hover, focus, disabled, ...) for a node.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct StyleState(u16);

impl StyleState {
    pub const HOVER: Self = Self(1 << 0);
    pub const ACTIVE: Self = Self(1 << 1);
    pub const FOCUS_VISIBLE: Self = Self(1 << 2);
    pub const DISABLED: Self = Self(1 << 3);
    pub const SELECTED: Self = Self(1 << 4);
    pub const CHECKED: Self = Self(1 << 5);
    pub const EXPANDED: Self = Self(1 << 6);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub fn contains(self, state: Self) -> bool {
        self.0 & state.0 == state.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn insert(&mut self, state: Self) {
        self.0 |= state.0;
    }

    pub fn with(mut self, state: Self) -> Self {
        self.insert(state);
        self
    }

    pub fn bits(self) -> u16 {
        self.0
    }
}

impl std::ops::BitOr for StyleState {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for StyleState {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}
