Filename: NB-002 - Share links never expire.pdf

# Share links never expire

**Identifier:** NB-002
**Finding ID:** fnd_01J00000000000000000000002
**Owner:** Notebin core team
**Product:** Notebin (sample app)
**Affected Component:** Share link service
**Source Repository:** example/notebin
**Existing Ticket References:**
- Type: Example Tracker
  Identifier: NB-42
  URL: https://tracker.example.com/NB-42
  Notes: Product team tracking remediation
**Impact:** Medium
**Category:** Insufficient Session Expiration
**CWE:** CWE-613
**Attack Vector:** Replaying an old share link
**Likelihood:** Medium
**Risk Rating:** Medium

## Description

Share links are signed tokens with no expiry claim, so a link keeps working after the owner stops sharing the note.

## Impact

Anyone who ever received a link can read the note for as long as it exists.

## Location

**Input:** `GET /s/{token}`

**Output:** The shared note page.

## Root Cause

`ShareToken::verify` checks the signature but the token format has no `exp` field to check.

## Call Chain / Attack Flow

Step 1: `GET /s/{token}` route (`src/app.rs:44-46`)
Step 2: `open_shared()` (`src/routes/share.rs:10-22`)

```rust
let claims = ShareToken::verify(&token)?;
```

Step 3: `ShareToken::verify()` (`src/share/token.rs:30-41`)

## Evidence

- The token claims have no expiry (`src/share/token.rs:5-9`).
- Verification never compares a time (`src/share/token.rs:30-41`).

```rust
pub fn verify(t: &str) -> Result<Claims> { check_sig(t) }
```

## Remediation

Add an `exp` claim when a link is minted and reject expired tokens in `verify`.

## Recommended Patch

```diff
--- a/src/share/token.rs
+++ b/src/share/token.rs
@@
-pub fn verify(t: &str) -> Result<Claims> { check_sig(t) }
+pub fn verify(t: &str) -> Result<Claims> { check_sig(t).and_then(check_exp) }
```

Existing links minted without `exp` should be treated as expired.

## CI/CD Detection

**Stage:** nightly

A test mints a token with a past `exp` and requires `verify` to reject it.

**Command:** cargo test --test share_expiry

**Fails when:** an expired token verifies

## Regression Test

`share_expiry::expired_link_is_rejected` mints a token that expired an hour ago.

```sh
cargo test --test share_expiry expired_link_is_rejected
```

**Vulnerable build:** fails: token accepted

**Patched build:** passes: token rejected

## Cross-References

- fnd_01J00000000000000000000001 is a prerequisite: share pages load notes through the same store.
- NB-007 covers link revocation.

## References

OWASP A07:2021 Identification and Authentication Failures

**CVSS v3 Base Score:** 6.5
**Risk Factor:** Medium

## Replication Steps

Step 1: As user A, share a note and copy the link.
Step 2: Stop sharing the note.
Step 3: Open the copied link in a private window; the note still loads.
