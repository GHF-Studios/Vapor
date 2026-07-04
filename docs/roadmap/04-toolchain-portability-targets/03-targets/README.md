# Targets

Status: open.

## Exists Now

- `vapor_core` currently lists Linux GNU and Windows MSVC as supported host/target triples.
- Windows GNU is discussed as a likely first cross-build proof from Arch/Linux, but the code model does not include it yet.

## Needed Next

- Split host triples, installed stdlib targets, linkable build targets, and release-supported targets.
- Decide whether `x86_64-pc-windows-gnu` enters the first proof.
- Keep `x86_64-pc-windows-msvc` visible as a later likely requirement.
- Make linker/tool requirements visible in status/check output before claiming target support.

## Total Target

Target support should mean the SDK knows the Rust stdlib target, the linker requirements, the proof status, and whether the target is suitable for release.
