# Code Cleanup

Vapor is pre-alpha; do not preserve weak public seams for compatibility. Clean
up code by returning each behavior to its semantic owner, not by moving files
without changing ownership.

Before extracting a module or crate:

1. Name the state and invariant it uniquely owns.
2. Trace every caller and confirm there is one operation path.
3. Keep UI presentation out of providers and business rules out of facades.
4. Remove duplicate conversions, policies, and state mirrors.
5. Preserve filesystem, Steam, and toolchain boundary checks.
6. Run the owning managed validation path and update relevant docs.

A smaller diff is not automatically simpler if it leaves separate catalogs or
duplicated lifecycle behavior. Conversely, a generic framework is not justified
unless more than one real semantic owner needs it. Prefer a clear module tree
and short code paths that expose the actual domain hierarchy.
