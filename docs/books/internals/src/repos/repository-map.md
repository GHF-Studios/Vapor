# Repository Map

The Vapor Client superworkspace contains the application-facing repositories.
The shared Vapor repository owns semantic types and reusable machinery;
Vapor-CLI, Vapor-GUI, and Vapor-Studio provide current frontends and
application modules. Vapor-Steam-Entrypoint owns Steam launch bootstrap.
Vapor-Examples contains example content. Loo-Cast is a separate game/content
workspace and is not the authority for generic Vapor behavior.

Vapor-Platform-Server is a separate deployment root. It composes independent
Homepage, Docs, Identity, Diagnostics, and Registry services behind the public
domain. The root server repository owns orchestration, Caddy, systemd, deploy,
backup, and recovery; each service repository owns its own data and business
logic.

The product direction now separates Vapor Launcher, Studio, and IDE as user
experiences over the same Client service. Do not assume they already have
separate repositories or binaries. Audit the actual manifests and source tree
before moving a crate or changing a Steam entrypoint.
