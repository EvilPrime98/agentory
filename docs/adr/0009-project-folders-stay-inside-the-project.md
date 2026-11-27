# 9. Configurable folders stay inside the project

## Status

Accepted

## Context

The docs and issues folders are typed by the user and joined to a project root, where files can be read and trashed. Windows drops trailing dots and spaces from a path component, so `.. ` reaches the parent, and `:` names a drive.

## Decision

`normalize_folder` accepts only a relative path below the project root and stores it `/`-separated. It rejects the root itself, absolute paths, components ending in a dot or a space, and components containing `:`. Where a folder is joined to the root for paths that are shown or put in a prompt, it is joined one component at a time so they use the OS separator.

## Consequences

Folders outside the project, and folder names ending in a dot, cannot be configured.
