# Ubiquisync

Conflict free sync over commodity cloud storage or server.

Ubiquisync solves the problem of syncing user workspace data between devices without
merge conflicts and without the need to stand up any sync server infrastructure.

It will allow you to sync data stored in SQLite over commodity cloud storage such as Google Drive, iCloud Drive, DropBox or a dedicated sync server.

## Features

Ubiquisync might be a good fit if your app could benefit from these features:
- local-first, offline data
- SQLite data storage and querying
- full revision history
- reactive updates
- sync over Google Drive, iCloud Drive, DropBox, etc. OR a dedicated sync server
- conflict-free merging of rich document content (a la Google Docs)
- user-defined schemas (a la AirTable, Notion)

## Caveats

Ubiquisync might not be a good fit if any of these things apply:
- needs fine grained read or write permissions
- manages a huge volume of data and maintaining history would bloat it
