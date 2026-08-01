# dolvim

KDE Dolphin, recreated in the terminal.

![dolvim](assets/Dolvim-2026-08-01_00-43.png)

A file manager that looks and behaves like Dolphin — Places panel, breadcrumb,
tabs, Details view, thumbnails, drag and drop, trash — driven by vim keys or the
mouse. Breeze colors, measured off a real Dolphin screenshot.

## Build

	make            # release build into target/release/dolvim
	make install    # cargo install --path .

Needs a truecolor terminal and a Nerd Font patched font for the icon glyphs.
Without one they render as tofu; swap them in `src/config.rs` and recompile.

## Use

	dolvim [DIR]

With no argument it opens the current directory. Press `F1` inside for the key
list.

## Configure

Configuration is source code. Colors, glyphs, and keys are `const` tables in
`src/config.rs` — edit, recompile, done. There is no dotfile and no config
parser.

That file is hand-aligned and `rustfmt.toml` skips it, but `ignore` is a nightly
option, so format with `make fmt` (which sets the nightly toolchain). Plain
stable `cargo fmt` will silently reflow the tables.

## Develop

	make check      # fmt --check, clippy -D warnings, test — must pass before a commit

`docs/UI_SPEC.md` is the visual contract. `docs/DECISIONS.md` records every place
the plan, the manifesto, or reality had to be reconciled. `CLAUDE.md` is
normative on style.

## License

MIT.
