# Distribution Map

Vapor has distinct source, staged, installed, and remote states. They should
not be treated as one folder.

| Object | Origin | Destination | Operation |
| --- | --- | --- | --- |
| Vapor application | Vapor root source and built binaries | Steam app/depot | root package / root publish |
| Content artifact | Registered source project and runtime outputs | Steam Workshop item | content package / create / publish |
| Player Library item | Workshop or local source | Local content cache | acquire / download |
| Installed Composition | Selected Packagepack and resolved dependencies | Installed content roots | install / update / repair |

The root application contains Vapor frontends and resources; it does not embed
Loo-Cast Workshop content in its depot. A Packagepack is a content artifact,
not an app depot. Players install Packagepacks and their dependency closure
through the content lifecycle.

For either remote destination, Vapor validates and stages the exact files
before upload. The root app uses a split common/Linux/Windows depot model. A
Workshop item is one logical content artifact that may carry multiple runtime
targets under target-specific directories.
