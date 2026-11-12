# Exact Agentiflow child-catalog ownership

Base: `365bb3b09a8e9478d8daeb4c545c6a2dd91c1967`. This adds `agentiflow` to the existing child-paging root types. It does not create a RunStore record or infer ancestry from transcript tool output.

A retained local flow has its canonical record and child ledger in the Agentiflow store, whereas its scratch subagent transcript is under the run store. The earlier root resolver recognized canonical workflow, standalone and session records only. Its refusal was correct for the old supported types, but omitted this independently recorded ownership.

Qualification requires an exact local `<global>/agentiflows/<af_id>/agentiflow.json`, a matching typed record, a complete first physical `run_start` record at `lead/transcript.r0.jsonl`, exact flow identity, a nonempty safe project ID, and no contradictory host/project aliases. The canonical `children.jsonl` must exist and be regular. Metadata and first-record proof reads each cap at 256 KiB, use anchored regular-file opens, and do not read the rest of the lead transcript. Missing/incomplete proof remains unsupported; malformed or conflicting identity is rejected. No other lead round or store is searched to replace missing evidence.

Direct edges come only from the AF ledger. An explicit contradictory `tree` is rejected. Recorded child run, handle, round and transcript locator remain unchanged. Root scratch `sub/` and `step_results.jsonl` do not invent extra AF membership. Existing recorded descendants still use the established active/archive RunStore resolution and reject dual-directory ownership. Existing page parent, host, owner and cursor guards remain unchanged. A child's transcript still requires the existing native exact prologue/owner verification before navigation.

Snapshots retain record/directory identities, bounded hashes of the AF metadata and lead first record, and the absence of conflicting workflow/standalone canonical records. Continuations recheck those proofs (at most two 256 KiB proof reads), not the child ledger or descendant sources. Proof replacement, mutation or new conflicting canonical ownership expires the snapshot. A changed live AF metadata record may therefore require Refresh. Other root types retain their previous proof semantics.

There is no invented AF archive collection. If its exact canonical directory is absent, it does not receive this qualification. Agentiflow run-family text search remains explicitly unsupported; adding this root type must not accidentally broaden that separate feature's contract.

The native delta accepts the new closed enum value and retains exact root, parent, host, authorization run and locator checks; root type cannot change across a cursor. No new UI action, fallback or inferred child is added.

Validation prepared: nine backend fixtures covering real-shaped proof/edge, owner and aliases, prefix-only reads, malformed/oversized first record, missing proofs, conflicting canonical identities/tree, cursor proof/parent changes, symlink refusal, explicit search refusal and archived descendants. Native adds one exact AF identity fixture and includes AF in the existing closed-root-type matrix. Tests are not executed in this implementation checkout. Source parsing and patch application checks are separate from runtime and native navigation acceptance.
