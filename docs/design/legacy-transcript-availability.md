# Legacy transcript availability

`GET /api/transcript` distinguishes an absent local recording from an existing
empty file. Missing local files, directly read mirror caches, and missing files
in the local host-connector fallback return HTTP 404 with
`{"error":"recorded transcript is not available yet"}`, matching the paged
endpoint. Existing empty files still return HTTP 200 with an empty event list.
The path and host guards run before reading, and parse-error/torn-tail handling
is unchanged. This response does not distinguish not-yet-written recordings
from deleted recordings or infer the run's execution state.

`/api/transcript/stream` can still be opened before a recording exists. A client
must retain the explicit read failure until real events arrive or a subsequent
read succeeds, including a successful read of an existing empty file. Either
can clear this specific missing-recording error. A late REST failure must not hide events
already received by the stream. Token, path, unknown-host and other errors are
not equivalent to this condition. The CP web panel handles both orderings;
native session clients also need to isolate historical-turn load failures so
that they still load later turns and subscribe to the current turn's stream.
Clients polling a running turn's recording must keep the exact missing-file
response retryable while following that run, without clearing authorization or
other failures, changing ownership, or bypassing cancellation.

Transport limits remain explicit. HTTP host proxies return 404 but their
generic connector currently replaces the remote error body with its URL, so
the specific missing-recording recovery signal is not preserved through that
proxy. Connect directly to the updated CP for the exact error contract. SSH's
existing missing-file sentinel may create an empty, complete cache; this
change does not reinterpret those caches or change remote pull semantics.
Neither case proves that an empty response contains a complete historical
conversation. No transcript events are reconstructed from findings or other
records.
