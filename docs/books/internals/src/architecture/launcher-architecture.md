# Launcher, Studio, And IDE Architecture

The approved product direction gives players a separate Launcher, creators a
Studio, and code authors an IDE. CLI/Shell remains a support and recovery
facade. All request the same Vapor Client service and canonical model.

The current implementation has not completed this split: Vapor Studio still
contains player and creator areas, while IDE code authoring is not yet a full
separate product. The architecture work should first preserve shared service
ownership, typed progress, cancellation, and result semantics; then extract
audience-specific surfaces without copying domain logic.

Launcher should not acquire creator navigation, source manifests, Rust/Cargo,
publishing, or app-root internals by default. Studio should explain creator
actions and review staged artifacts. IDE should own source editing and
developer-facing navigation, not a second content resolver or install state.

Review Steam mappings and root-app packaging before changing entrypoint modes.
Product names do not imply a shipped executable.
