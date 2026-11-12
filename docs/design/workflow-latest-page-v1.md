# Newest bounded workflow page

Host capability `workflow.observation.latest_page.v1` adds an opt-in Boolean
`latest` to GET `/api/runs/:id/observation/page`; it defaults to false. Existing
requests and prefix pages retain their behavior.

`latest=true` requires an immutable capture whose window is `latest`, and section
`results`, `events`, `checkpoints`, or `attempts`. It excludes `cursor` and `parent`.
`step_id` retains its exact results-only filter and 1..1024 UTF-8 byte limit.
Invalid combinations return400. Existing owner, snapshot and limit1..100 checks
apply. No new filesystem IO or capture is performed by this selector.

The server chooses the newest suffix within both the requested row limit and
existing1MiB response budget, then returns those rows in chronological order.
The reply uses the existing exact owner/snapshot/source_generation/window/frontier,
section/parent/step_id provenance and adds `selection:"latest"`. The returned
`offset` is the first row's index in the captured filtered selection, and
`offset + records.count == captured_total`. `next_cursor` is null. An empty
selection has offset0 and captured_total0; a nonempty selection always includes
its final row. Rows remain individually bounded by the existing64KiB capture cap.

`captured_total` counts only captured matching rows. `source_complete`, nullable
`source_total` and `unassigned_rows` are unchanged; a latest suffix, malformed row
or unknown earlier history cannot manufacture a known underlying total. `offset`
is the exact number omitted before this page within the captured selection;
header head/trailing/partial evidence describes history beyond the capture.

Clients use this selector for live overview log sections. Declared steps/edges
still use their ordinary bounded topology pages. Older history remains reachable
through ordinary pages on the immutable capture, or a separately retained prefix
capture; choosing newest never changes a selected transcript or pinned historical
page. Clients do not derive opaque cursors from counts or scan pages to find new
activity. Refreshing a live capture after a changed frontier remains explicit.

Four authored fixtures cover more than one page below the tail-byte cutoff, exact
step filters with unknown totals, response byte limits retaining the final row,
and HTTP rejection of prefix/mixed selectors. Source-only; not yet executed.
