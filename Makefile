.PHONY: help build start fmt check

help:
	@echo "Usage: make [target]"
	@echo ""
	@echo "Targets:"
	@echo "  help          Show this help message"
	@echo "  build         Build the project"
	@echo "  start         Run the build"
	@echo "  fmt           Format the code"
	@echo "  check         Format, check and lint the code"

build:
	cargo build --release

start: build
	./target/release/s-dock

fmt:
	cargo fmt

check: fmt
	cargo check
	cargo clippy -- -D warnings
