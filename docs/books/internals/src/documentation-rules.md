# Documentation Rules

Public docs are built from mdBook sources and Shell guides. Each book's title
and description come from book.toml; SUMMARY.md defines its navigation. The
published index is generated from that metadata instead of a second hard-coded
book list.

For a public page:

- write for a named audience and task;
- distinguish current behavior from approved direction and unverified claims;
- explain ownership boundaries and the next safe action after failure;
- use commands only when present in the current Shell or GUI;
- link to rendered routes that exist in the deployed docs tree;
- remove placeholder markers before publication.

The docs publisher should validate the complete staged site before upload.
A failed build or placeholder check must leave the currently served tree
untouched. After a successful publish, verify the public index and at least one
page from each book.
