# Dolphin + vim = Dolvim

KDE Dolphin, recreated in the terminal, with vim bindings, written in rust btw.

![dolvim](assets/Dolvim-2026-08-01_00-43.png)

A file manager that looks and behaves like Dolphin — Places panel, breadcrumb,
tabs, Details view, thumbnails, drag and drop, trash — driven by vim keys or the
mouse. Breeze colors, measured off a real Dolphin screenshot.

## Requirements

A Rust toolchain (2021 edition), a truecolor terminal, and a Nerd Font patched
terminal font — the icons are glyphs from it. Without one they render as tofu;
swap them for plain Unicode in `src/config.rs` and recompile. A glyph must
exist in the font you use: one that does not falls back to another face, and a
proportional fallback shifts every column after it.

Six crates, no framework, no `build.rs`. `cargo build` on a fresh system is the
whole story.

## Build

	make            # release build into target/release/dolvim
	make debug      # unoptimised, for a backtrace worth reading

## Install

	make && sudo make install

Installs to `/usr/local/bin/dolvim`. Build unprivileged and install privileged —
`make install` deliberately does not build, because under `sudo` cargo runs as
root, where rustup has no default toolchain.

	sudo make install PREFIX=/usr    # somewhere else
	make install DESTDIR=/tmp/stage  # stage it, no root needed
	sudo make uninstall

## Use

	dolvim [DIR]

With no argument it opens the current directory. Press `:help` inside for the key
list.

## Configure

Configuration is source code. Colors, glyphs, and keys are `const` tables in
`src/config.rs` — edit, recompile, done. There is no dotfile and no config
parser.

## Develop

	make run        # cargo run --
	make test       # unit tests, inline #[cfg(test)]
	make lint       # clippy, warnings are errors
	make fmt        # rustfmt, via nightly
	make check      # fmt --check + lint + test; what must pass before a commit

`make fmt` needs a nightly toolchain (`rustup toolchain install nightly`).
`rustfmt.toml` ignores `src/config.rs` to keep its hand-aligned tables, and
`ignore` is nightly-only — stable `cargo fmt` warns and reflows them anyway.

`docs/DECISIONS.md` is the log: every non-obvious choice, why it was made, and
what it cost. Read it before changing behaviour, and add to it when you do.
`docs/UI_SPEC.md` holds the Breeze measurements the colors came from.

`CLAUDE.md` is the manifesto the code is written to. It is not decoration —
features are refused on purpose, and the absence of a test suite is an argument
made there, not an oversight.

## License

MIT.
