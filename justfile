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
