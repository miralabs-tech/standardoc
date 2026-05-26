// Themed replacement for the native `<select>` element. The native
// dropdown menu is UA-rendered and ignores the `--sd-*` design tokens,
// which leaves the open menu white-on-light-grey inside an otherwise
// dark shell. `<standardoc-select>` is a button + popover combo that
// renders entirely with our own SCSS so the open state is theme-aware.

export interface SelectOption {
  readonly value: string | number;
  readonly label: string;
  readonly disabled?: boolean;
}

/// Where the popover should anchor relative to the trigger button.
/// `auto` flips between top/bottom based on the available viewport
/// space at open time. Defaults to `auto`.
export type SelectPlacement = 'bottom' | 'top' | 'auto';

export interface SelectChangeDetail {
  readonly value: string | number;
  readonly option: SelectOption;
}
