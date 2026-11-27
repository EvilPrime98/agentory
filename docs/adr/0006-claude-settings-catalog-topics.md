# 6. Catalog topics are locale keys

## Status

Accepted

## Context

The settings catalog is generated from the official reference, and its topics are shown translated.

## Decision

Each topic id in the generator is also the locale key `claude_settings.topic.<id>`.

## Consequences

Adding a topic needs the key in every locale file.
