# Core Crates

The shared Vapor workspace is separated by semantic ownership. Content kinds,
stable identities, compatibility, dependencies, and pack rules belong in the
content and manifest layer. Client domain types express read models and user
operations. Client protocol carries typed actions, snapshots, progress events,
and results across the local process boundary.

Application and service crates coordinate those contracts. Providers perform
Steam, filesystem, toolchain, Git, and documentation transport operations.
They return owned data and explicit failures rather than exposing external
handles or implementation internals to UI code.

The key rule is that a facade may format or coordinate a request but must not
reimplement the content graph, resolution algorithm, install state, or
publication lifecycle. Reuse the existing canonical model first. If a new
crate is proposed, document the state and invariant it uniquely owns.
