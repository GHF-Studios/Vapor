# Registry Authority

A Vapor Registry is an infrastructure authority over declared organizations
and identities. It is not a content workspace, a Cargo registry, or a
Steam account. The registry verifies that submitted identities belong to an
authorized namespace and that first-party declarations match expected sources.

A manifest's organization string is only a claim. It does not grant the author
permission to publish under that organization's identity. Publication requires
the relevant registry or Steam authority and must be checked at the operation
that changes remote state.

Vapor's local artifact identity remains stable across source, staging, Workshop,
Library, and Composition. A registry can confirm source authority; Steam can
confirm Workshop ownership and account permissions. Neither provider replaces
the local service's dependency resolution or install verification.
