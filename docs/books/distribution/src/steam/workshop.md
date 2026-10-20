# Workshop

Workshop content is published one typed artifact per item. The item identity
and role manifest carry its version, dependencies, compatibility, title,
visibility, tags, and PublishedFileId. A Packagepack references Engine, Game,
and optional content by stable Vapor IDs.

For the first release of a dependency graph, create dependency items first.
After Steam assigns each item a PublishedFileId, repair the authored dependency
metadata before creating dependent items. Existing items update with content
publish; a reviewed preview shows the staged artifact and its dependency
information.

One Workshop item can contain Linux and Windows runtime output under the same
artifact root. The platform-specific files live under target-specific bin and
lib directories. Vapor does not split one content artifact into separate
Workshop items just because it supports multiple operating systems.

Workshop discovery, subscription, local download, Library availability, and
Composition membership are separate states. A successful publish updates the
remote item; it does not install it into the publisher's local playable
Composition.
