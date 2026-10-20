# The Vapor Distribution Book

This book explains how authored artifacts become installed or remotely
published products. It keeps three pathways distinct:

- the **Vapor root app** is shipped through Steam depots and SteamPipe;
- **Engine, Game, Mod, and Packagepack content** is published as separate
  Workshop items;
- **player installation** resolves those items into a local Library and
  Composition.

A release manifest describes authority and file membership. Vapor validates
and stages the payload before Steam receives it. The shared Client service
owns the operation and records its result; Steam supplies account and upload
authority.

Start with the [distribution map](distribution-map.md), then read the chapter
for the destination. Root releases use [SteamPipe](steam/steampipe.md);
content releases use [Workshop](steam/workshop.md). Identity and trust are
covered in the registry and trust chapters. The SDK and Shell guides explain
the commands that create a local preview.
