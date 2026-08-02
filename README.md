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

## License

MIT.
