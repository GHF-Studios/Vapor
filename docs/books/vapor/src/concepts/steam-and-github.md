# Steam And GitHub

Steam and GitHub provide different services to Vapor.

## Steam

Steam delivers Vapor's app and provides Steamworks user context and Workshop
operations. Steam Workshop can be searched and subscribed to from the running
client, and its content can be acquired into Vapor's local Library. A creator
can publish a staged release through the authorized Steam account after
reviewing the payload.

Steam owns subscription state and Workshop transfer state. Vapor owns its local
content records, composition selections, installed graph, and verification
results. A Steam item showing as subscribed does not by itself prove that the
Vapor release is installed or selected for play.

## GitHub

GitHub hosts source repositories and supports developer source workflows.
A creator edits a local checkout or Workspace, then Vapor builds and packages
its content. The source repository is not the runtime install: players acquire
the versioned artifact from a configured distribution provider rather than
cloning the creator's Git working tree.

First-party Steam app-root publishing and user content publication are separate
authority paths. An app-root release can update the installed Vapor application
or Shell, while a Workshop release updates one logical content artifact. The
Studio confirmation surface must make that target clear before submission.

## Why both are present

GitHub is a source and development boundary; Steam is an application and
Workshop distribution boundary. Using both does not make them interchangeable.
Vapor connects their workflows through stable content identity, reviewed
artifacts, and an explicit local service, while preserving the permissions and
state each platform owns.

Explain Steam distribution, Workshop content, and GitHub-backed registry authority at a high level.

