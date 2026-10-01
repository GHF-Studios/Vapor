# Source-authoritative local Client deployment

`vapor client deploy local` is a self-hosting deployment transaction.

## Invariant

The source version being deployed owns interpretation and reconciliation of its
own distribution.

An older installed `vapor` may bootstrap the canonical `vapor` source binary,
but it must not decide which additional binaries or cleanup rules belong to a
newer source version.

## Protocol

1. The installed CLI builds only the stable `vapor` authority binary from
   current source into Vapor-managed development output.
2. It launches that source-built binary directly.
3. The child receives the active App Instance through `VAPOR_HOME` plus
   `VAPOR_LOCAL_DEPLOYMENT_AUTHORITY`, containing its exact canonical executable
   path.
4. The source-built CLI re-enters `vapor client deploy local`, verifies that it
   is the designated authority, and performs full distribution reconciliation
   using its own source version.
5. The already-built authority executable is reused as the `vapor` distribution
   input instead of being redundantly rebuilt.

The authority runs outside the App Instance it replaces, so deployment does not
depend on overwriting the currently running executable.

## User experience

After the protocol is installed:

```text
vapor client deploy local
```

is one invocation. No second deploy and no shell `hash -r` are part of the
model.

## One-time migration

An installation predating this protocol cannot retroactively know how to
transfer authority.

For that one crossing, run the current source CLI through the existing managed
Cargo environment:

```text
vapor toolchain cargo --project Vapor -- run --package vapor_core --bin vapor -- client deploy local
```

That installs a Vapor version supporting source-authoritative local deployment.
Every subsequent deployment uses the normal single command.
