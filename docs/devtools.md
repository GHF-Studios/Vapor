# Vapor Devtools

Vapor Devtools is Vapor's local runtime debugging, observability and
performance-analysis workbench.

It is not a profiler implementation. Tracy remains the native profiler and
collector. Devtools consumes the same role-owned developer environment as every
other Vapor development workflow.

## Developer experience

A normal developer setup is:

```text
install Vapor through Steam
run the Vapor installer
vapor role promote content-developer
# or ecosystem-developer
```

Promotion reconciles the complete developer toolset. There is no separate Tracy
setup step and no profiling-analysis setup flag.

The same declared environment can always be checked or repaired through:

```text
vapor toolchain status
vapor toolchain diagnose
vapor toolchain repair
vapor installation repair
```

## Runtime and Tracy

`vapor run --profiling` selects the profiling Run Configuration and emits the
ordinary Vapor Development Session / telemetry context. It does not install or
launch Tracy.

`vapor toolchain tracy` launches the already-managed pinned Tracy profiler. If
Tracy is missing, the developer environment is degraded and should be repaired
rather than silently performing setup from a feature command.

## Devtools CLI

```text
vapor devtools
vapor devtools open [--address HOST:PORT]
vapor devtools trace import capture.tracy
vapor devtools trace list
vapor devtools trace report [TRACE] --top 50 --sort p95
```

`trace report` supports thread-id, trace-relative time, zone-name and
inclusive/self-time filtering plus table/CSV/JSON output. The CLI and GUI use
the same analysis backend.

## Devtools application

The `vapor-devtools` app combines active Development Session context, live
telemetry/logs/snapshots, saved Tracy artifacts, aggregate zone statistics and
source-aware inspection. Saved traces are one evidence source alongside live
runtime state, not the identity of the app.

## Deployment identity

The installed application binary is `vapor-devtools`. Superseded
`vapor-monitor` and `vapor-profile` binaries are removed by local deployment.

A Git commit/push does not replace the installed Vapor App Instance. After
changing Vapor Client source, run `vapor client deploy local` before testing the
installed command surface.
