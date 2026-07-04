# Toolchain Cleanup

Status: wait for relocation evidence unless extraction is mechanical.

## Exists Now

- Toolchain status, layout, Rustup resolution, install planning, and install execution are close together.
- Rustup vs standalone archives is still provisional.

## Needed Next

- Run or define the relocation test.
- Split layout/status from install execution if the split is behavior-equivalent.
- Avoid naming a final backend until evidence supports it.

## Total Target

Toolchain code cleanly separates app-root layout, backend choice, verification, install, repair, and target support.
