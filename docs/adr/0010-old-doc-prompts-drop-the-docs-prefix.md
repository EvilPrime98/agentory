# 10. Old doc prompts drop the `docs/` prefix

## Status

Accepted

## Context

`{doc}` used to be relative to `docs/`, so saved prompts spell out `docs/{doc}`. It is now relative to the project root and includes the configured folder.

## Decision

The prompt is rewritten each time it is applied: `docs/{doc}` becomes `{doc}`, unless `docs/` is the tail of a longer folder name such as `mydocs/`. The stored text is not changed.

## Consequences

Old prompts keep working. A new prompt cannot put a literal `docs/` directory in front of `{doc}`.
