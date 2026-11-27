# 3. When the Claude settings page writes

## Status

Accepted

## Context

Writing typed text on every keystroke would store half-typed values, while a toggle or a choice is complete as soon as it is made.

## Decision

Toggles, dropdowns and list changes write at once. Typed text writes on its Save button, and unsaved text survives topic switches and reloads of the file. An object whose parts have rows of their own opens its JSON box only on request. A one-line editor cannot show newlines, so an untouched field whose value gains one is rebuilt as a multi-line editor.

## Consequences

Editors live as long as the page, and a reload only overwrites fields the user has not edited.
