# The Vapor Distribution Book

This book explains how authored artifacts become installed or remotely
published products. It keeps the runtime, product payloads, and content
artifacts distinct:

- the **Vapor runtime** is installed and updated independently of Steam;
- the **Loo Cast Packagepack** is mounted from its Steam app, with one primary
  executable entry point per OS plus only its manifest and declared runtime
  sidecars;
- **Engine, Game, Mod, and Packagepack content** is published through its
  configured content source, such as Workshop where supported;
- **player installation** resolves those items into a local Library and
  Composition.

A release manifest describes authority and file membership. Vapor validates
and stages each payload for its declared destination. Steam distributes the
Loo Cast Packagepack; it is not the Vapor runtime installer. The shared Client
service owns local operations and records their results, while each publishing
destination supplies its own upload authority.

Start with the [distribution map](distribution-map.md), then read the chapter
for the destination. Loo Cast Packagepack releases use [SteamPipe](steam/steampipe.md);
content releases use [Workshop](steam/workshop.md) where configured. Identity and trust are
covered in the registry and trust chapters. The SDK and Shell guides explain
the commands that create a local preview.
