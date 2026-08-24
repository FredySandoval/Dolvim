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

behavioral: debug
	python3 tests/behavioral/run.py --unit
	python3 tests/behavioral/run.py

behavioral-prototype:
	python3 tests/behavioral/prototype/test_analyze.py
	python3 tests/behavioral/prototype/analyze.py tests/behavioral/prototype

# Render the <dataflow> graph from SOURCE-OF-TRUTH.xml into an interactive
# HTML viewer and a Graphviz .dot source. Pure stdlib python3; no installs.
dataflow:
	python3 tools/dataflow.py

# Same, plus a static SVG if graphviz `dot` is installed.
dataflow.svg:
	python3 tools/dataflow.py
	@if command -v dot >/dev/null 2>&1; then \
		dot -Tsvg dataflow.dot -o dataflow.svg && echo "wrote dataflow.svg"; \
	else \
		echo "graphviz 'dot' not installed — skipped dataflow.svg (see dataflow.dot)"; \
	fi

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

.PHONY: all build debug run fmt lint test behavioral behavioral-prototype dataflow dataflow.svg check install uninstall clean
