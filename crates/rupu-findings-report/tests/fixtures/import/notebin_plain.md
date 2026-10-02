Filename: NB-001 - Notes API returns another user's note by id.pdf

Notes API returns another user's note by id

Identifier: NB-001
Finding ID: fnd_01J00000000000000000000001
Owner: Unknown
Product: Notebin (sample app)
Affected Component: GET /api/notes/{id} handler
Source Repository: example/notebin
Existing Ticket References: None Provided
Impact: High
Category: Authorization Bypass Through User-Controlled Key
Attack Vector: Authenticated HTTP request with another user's note id
Likelihood: High
Risk Rating: Critical

Description
The get_note handler loads a note by the id path parameter and returns it without checking that the note belongs to the signed-in user.

Impact
Any signed-in user can read every other user's notes by iterating ids.

Location
Input:
GET /api/notes/{id} path parameter id.

Output:
The JSON body of the note, including title and body.

Root Cause
NoteStore::find_by_id is called with only the note id; the owner id from the session is never part of the lookup.

Call Chain / Attack Flow
router GET /api/notes/{id} (src/app.rs:12-30) → get_note() (src/routes/notes.rs:40-58) → NoteStore::find_by_id() (src/store/notes.rs:88-97)

Evidence
The handler looks the note up by id alone (src/routes/notes.rs:40-58):

```rust
let note = store.find_by_id(id).await?;
Ok(Json(note))
```

Remediation
Scope the lookup to the session user and return 404 when the note belongs to someone else.

Recommended Patch
```diff
--- a/src/routes/notes.rs
+++ b/src/routes/notes.rs
@@
-    let note = store.find_by_id(id).await?;
+    let note = store.find_by_id_for_owner(id, session.user_id).await?;
```

CI/CD Detection
Integration test: user B requests user A's note and must get 404. Run it on every merge request.

Regression Test
notes_access::other_users_note_is_404 creates two users and one note.
Command: cargo test --test notes_access other_users_note_is_404
Vulnerable build: fails: got 200
Patched build: passes: got 404

Cross-References
None

References
CWE-639: Authorization Bypass Through User-Controlled Key; CWE-862: Missing Authorization; OWASP A01:2021 Broken Access Control

CVSS v3 Base Score: 8.1
Risk Factor: High

Replication Steps
Step 1: Sign up as user A and create a note; record its id.
Step 2: Sign up as user B.
Step 3: As user B, request GET /api/notes/{id} with user A's note id.
