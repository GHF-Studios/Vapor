# Install A Packagepack

Install acquires a playable composition and its required dependency closure.
In the GUI, choose the item in Discover and select an eligible composition
destination. The shell provides the same service operation for a known content
ID or Workshop PublishedFileId:

```text
content install publisher/project/my-packagepack
```

For a numeric Workshop ID:

```text
content install 1234567890
```

Private or unreleased items may require an account:

```text
content install 1234567890 --account ACCOUNT
```

After installation, inspect the installed artifact and select a Packagepack
when it is intended to be playable:

```text
content list
content select publisher/project/my-packagepack
content selected
```

Installation and selection are distinct: an item can be in the Library without
being part of the active composition. For update/removal semantics, see
[Install, Update, Remove](../../launcher/workflows/install-update-remove.html).
