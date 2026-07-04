# Code Cleanup And Maintainability Roadmap

Status: working roadmap.

Goal:
Make code easier to maintain without smuggling policy decisions into names, comments, rustdoc, or command text.

## Map

1. [SDK core cleanup](01-sdk-core-cleanup/README.md)
2. [Toolchain cleanup](02-toolchain-cleanup/README.md)
3. [Doc comments](03-doc-comments/README.md)

## Exists Now

- SDK root and Steam modules are split.
- SDK workspace and toolchain modules still carry several responsibilities.
- Launcher is smaller but mostly placeholder.

## Needed Next

- Split large SDK workspace files behavior-equivalently.
- Split toolchain layout/status/install logic once relocation pressure is understood.
- Keep comments/rustdoc factual unless intent wording has been discussed.

## Total Target

Code structure should make the next real workflow easier to implement and test without pretending unsettled policy is settled.
