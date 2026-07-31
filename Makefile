# dolvin - KDE Dolphin, recreated in the terminal
# See LICENSE for copyright and license details.

# config.rs is hand-aligned and rustfmt.toml ignores it, but `ignore` is a
# nightly option. Formatting through stable silently reflows the tables.
FMT_TOOLCHAIN = nightly

BIN = target/release/dolvin

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

install:
	cargo install --path .

uninstall:
	cargo uninstall dolvin

clean:
	cargo clean

.PHONY: all build debug run fmt lint test check install uninstall clean
