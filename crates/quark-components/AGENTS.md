# AGENTS.md

Guidance for agents working in `crates/quark-components`. The root rules
and `../quark/AGENTS.md` still apply.

## External consumers

- Diffy's diff view (extracted from this crate at tag `handoff/diffy-v1`)
  relies on `tree::CollectionEnv` staying public, and on these `quark_ui`
  items: `Action`, `FocusId`, `design::Alpha`, `element::{AnyElement,
  ScrollHandle, ScrollbarVisibility, WHEEL_LINE_PX}`, `icons::lucide`,
  `style::Styled`, `theme::{Color, Theme, ThemeMode}`, and
  `virtual_list::{RowKey, RowTable, VariableList}`. Do not make them
  private or change their meaning without checking that view.
- Diff and syntax code does not belong here. Quark stays an application
  platform; domain libraries live in the apps that use them.
