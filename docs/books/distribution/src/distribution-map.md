# Distribution Map

Vapor has distinct source, staged, installed, and remote states. They should
not be treated as one folder.

| Object | Origin | Destination | Operation |
| --- | --- | --- | --- |
| Vapor runtime | Vapor Client release | Standalone Vapor home | install / update / repair / uninstall |
| Loo Cast Packagepack | Loo Cast product source and target build | Steam app depot | validate / stage / SteamPipe publish |
| Content artifact | Registered source project and runtime outputs | Its configured content source | validate / package / publish |
| Player Library item | Workshop or another registered source | Local content cache | acquire / download |
| Installed Composition | Selected Packagepack and resolved dependencies | Installed content roots | install / update / repair |

The Steam app depot is a Loo Cast product mount, not a Vapor installation. For
each supported OS it contains one primary Packagepack executable entry point,
the Packagepack manifest, and only declared runtime sidecars required by that
target. It contains no Vapor runtime or mutable Vapor state. Other content
artifacts follow their own configured source and are acquired through the
Library and Composition lifecycle.

Vapor validates and stages exact files before upload. Steam uses a shared
manifest depot plus target-specific Linux and Windows depots. Each content
source can define its own target layout and authority rules.
