# Permissions

Permissions are checked at the authority boundary that performs an operation.
Local validation does not grant Steam publish permission; being signed in to
Steam does not authorize a claim to another organization's Vapor namespace.

A content release may need several distinct approvals:

- The author can edit the source workspace.
- The registry authority can validate the declared namespace and repository.
- The Steam account can create or update the target Workshop item.
- The app build account can upload the Vapor root app to its configured branch.

Preview operations do not change remote state. Real publication requires an
explicit account and confirmation. Keep credentials in the local authentication
provider and service environment, never in source manifests or released files.
A failure should identify which authority rejected the operation without
printing secrets.
