# Installed State And Library

The Launcher distinguishes content that is **available on this device** from
content that is **installed for a Composition**. The Library is the first
collection; the Composition's installation is the second. A release can be
cached and reusable without being part of the game setup currently being
launched.

Vapor stores provider downloads, cached packages, installed artifacts, disabled
artifacts, quarantined corrupt copies, and generated receipts under its owned
app-root state. These are managed by the service. Their folder names are useful
to developers and recovery tools, but normal player workflows should say
**Download**, **Install**, **Disable**, **Repair**, and **Remove** rather than
asking the user to manage cache directories or local records.

The installation record includes exact content versions, dependency edges,
provenance, and payload fingerprints. The service uses those facts to identify
which release a Composition uses, avoid deleting shared dependencies, and
detect local modification. The Library shows what can be installed; the
Composition shows what will actually launch.

Removing one Composition must not remove a cached item still required by
another Composition. Auto-Install content can be collected when no Composition
needs it. Content without Auto-Install may remain cached for later composing or
development use.

