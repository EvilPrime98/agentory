# 11. The Projects settings tab scrolls per pane

## Status

Accepted

## Context

Settings tabs share one page-level scroll area. The Projects tab shows a project list beside the selected project's settings.

## Decision

The Projects tab takes the page height and each pane scrolls on its own, without the page-level scroll area.

## Consequences

Content added to this tab must go inside one of its panes to be reachable by scrolling.
