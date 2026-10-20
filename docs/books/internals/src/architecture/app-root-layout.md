# App Root Layout

The installed app root is discovered from the running Vapor executable. It
contains replaceable application payload such as binaries, resources, and
installed documentation. Mutable user state and provider/toolchain data are
managed separately from Steam-owned files so an app update does not erase
authoritative source or service state.

At a high level:

```text
app root/
├── App.vapor.toml
├── bin/<target>/                 platform executables
├── resources/vapor/              scripts, templates, managed tools
├── docs/                         generated Rustdoc and installed books
├── output/dev/                   managed development outputs
├── output/release/               managed release outputs
└── content/                      cache, installed items, disabled items
```

Exact paths evolve and some mutable state lives outside Steam's install
directory. Consult the current InstallationPaths and content-layout code before
mutating paths. Use typed Vapor operations for cleanup and repair; manual
deletion can invalidate indexes and operation receipts.
