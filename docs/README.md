# Documentation

Project documentation is organized by ownership boundary.

## Root

- [README](../README.md): install, source development, requirements, and release notes.
- [PROJECT](../PROJECT.md): active cleanup and product-quality plan.
- [Quality plan](quality-plan.md): cleanup phases and refactor order.
- [Release process](release-process.md): signed/notarized release build workflow.
- [Surface design specification](design-agent-surface-spec.md): design-agent brief
  for onboarding, settings, menu, overlays, history, and connectors.

## App

- [Azad README](../crates/azad/README.md): launchd app workflow and app-specific commands.
- [Azad specification](../crates/azad/SPECIFICATION.md): app architecture, interaction state, overlay, paste, settings, and lifecycle contracts.
- [Keyboard workflow](../crates/azad/docs/keyboard-workflow.md): user-facing keyboard behavior.
- [Keyboard state machine](../crates/azad/docs/keyboard-shortcut-state-machine.md): engineering contract for hotkey/listen-mode transitions.
- [macOS build notes](../crates/azad/docs/build-macos.md): local macOS build details.
- [Troubleshooting](../crates/azad/docs/troubleshooting.md): common runtime issues.

## ASR Runtime

- [ASR README](../crates/azad-asr/README.md): CLI use, model layout, and runtime dependencies.
- [ASR specification](../crates/azad-asr/SPECIFICATION.md): capture, VAD, streaming, incremental finalization, and embed contracts.

## Swift Helper

- [MLX/CoreML helper README](../crates/azad-mlx-asr/README.md): helper purpose, build command, and line protocol.

## Documentation Rules

- Cross-cutting plans and documentation indexes live under `docs/`.
- Crate-specific docs live in the crate they describe.
- Historical investigations should stay out of the active docs tree unless they describe current behavior or a still-open debugging playbook.
- Old fork/model notes belong in Git history or external backups, not in the public working tree.
