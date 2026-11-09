# Next backend source union

Source-only composition against immutable `fd17487a84df239dab5fdff902e610efb9433bc0` (tree `9173c2409d8cb9a9621b0421a2d64eb560eec4e2`). The composition used an isolated checkout; the active checkout was not edited. No builds, tests, provider API calls, services, publishing, or actual-store mutations were performed by this integration lane.

## Frozen inputs

| Slice | Patch SHA256 | Input base |
| --- | --- | --- |
| G4 guarded forge writes | `0615a7c458e075fc6d3d05ba3e8aed76d5352cae3e8db00b766fc6aca94e0a5f` | `d16fe6dd1240d0ff763f8c440706a2a3db255124` |
| G5C exact-run event scan | `3d0f6907fc8ec0971e736837970f89aa989b389513c8388e829ef0c4b7274a97` | `a42e3beaa5736998fabcf9513976545044e326de` |
| G8 agent source repair | `0d2b2c2e67c8e1189b36a7a99ab5ff941d8426356823312ac1136dff49737fb0` | `a42e3beaa5736998fabcf9513976545044e326de` |
| Issue discussion continuation | `f7888b1e614be319123397c382411f74c045cb49c94df7b25e34a6dcba4a22bf` | `16cfcae7f8f4a5ccf9fe5b180c273f36ffe0b81a` |

## Additive conflict decisions

- Retain every current host feature/condition, including retention, G2 usage, G1 reads, G3 review pages and G6 import. Add guarded forge writes, event scan, agent repair and supported discussion capability independently.
- Retain every current module and server route, adding forge and discussion routers separately. Event scan is merged by its reviewed events router.
- Retain all existing MCP names and platform filters. Add two Unix-only agent source/repair tools and two cross-platform issue-discussion tools. The pinned catalog becomes 211 on Unix and 205 otherwise; previous retention and usage filters remain. The five G4 HTTP routes remain explicitly excluded.
- The frozen G5C patch omitted its new `POST /api/events/scan` from route inventory. Add that exact route to `EXCLUDED` and the pinned exclusion fixture with a native-operator rationale. No event scan model/operator tool or grant is added.
- Retain both forge and discussion methods in `IssueConnector`, four issue adapters, and GitHub/GitLab clients. All seven conflicted SCM files were reconstructed from the current side plus the exact added blocks from the reviewed discussion input. Constructor fields and construction calls remain separate. GitLab writes use atomic token/epoch capture; discussion retains its fail-closed epoch bracket. Neither path uses the other's transport.
- G1/G3 reader and transport/compiler corrections from the baseline remain present. All G4 private store, write lifecycle, provider dispatch and fixture files are byte-identical to their reviewed export. All discussion provider/CP modules are byte-identical to their reviewed export.

No scoped-read or Ask work is included; those are separate iterations. Component source reviews establish only their reviewed source. The union needs its own integration source review and subsequent compilation/fixtures in the designated warm lane.

## Focused validation handoff

The frozen source inputs author 115 new fixture functions: G4 40, G5C 29, G8 repair 19, discussion 27. Counts exclude existing fixture extensions and older tests. None was executed here.

| Package/target | Filters |
| --- | --- |
| `rupu-cp --lib` | `forge_write_store`, `api::forge_writes`, `api::event_scan`, `api::agents::source`, `api::issues_discussion` |
| `rupu-cp --test it` | `mcp_routes`, `mcp_ops_definitions`, `mcp_ops_scm` |
| `rupu-scm --lib` | `forge_write`, `token::tests`, `issue_discussion` |
| `rupu-cli --lib` | `cp_forge_writer`, `cp_issues` |

Run default and supported no-default configurations in the owning validation lane. Preserve prior G1/G2/G3/G6/retention validation coverage when assembling the next candidate. Every imported slice retains its original product bounds, uncertainty semantics and provider qualifications; this composition adds no new live acceptance claims.
