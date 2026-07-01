set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
    @just help

help:
    @echo "Workspace commands:"
    @just --list

build:
    cargo build -p azad

check:
    cargo check --workspace

test:
    cargo test --workspace

dist:
    just --justfile crates/azad/justfile dist

doctor:
    @missing=0; \
    for tool in git cargo rustc just cmake clang; do \
      if command -v "$tool" >/dev/null 2>&1; then \
        printf "ok  %s: %s\n" "$tool" "$(command -v "$tool")"; \
      else \
        printf "err %s: missing\n" "$tool"; \
        missing=1; \
      fi; \
    done; \
    if [[ "$(uname -s)" == "Darwin" ]]; then \
      printf "ok  macos: %s\n" "$(sw_vers -productVersion)"; \
      if xcode-select -p >/dev/null 2>&1; then \
        printf "ok  xcode-select: %s\n" "$(xcode-select -p)"; \
      else \
        printf "err xcode-select: command line tools missing\n"; \
        missing=1; \
      fi; \
    else \
      printf "err macos: Azad app install/start requires macOS\n"; \
      missing=1; \
    fi; \
    if [[ -f Cargo.lock ]]; then printf "ok  Cargo.lock\n"; else printf "err Cargo.lock missing\n"; missing=1; fi; \
    exit "$missing"

install:
    just --justfile crates/azad/justfile install

start:
    just --justfile crates/azad/justfile start

stop:
    just --justfile crates/azad/justfile stop

restart:
    just --justfile crates/azad/justfile restart

status:
    just --justfile crates/azad/justfile status

logs:
    just --justfile crates/azad/justfile logs

uninstall:
    just --justfile crates/azad/justfile uninstall

reset-permissions:
    just --justfile crates/azad/justfile reset-permissions
