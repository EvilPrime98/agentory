# 12. The New agent session shortcut binds first

## Status

Accepted

## Context

A user may have given the default combo of New agent session to another action before New agent
session existed. In gpui, later bindings take precedence.

## Decision

`bind_keys` binds New agent session before every other action, so the user's earlier binding wins.

## Consequences

When two actions hold that combo, New agent session does not fire. Moving its binding down the list
takes the combo away from the user's action.
