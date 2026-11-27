# 2. Claude settings are replaced atomically, keeping links and permissions

## Status

Accepted

## Context

A partially written `settings.json` would break Claude Code. Users may link the file from a dotfiles folder and restrict its permissions.

## Decision

Write a temporary file next to the target and rename it over the target. Resolve links first, and copy the old file to the temporary one before writing so it has the old permissions before it has the new content.

## Consequences

A linked file stays a link. The target folder has to be writable.
