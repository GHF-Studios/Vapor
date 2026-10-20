# A Small Vapor Tour

This tour uses the current Vapor Shell commands. The GUI is the normal player
and creator surface, but a few direct commands are useful for first-party
development and diagnosing a workflow. Steam must be running for live Workshop
search, subscriptions, and Steam-backed acquisition. Local source inspection
and fixture tests do not need Steam.

## Open a source workspace

From the installed Vapor Shell, open a workspace by path:

```text
source open /path/to/Loo-Cast
source status
content list
```

`source open` selects the source context. It does not copy files into the Steam
installation or change the selected composition. `content list` shows the
registered projects and their Vapor identities. The workspace manifest is the
authority for membership; Vapor does not infer that every nested manifest is
content.

## Validate and exercise local content

Check authored metadata and dependency shape before building:

```text
content validate
```

For a local end-to-end proof, the checked-in `content-roundtrip` script exercises
packaging, acquisition, installation, fingerprint verification, update, disable
and enable, repair, and uninstall against local fixture/provider state:

```text
script run content-roundtrip
```

This is a development operation. It uses Vapor-managed outputs and app-root
state; it does not publish a Workshop item. For quick local runtime iteration,
the content deploy command can explicitly select the host target:

```text
content deploy ghf-studios/loo-cast/loo-cast-packagepack --select --host-only
```

The default release path instead uses the workspace's declared runtime target
matrix. A Workshop release must include both supported Linux and Windows
targets; a host-only deploy is not a publish artifact.

## Find and use Workshop content

In Studio, search Workshop, inspect an item, and subscribe or acquire it. A
successful download appears in the local Library; it is not yet a change to the
composition you launch. If the item is a Packagepack, choose it as the root of a
composition. For other content kinds, add it only through a compatible pack or
dependency slot. Studio sends these actions through the shared Client service,
which uses the Steam client and then updates Vapor's local content records.

The Shell has direct content operations for development and recovery. Use
`help content` and `help launch` in the installed Shell for the command grammar
supported by that installation. For ordinary play, select a composition in the
GUI or launch the configured default:

```text
launch default
```

The service checks the composition's resolved state before handing off to the
game. It can acquire missing Workshop dependencies and refuses to launch when
the installed graph needs review or repair.

## Stage, review, and publish

For a local source project, stage a Workshop preview first. A dry run packages
content and writes provider preview data but does not upload anything:

```text
content publish ghf-studios/loo-cast/spacetime-engine ghf-studios/loo-cast/loo-cast-game ghf-studios/loo-cast/loo-cast-packagepack --dry-run
```

Real Workshop publication is a separate, explicit action. It requires the
Steam account, existing Workshop identity where applicable, successful target
matrix build, a reviewed staged artifact, and user confirmation. Studio's
review flow checks that the payload has not changed and uploads the exact bytes
that were reviewed. Do not use a script to bypass the interactive confirmation
for real Workshop changes.

## Learn the terms

The [ecosystem map](concepts/ecosystem-map.md) describes which component owns
each operation. The [content type guide](concepts/content-types.md) explains
the role names used by manifests and the interface.

