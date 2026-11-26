# Limits

Vapor can validate its own manifests, target matrix, dependency graph, and
staged artifact. It cannot guarantee that an external provider remains
available, a Steam account retains permission, or a game binary is safe.

A preview is not a remote upload. A successful SteamPipe or Workshop upload is
not a runtime test. A build on Linux does not prove a Windows artifact works.
Fingerprints detect payload changes but do not certify author intent.

Vapor maintains one local Cargo profile (`dev`) and one distribution profile
(`release`). Release-profile builds and real SteamPipe or Workshop uploads are
accepted only inside the designated GitHub Actions distribution job, marked
with `VAPOR_DISTRIBUTION_JOB=steam-release-v1`. Keep Steam publishing
credentials scoped to that job; the process marker is an operational guard,
not a security boundary against a machine owner.

The Publisher product split is also in progress: Launcher, Studio, and IDE are
approved separate frontends, but the root Steam entrypoints still reflect the
current bundled GUI. Do not infer a newly approved product boundary is already
packaged or shipped.
