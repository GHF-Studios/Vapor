# Player Actions

The Launcher should present a small set of actions with plain player outcomes.
Each action is backed by the same shared service used by Studio, IDE, and the
CLI.

| Player action | Result |
| --- | --- |
| **Browse** | Search configured catalogs such as Steam Workshop and local content. |
| **Subscribe** | Follow a Workshop item through the current Steam account. |
| **Install** | Acquire an item and add it to a compatible Composition. |
| **Play** | Verify and launch the selected Composition. |
| **Update** | Review and apply newer versions to that Composition. |
| **Repair** | Verify installed files and restore damaged artifacts when possible. |
| **Remove** | Remove an item from a Composition or uninstall it when safe. |

Players should not need to understand cache records, package fingerprints,
Cargo, SteamCMD, source roots, or release staging to perform these tasks. When
an operation cannot proceed, show the item and Composition involved, the
specific blocker, and one safe next action.

The current CLI equivalents are documented in the
[Shell command guide](https://vapor.ghf-studios.site/docs/shell/commands.md). The player GUI still ships
inside Vapor Studio at this stage; the independent Launcher is approved
direction, not yet a separate binary.
