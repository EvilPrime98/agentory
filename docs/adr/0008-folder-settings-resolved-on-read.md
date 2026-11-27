# 8. Folder settings are resolved on read

## Status

Accepted

## Context

The docs and issues folders have an app-wide setting and an optional per-project override. Both live in JSON files that can be edited by hand. A field of the wrong type would otherwise fail the whole file.

## Decision

A stored folder value is not trusted. A value of the wrong type is ignored while loading. `Folder::resolve` takes the project override, else the app-wide value, else the built-in name, and skips invalid ones. Code that needs the folder in use reads it through `Folder::resolve`, and the settings CLI reports what the app uses rather than the invalid stored text.

## Consequences

A bad hand edit falls back silently. The raw stored fields must not be used as paths.
