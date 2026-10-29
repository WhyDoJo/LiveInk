PREFIX ?= $(HOME)/.local

.PHONY: build install uninstall check

build:
	cargo build --release --locked

install: build
	install -Dm755 target/release/livepaint "$(DESTDIR)$(PREFIX)/bin/livepaint"
	install -Dm644 contrib/livepaint.desktop "$(DESTDIR)$(PREFIX)/share/applications/livepaint.desktop"
	install -Dm644 contrib/livepaint.svg "$(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/livepaint.svg"

uninstall:
	rm -f "$(DESTDIR)$(PREFIX)/bin/livepaint"
	rm -f "$(DESTDIR)$(PREFIX)/share/applications/livepaint.desktop"
	rm -f "$(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/livepaint.svg"

check:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings
	cargo test
