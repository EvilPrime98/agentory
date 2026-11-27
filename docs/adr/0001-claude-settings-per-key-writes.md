# 1. Claude settings are changed one key at a time

## Status

Accepted

## Context

Claude Code and text editors change the user's `settings.json` while the settings page is open.

## Decision

Each write reads the file again and changes only the requested key. List entries are removed by value when their position no longer matches. An unreadable or invalid file is left untouched. The open page polls the file.

## Consequences

Keys changed elsewhere survive a write from the app. A list entry cannot be addressed by its row index alone. A broken file has to be repaired outside the page.
