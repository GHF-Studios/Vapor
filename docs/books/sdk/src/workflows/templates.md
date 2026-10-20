# Templates

Vapor's source templates create ordinary files that you can inspect, edit, and
commit. They do not hide content in a private database or require a separate
authoring format.

The built-in `basic-content` template creates one source workspace containing
an Engine, a Game, and a Packagepack. It writes the workspace and Cargo
registries, creates the role-specific manifests, and opens the result:

```text
source init basic-content PATH --organization ORG --name NAME
```

Replace `PATH`, `ORG`, and `NAME` with your chosen destination and stable
slugs. The target must be absent or empty. If you omit `--app-id`, the
template uses the installed app's Steam AppID for generated Workshop metadata.

The template is a starting point, not a complete game. Read each generated
manifest before publication; fill in meaningful titles, descriptions,
versions, runtime targets, dependencies, and Workshop metadata as needed.
Then run `content validate` and inspect any diagnostics.

To see the exact current template output, create it in a fresh temporary
directory and inspect the generated files. Do not copy an old generated folder
over an existing project: manifests and source files may have changed between
Vapor versions. Studio's creator flow is intended to make template selection,
project registration, and initial validation a single discoverable workflow.
