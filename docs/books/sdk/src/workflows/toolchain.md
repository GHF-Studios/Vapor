# Toolchain

Vapor pins a Rust toolchain for its managed workflows and installs it inside
Vapor-owned application data. The version is selected by the Vapor release;
the user does not need to install Rust globally to use an installed Vapor
application.

The managed toolchain is used for operations such as workspace checks, tests,
builds, Rustdoc, authored-content realization, and root-app builds. The exact
host targets and environment are derived from the app's managed configuration.
Use Vapor commands so the selected compiler and output locations remain
consistent.

A development installation may include additional cross-compilation
requirements for a declared runtime matrix. For example, publishing a package
for both Linux and Windows requires the configured Windows target and linker
support. A host-only local deploy is useful for a quick smoke check, but it is
not publication proof.

Vapor can report installed toolchain and target state through its toolchain
commands. Prefer those diagnostics over manually replacing files in the
managed toolchain directory. If a managed tool is missing or damaged, use the
Vapor repair/setup workflow so the app's pinned version and target policy are
restored together.
