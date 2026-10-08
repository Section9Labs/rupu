# F1c: Messages UI (Messages tab everywhere + messages in transcripts)

- **Card:** F1c · **Depends on:** F1a, F1b · **GUI card:** show matt a mock before building, and stop for his visual check before the PR ([[feedback-show-crux-visual-early]])
- **Overview:** [F1-messaging-everywhere.md](F1-messaging-everywhere.md)
- **Closes:** M1 (UI side); matt's asks 2 + 3

## 1. Goal

1. A **Messages tab** on workflow run detail, session detail and agentiflow detail. It shows **every** message kind from the log, with who→whom, delivery status and receipts, plus a composer that can reach anyone.
2. **Messages inside transcripts**: each agent's transcript shows the messages it sent and received as identifiable chat bubbles, right where the agent saw them. Each bubble is cross-linked with the channel view.

## 2. Starting point

`crates/rupu-cp/web/src/components/agentiflow/MessageFeed.tsx` (#805) already has:
- a compact group-chat layout: role-tinted avatars, `@mention` pills and inline mention highlighting;
- a participants bar, kind pills, day dividers and show-more;
- pinned directives with retractions folded;
- `SteeringBox` (Send / Now).

It reads `GET /api/agentiflows/:id/messages` (posts + directives only). The run detail page (`pages/RunDetail.tsx:65`) has the tabs `transcript | events | findings | cycles | netflow`. Transcript rendering lives in `components/transcript/` (`Turn.tsx`, `ToolCard.tsx`, `transcriptView.ts`, `subrunIdentity.ts`).

## 3. Design

### 3.1 `MessageFeed` becomes source-agnostic

`MessageFeed` moves to `components/messages/MessageFeed.tsx` and takes `{ source: { kind: 'run' | 'session' | 'agentiflow', id, host? }, live }`. It reads F1b's `GET …/messages` (paged) plus `…/messages/stream` (live). It renders **all** log kinds:

| Kind | Row treatment |
|---|---|
| `post` (`to: null`) | a channel post (today's look) |
| `post` with `to` | a channel post with an `@to` pill and a left accent (today's directed look) |
| `direct` | a **DM row**: `lynx#1 → otter#2` header with a "direct" badge and a subtle DM background. Hidden by default when the "channel only" filter is on |
| `broadcast` | a megaphone badge and full-width emphasis. This is the **only** thing labelled broadcast (M5) |
| `operator` | the operator avatar with the operator accent and an `URGENT` badge when urgent; the recipient pill |
| `directive` / `retract` | pinned in the directives panel (today's behaviour). The feed shows a compact "📌 directive posted / retracted" marker |

Every row also shows:
- **delivery:** amber `undelivered: <reason>` chips (e.g. "no live participant for role:recon");
- **receipts:** a quiet "seen by otter#2 · turn 7" line, collapsed to "seen by 3" when there are more than 2;
- **links:** "↗ in transcript" opens the sender's transcript at that message.

**Filters:** kind toggles, a participant filter (from the participants bar), and "involving <participant>".

**Paging:** pages of 200, older pages loaded on scroll, newest at the bottom, auto-scroll only when already at the bottom. Logs can run to thousands of lines, so render with windowing (same approach as the netflow virtual table) once there are more than 500 rows.

**Legacy:** with `legacy: true` (a pre-F1 flow) a note "messages before <date>: board posts and directives only" is shown, so the absence of DMs isn't misread.

### 3.2 Composer (replaces `SteeringBox`)

- **Recipient picker** with grouped options:
  - **Everyone:** `broadcast` (and `lead` for flows)
  - **Participants:** live first, then left greyed; avatar + codename
  - **Roles:** `role:<x>` with a live count
  - **Steps:** workflows only, `step:<id>` with a live count
- **Defaults** as in F1b: lead for flows, broadcast otherwise.
- **Urgent toggle**, the replacement for "Now", with a tooltip: "delivered first, and ends an agentiflow lead's round early".
- **Gated:** disabled with a reason when the run isn't live (the `409` from F1b) or the host returns `501`.
- **After sending**, the message appears in the feed through the stream, never optimistically. It shows "pending" until its log line arrives (the pending-state rule from the macOS work; same principle).

### 3.3 Where the tab appears

| Page | Tab | Shown when |
|---|---|---|
| `RunDetail` (workflow runs, standalone runs) | **Messages** (with an unread-style count of messages since the page opened) | `space: "present"` in the API response, or the workflow has `messaging:`. An absent space shows no tab; a hidden empty tab is better than a dead one |
| `SessionDetail` | Messages | space present |
| `AgentiflowDetail` | Messages (existing tab, now on the new API) | always |

### 3.4 Messages in transcripts

- **`Event::Message` in `Turn.tsx` / `transcriptView.ts` is a chat bubble.**
  - **Received:** left-aligned, sender avatar tinted with the codename crew colour from `lib/codenamePalette.gen.ts`, a kind badge, `→ you` (or `→ role:recon`), and an urgent / operator accent.
  - **Sent:** right-aligned outline bubble, `you → otter#2`, plus delivery chips taken from the event's `delivery`.
  - **Links:** each bubble opens the Messages tab anchored to `#msg=<id>`, and "open sender's transcript" opens the other side.
- **The messaging tool cards are folded.** The `ToolCall` / `ToolResult` pair for `msg.send` / `board.post` collapses *into* the sent bubble, so it doesn't render twice. A "raw" toggle still shows the tool card.
- **`Event::Injected` that isn't a message** (roster, children status, directives) renders as a collapsed chip: "context injected · roster (12 lines)". Expanding it shows the content exactly as the model saw it.
- **Anchors:** `#msg=<id>` on the transcript page scrolls to and highlights the bubble, alongside the existing `#call=` hash (`parseCallHash`).

### 3.5 Mock-first

**Before any implementation**, the session shows matt a static mock built from a realistic fixture log, covering:
- a workflow with three participants;
- one DM, one broadcast, one undelivered role message;
- an urgent operator message;
- a directive and its retraction;
- seen receipts;
- plus one transcript turn with a received bubble, a sent bubble and an injected-context chip.

Use an artifact page or a Storybook-style route. Build only after matt approves the look.

## 4. Files

| File | Change |
|---|---|
| `web/src/components/messages/{MessageFeed,MessageRow,Composer,RecipientPicker,DirectivesPanel,ParticipantsBar}.tsx` | moved and extended from `components/agentiflow/MessageFeed.tsx` (split for size) |
| `web/src/lib/api.ts` | `getMessages(source)`, `streamMessages(source)`, `sendMessage(source, …)`; DTO types for log lines, participants, delivery |
| `web/src/pages/{RunDetail,SessionDetail,AgentiflowDetail}.tsx` | the tab |
| `web/src/components/transcript/{Turn,transcriptView}.tsx/ts` | bubbles, injected chips, tool-card folding, `#msg=` anchors |
| `web/src/components/transcript/MessageBubble.tsx` | **new** |
| tests next to each component (vitest) | below |

## 5. Tests (vitest)

1. `MessageFeed` renders every kind from a fixture log with the right badge/treatment; filters hide and show correctly; a legacy response shows the legacy note.
2. `RecipientPicker` groups participants, roles and steps; left participants are greyed; defaults match the source kind.
3. `Composer`: a 409 disables it with the reason; a send shows pending until the stream delivers the line.
4. Transcript: `Event::Message` sent/received render as bubbles; a `msg.send` tool pair folds into the sent bubble; `Event::Injected` renders a collapsed chip; `#msg=` scrolls and highlights.
5. Windowing: a 5,000-line log renders fewer than 100 row nodes.

## 6. Acceptance

- matt approves the mock (§3.5), then checks the built UI on a real workflow run with `messaging: true` and a real agentiflow on the Mac, before the PR merges.
- No message kind that exists in the log is invisible in the UI.
