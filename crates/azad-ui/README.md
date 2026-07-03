# Azad UI

Swift AppKit package for Azad's onboarding and settings windows.

The Rust app remains the runtime owner. It serializes UI view models to JSON,
loads `libAzadUI.dylib` at runtime, and receives UI events back through a small
C callback. The Swift side owns only native window construction and control
rendering.

Build from the workspace root:

```bash
just swift-build
```

Run the local preview harness:

```bash
target/swift/azad-ui/release/azad-ui-preview
```
