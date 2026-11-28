# 14. The default shell menu's reopen guard is per project

## Status

Accepted

## Context

A click on a menu button right after the menu closed itself is the same click, so it must not reopen
the menu. Every project has its own default shell button.

## Decision

The sidebar remembers which project's menu closed last, and the guard applies only to that project.

## Consequences

Closing one project's menu does not block the button of another project.
