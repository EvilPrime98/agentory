## graphify

This project has a knowledge graph at graphify-out/ with god nodes, community structure, and cross-file relationships.

Rules:
- When graphify-out/graph.json exists, run `graphify query "<question>"` before your first grep, search or source-file read for a codebase question, and put this rule in every subagent prompt that explores code. Read raw files only for lines the graph or an earlier read already located. Use `graphify path "<A>" "<B>"` for relationships and `graphify explain "<concept>"` for focused concepts. These return a scoped subgraph, usually much smaller than GRAPH_REPORT.md or raw grep output.
- If graphify-out/wiki/index.md exists, use it for broad navigation instead of raw source browsing.
- Read graphify-out/GRAPH_REPORT.md only for broad architecture review or when query/path/explain do not surface enough context.
- After modifying code, run `graphify update .` to keep the graph current (AST-only, no API cost).
- A committed graphify-out/graph.json is rewritten whole on every build, so a large diff to it in a pull request is expected. Do not flag it as churn or suggest skipping the graph rebuild or its hooks.

## gpui layout

- Wrap every scrollable area in `Scrollbar::overlay` (`src/ui/components/scrollbar.rs`). A new wrapper around a scroll element follows the same shape: a `relative` container sized by its parent (`flex_1 min_h_0`), with the scroll element pinned inside it (`absolute`, all four edges at 0) so its size never depends on its content.
- Why: each flex level between a long list and its sized ancestor is another layout pass over every row, and gpui never caches `.truncate()` text, so it is re-shaped on each pass. A wrapper that nested the scroll element as a flex child made the GitHub PR list cost ~34 ms per frame; pinned, it costs ~5 ms. gpui re-renders the whole window on every OS window move, so that cost shows up as stutter while dragging.
- A pinned scroll element gives its container no height, so the container needs a definite size from its parent. A scroll area that should grow with its content (inside a `max_h` box, say) is a plain `overflow_y_scroll` div.
- Measure UI performance before changing code. Time render, layout and paint separately: a zero-size `canvas` as the root's last child marks when paint is reached. In this app layout has been the cost; `render()` itself is usually cheap.

## branches

Topic branches open PRs into `dev`; releases go `dev` to `main` via `/release-doctor`.
