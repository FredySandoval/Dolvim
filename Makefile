# dolvim - KDE Dolphin, recreated in the terminal
# See LICENSE for copyright and license details.

# config.rs is hand-aligned and rustfmt.toml ignores it, but `ignore` is a
# nightly option. Formatting through stable silently reflows the tables.
FMT_TOOLCHAIN = nightly

PREFIX = /usr/local

BIN = target/release/dolvim

all: build

build:
	cargo build --release

debug:
	cargo build

run:
	cargo run --

fmt:
	RUSTUP_TOOLCHAIN=$(FMT_TOOLCHAIN) cargo fmt

lint:
	cargo clippy --all-targets -- -D warnings

test:
	cargo test

# What must pass before a commit.
check:
	RUSTUP_TOOLCHAIN=$(FMT_TOOLCHAIN) cargo fmt --check
	cargo clippy --all-targets -- -D warnings
	cargo test

# Not dependent on `build`: cargo runs as root under sudo, where rustup has no
# default toolchain. Run `make` first.
install:
	install -Dm755 $(BIN) $(DESTDIR)$(PREFIX)/bin/dolvim

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/dolvim

clean:
	cargo clean

.PHONY: all build debug run fmt lint test check install uninstall clean
