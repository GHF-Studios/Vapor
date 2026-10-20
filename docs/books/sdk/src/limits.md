# Limits

Vapor centralizes toolchain selection and output paths, but that does not make
every operation portable or self-validating.

- A managed build can fail when a target, linker, or platform SDK is absent.
- Host-only success does not prove another operating system works.
- Content validation checks metadata and graph constraints, not native-code
  safety or runtime correctness.
- Workshop creation depends on Steam permissions and provider availability.
- Source synchronization is not yet implemented by the source-sync command.
- A binary-only downloaded artifact cannot be forked into complete Rust source.
- Root-app publication and Workshop content publication remain different
  operations with different authority.

Use the result of the specific operation as evidence. Do not infer that a later
package, install, launch, or remote publish succeeded because an earlier build
completed.
