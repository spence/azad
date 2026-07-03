set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
    @just help

help:
    @echo "Workspace commands:"
    @just --list

build:
    cargo build -q -p azad

check:
    cargo check -q --workspace

fmt-check:
    cargo fmt --all --check

test:
    cargo test -q --workspace

test-replay:
    cargo test -q -p azad-asr --test replay -- --ignored --test-threads=1

test-replay-required:
    AZAD_TEST_REQUIRE_MODELS=1 cargo test -q -p azad-asr --test replay -- --ignored --test-threads=1

clippy:
    cargo clippy -q --workspace --all-targets -- -D warnings

swift-build:
    crates/azad-mlx-asr/scripts/swift-build-release.sh crates/azad-ui target/swift/azad-ui
    crates/azad-mlx-asr/scripts/swift-build-release.sh crates/azad-mlx-asr target/swift/azad-mlx-asr

ui-snapshots:
    crates/azad-mlx-asr/scripts/swift-build-release.sh crates/azad-ui target/swift/azad-ui
    @mkdir -p target/ui-snapshots
    @for surface in onboarding-fresh onboarding-ready settings-general settings-models settings-permissions settings-debug settings-connectors menu-collapsed menu-expanded; do \
      target/swift/azad-ui/release/azad-ui-preview \
        --surface "$surface" \
        --screenshot "target/ui-snapshots/$surface.png" \
        --quit-after 0.5; \
    done

verify:
    just doctor
    just fmt-check
    just check
    just test
    just swift-build
    just clippy

dist:
    just --justfile crates/azad/justfile dist

doctor:
    @missing=0; \
    for tool in git cargo rustc just cmake clang swift; do \
      if command -v "$tool" >/dev/null 2>&1; then \
        printf "ok  %s: %s\n" "$tool" "$(command -v "$tool")"; \
      else \
        printf "err %s: missing\n" "$tool"; \
        missing=1; \
      fi; \
    done; \
    if [[ "$(uname -s)" == "Darwin" ]]; then \
      macos_version="$(sw_vers -productVersion)"; \
      printf "ok  macos: %s\n" "$macos_version"; \
      major="${macos_version%%.*}"; \
      if [[ "$major" -lt 14 ]]; then printf "err macos: Azad MLX runtime requires macOS 14+\n"; missing=1; fi; \
      if xcode-select -p >/dev/null 2>&1; then \
        printf "ok  xcode-select: %s\n" "$(xcode-select -p)"; \
      else \
        printf "err xcode-select: command line tools missing\n"; \
        missing=1; \
      fi; \
      if xcrun --find metal >/dev/null 2>&1; then \
        printf "ok  metal: %s\n" "$(xcrun --find metal)"; \
      elif [[ -d /Applications/Xcode.app/Contents/Developer ]] && DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer xcrun --find metal >/dev/null 2>&1; then \
        printf "ok  metal: %s\n" "$(DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer xcrun --find metal)"; \
      else \
        printf "err metal: missing; run DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer xcodebuild -downloadComponent MetalToolchain\n"; \
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
