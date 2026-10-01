# Vapor Profile

`vapor profile` is Vapor's single local performance-analysis product.

Tracy remains the native collector, capture format and deep interactive viewer.
Vapor does not silently acquire or launch Tracy during game execution.

## Runtime

`vapor run --profiling` selects the existing profiling Run Configuration and
marks the Development Session as an explicit profiling run for UI context. It
does not resolve, build, launch or capture with Tracy.

Open the profiling workspace separately:

```text
vapor profile
```

Use **Open Tracy** when you want the collector. Save `.tracy` captures manually
from Tracy as usual.

## Product surface

```text
vapor profile
vapor profile open [--address HOST:PORT]
vapor profile tracy [TRACE]
vapor profile import path/to/capture.tracy
vapor profile list
vapor profile report [TRACE] [filters...]
vapor profile setup
```

There is no `vapor monitor` command and no `vapor-monitor` executable. The GUI
binary is `vapor-profile`.

## Application

Vapor Profile replaces the old monitor UI rather than adding another tab.

- left: active-run context and manually saved trace library;
- center: live telemetry workspace or saved-trace statistical analysis;
- right: selected metric or Tracy-zone inspector.

Drag a `.tracy` file anywhere into the window to register it. Vapor stores the
path and available Development Session/workspace context; it does not duplicate
large trace files.

## Analysis

The first backend uses Tracy's official `tracy-csvexport --unwrap` output and
computes count, total, mean, p50, p90, p95, p99, max, source location and thread
ids, with inclusive/self-time plus name/thread/time-range filters.

If `tracy-csvexport` is missing, analysis reports that fact. Install that
optional helper explicitly with:

```text
vapor profile setup
```

This setup is never invoked by `vapor run`.

The next tranche should use Tracy's richer Worker data model for selected frame
ranges, thread names, bad-frame correlation, concurrency, critical path and
before/after comparisons behind the same `vapor profile` product.
