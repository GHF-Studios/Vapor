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

The root application contains Vapor frontends, resources, and the built default
Loo-Cast App runtime closure. The default closure is staged from reviewed content
packages into `content/default/`; it contains the Packagepack executable,
required manifests, and runtime assets, without source trees or mutable Library
state. Other Packagepacks and their dependency closures use the content lifecycle.

Vapor validates and stages exact files before upload. Steam uses a shared
manifest depot plus target-specific Linux and Windows depots. Each content
source can define its own target layout and authority rules.
