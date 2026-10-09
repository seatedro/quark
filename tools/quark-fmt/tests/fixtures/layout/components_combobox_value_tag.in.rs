// crates/quark-components/src/combobox.rs, line 449
fn excerpt() {
    view! {
        <div class="relative flex-col" w={width} accessibility_id={&*self.id}
             test_id="combobox" accessibility_role={accesskit::Role::ComboBox}
             aria-label={self.label.clone()} aria-expanded={open}
             on_key={("alt+arrowdown", msg(ComboboxMsg::Open))}
             @when {open} {
                 on_key={("enter", msg(ComboboxMsg::CommitHighlighted))}
                 on_key={("escape", msg(ComboboxMsg::Close))}
             }>
            <{self.input} placeholder={self.placeholder} focused={focused} class="w-full"
                          h={field_h} />
            if open {
                <anchored(
                    view! {
                        <popover_panel(theme) w={width} py={list_padding(theme)}
                            accessibility_id={format!("{}-listbox", self.id)}
                            test_id="combobox-listbox"
                            accessibility_role={accesskit::Role::ListBox}
                            aria-label={self.label.clone()}>
                            for (i, &option) in self.matches.options.iter().enumerate() {
                                {match_row(
                                    &self.options[option],
                                    &self.matches.ranges[self.matches.spans[i].clone()],
                                    self.highlighted == Some(i),
                                    self.selected == Some(option),
                                    msg(ComboboxMsg::Commit(i)),
                                    theme,
                                )}
                            }
                            if self.loading || self.matches.options.is_empty() {
                                {status_row(self.loading, theme)}
                            }
                        </popover_panel>
                    },
                    PopoverSide::Bottom,
                    self.viewport,
                ) />
            }
        </div>
    }
}
