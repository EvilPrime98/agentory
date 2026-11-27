# 7. Tooltips set their own text style

## Status

Accepted

## Context

gpui draws tooltips outside the app root, so they inherit none of its text style.

## Decision

The tooltip view sets its own font family, text size and text colour.

## Consequences

A change to the app root's text style does not reach tooltips.
