# Agentiflow flow: the unfolding map

Status: design, for matt's review (2026-10-21).
Approved direction: mock A, "Unfolding map" (`assets/agentiflow-flow/a-unfolding-map/index.html`, screenshots `agentiflow.png` and `workflow-run.png` beside it). The mock runs on an invented fixture (`assets/agentiflow-flow/fixture.js`) shaped like the CP API; open it in a browser from that directory.

## Intent

The agentiflow page's Flow tab is a vertical list of unit rows off one spine. It reads as an event log, not a diagram, and it shows nothing of what makes an agentiflow interesting. matt wants it to show "how the flow of agent work happened": the graph the agents generate with their work, the back and forth between them, a custom workflow the lead generated and how it ran (panels, fan-outs, gates), the agents it created, and agents talking to each other. It must use the same visual language as the workflow run graph and editor ("Calm depth", PRs #928/#929) so the experience is unified, and it must grow down and to the right as the run proceeds.

Normal workflow runs get the same message and board layer, since today a run's messages and board are only reachable in a separate tab.

## Goals

1. **The agentiflow Flow tab becomes the unfolding map.**
   - The lead is a spine down the left with one node per round. Each round node carries what the lead decided that round, the round's goals and spend, and a link to that round's lead transcript.
   - Each round branches right into the children it launched, drawn as the run graph's own node cards: codename, agent, kind badge (AGENT, SUB-AGENT, WORKFLOW), status glyph, tokens and duration, a one-line result, and a transcript link. A child launched by a child (a sub-agent, at any depth) hangs one column further right off its parent card.
   - A workflow child is drawn as the workflow it is: a framed region holding its real run graph, built from the same components as the run page (step, for_each with unit squares, parallel, panel with panelists, verdicts and gate iterations, approval gate with its decision, action). A workflow the lead generated is labelled "written by the lead in round N" and links to the definition it wrote. A named pool workflow gets the same frame without that label.
   - The diagram grows down (rounds) and to the right (work and depth).
2. **Messages are drawn on the map.**
   - Each message is an arc from the sender's card to each receiver's card, coloured by kind: direct, board post, directive, operator. Each arc has a small packet that travels it when the message arrives.
   - An operator message enters from outside the canvas into its recipient's node.
   - A directive to a role or broadcast fans out to every resolved recipient. An undelivered message ends in a dead end marked with its reason.
   - Hovering or selecting an arc, a card or a message row lights both ends and shows the body.
   - Filters by message kind and by participant sit in the toolbar, as in the mock.
3. **The board is a rail on the right edge of the canvas.**
   - Claims show as held or released, with their owner and times.
   - Board posts show where they landed.
   - Standing directives show with their tail. A retracted directive ends where it was retracted.
   - Each board item connects by an arc to the card that wrote it.
4. **Header, time axis and panels, as in the mock.**
   - The header shows the run's name, crew, status, goals met, budget spend and tokens.
   - A time axis with round bands and message ticks runs across the top.
   - A Messages list and a Goals panel sit under the canvas. A row in the Messages list finds its arc on the map.
5. **It feels alive.** (The same engine drives the workflow run page, see goals 6 and 7.)
   - A running agentiflow grows as rounds and children arrive: new cards arrive staggered, their edges draw in, packets travel.
   - The running tail breathes, and a "live" pill shows the current time.
   - A Replay control replays the whole run's growth from the start, with a scrubber on the time axis.
   - All motion uses the chart and graph motion vocabulary (`styles.css` "Chart motion", `styles/graph-motion.css`) and degrades to fades under reduced motion.
6. **Workflow runs gain the message and board layer.**
   - On a workflow run's page, the existing left-to-right run graph gets the same message arcs between the steps' cards (and between units), the operator entry, the board rail, and the Messages and Board panels beneath, as in `workflow-run.png`.
   - The layer shows only when the run has a message space (`space: present`).
   - The Messages tab stays; the graph layer is an addition, not a replacement.
   - The run page also gets the time axis (step bands and message ticks), Replay with its scrubber, the live pill, and the growing draw-in: steps arrive and their edges draw in in the order they ran, messages and board items land at their times. It plays the run graph forward from the start.
7. **One timeline engine for both pages.** The time axis, Replay, the scrubber, the live "now" and the growing reveal are one shared piece, fed by a time-ordered list of what appeared when (a round, a child, a step, a unit, a message, a board item). The agentiflow map and the run graph both draw "the run as of time t" from it, so the two pages replay, scrub and go live the same way. On the run page this means the existing graph gains a time-driven reveal; with Replay idle, a finished run renders whole and a running one shows live as today.

## Non-goals

- Changing how agentiflows, the lead, units or messaging behave, beyond recording the extra facts listed under Backend.
- The workflow editor, the Children tab and the Messages tab, which stay as they are, though the map links into them.
- The macOS app, which is deprecated.
- Mock B (lanes) and mock C (graph of graphs). Ideas from C may be borrowed later, notably the minimap and drawer, but they are not part of this work.

## Anti-goals (a failure even if it technically works)

- An event feed again: rows on a spine, with the work not drawn as a graph.
- A second visual vocabulary. Every card, edge, badge and status glyph comes from the run graph's components, never a look-alike.
- Spaghetti at real density. Mock B showed how lines become unreadable when everything is drawn at once. Real runs reach 23 units in a round, a 200-round agentiflow exists, and a busy run has hundreds of messages. The map must stay legible at that scale (see Density).
- Invented data: a placeholder decision, fake round attribution, or a guessed "generated" flag. If the CP can't say it, the map doesn't show it.

## Constraints

- **Data.** Everything comes from the CP API. The web never reads transcripts to infer structure, except to open one when the operator asks.
- **Live load.**
  - The agentiflow page polls `GET /api/agentiflows/:id` every 5 s while running. The map must render from that without re-fetching per child on every poll.
  - A workflow child's run graph is fetched once when its frame first comes into view, and refreshed only while that child is running.
  - Messages use the existing `useMessageSpace` (SSE for a live local space, pages otherwise).
- **Remote hosts.** Agentiflows have no host transport today (every `/api/agentiflows/*` route is local-only and answers `?host=` with 501), and this work does not add one: the new agentiflow routes stay local-only too. A remote workflow run's page uses the new run endpoints through the host transports (below), and whatever a host can't serve is said in place, never drawn empty as if nothing happened.
- **Visual verification.** The UI is checked in light and dark, at desktop and tablet widths, on real runs (local and remote where available) as well as the fixture, before the PR.
- **Repo rules.** Tests sit beside the code, the Rust side follows the crate rules in `CLAUDE.md`, and no assessment-derived data goes into fixtures or docs.

## Backend additions (decided)

The CP serves the structure today: `GET /api/agentiflows/:id` (rounds as events, direct units), `GET /api/agentiflows/:id/children` (every child at every depth with `parent_run_id`, `depth`, `address`, tokens and times), `GET /api/runs/:id/graph` (a workflow child's full graph), and `GET /api/{agentiflows,runs}/:id/messages` (messages with `delivery`, receipts, and participants mapping address ↔ codename ↔ run id). It lacks:

1. **The round that launched a child.** The launcher stamps `round` on the children ledger's `started` line (`rupu-launch` `ledger::Started`), and `ChildDto` and `UnitDto` carry it. A child launched by another child inherits its root child's round. Older ledgers have no `round`, and the map then places those children by start time against the round events' timestamps, marked as inferred.
2. **What the lead decided each round.** The `round` event (`rupu-agentiflow` `run.rs`) gains:
   - `started_at`;
   - `decision`: the lead's final answer for the round, capped to a short excerpt;
   - per-goal `{id, met, current}` after the round (the assessment that follows it, not the digest handed into it);
   - the digest's `warnings`, if any.

   Older runs have none of these, and their round nodes show the round number, counts and the transcript link only.
3. **A child's result line.** `ChildDto` carries a short excerpt of the ledger's `finished.output`, so a card shows its result without a fetch per child.
4. **Generated workflows.** The children ledger's request records `generated: true` and the generated file's path when a workflow child came from `workflows.generate`. `ChildDto` carries `generated`, and `GET /api/agentiflows/:id/generated/:child_handle` serves the definition it wrote, read only, with the path validated to stay under the run's `generated/` directory. Older runs can't tell, and their workflow children get the plain frame.
5. **The board.** `GET /api/agentiflows/:id/board` and `GET /api/runs/:id/board` return the space's live claims (key, owner, acquired, lease expiry, expired) read through `rupu_fleet::Board`, and the standing directives with their retractions. Claim history needs a record, because release deletes the claim's file today: `Board` appends `claimed` / `released` / `expired` lines to a journal (`board/claims.jsonl`) as it grants, releases and reaps, and the endpoint folds it. Runs before the journal show their live claims only.

Remote hosts, for the run endpoints only (agentiflows stay local, see Constraints):
- An HTTP peer advertises `GET /api/runs/:id/board` as a feature and is proxied as the other run endpoints are.
- An SSH remote gets it through a `__features`-gated hidden command, as messages do.
- Tunnel and bucket hosts answer 501, and the page says so in place.

## A workflow child's own messages and board (decided)

A workflow child is its own message root (F1a FD6, `rupu-launch` `launcher.rs`): its step agents talk in the child run's own space, not the flow's. So the flow's `/messages` and `/board` never contain them, and the map reads them from the child: when a workflow child's frame first comes into view it fetches `GET /api/runs/:child/messages` and `GET /api/runs/:child/board`, follows the messages stream while the child is running and local, and draws those arcs and that board inside the child's frame. The Messages list under the canvas merges every space it has loaded, each row tagged with the space it came from (the flow, or the workflow child by codename), so it reaches every message the map draws. The rail on the right is the flow's board; each workflow frame carries its own small board strip. Workflow children never join the flow's space, since that would change messaging behaviour.

## Density (decided)

These rules keep the map legible at real scale. The thresholds are the builder's to tune on real runs.

**Rounds.**
- All rounds are drawn.
- A round whose children are all finished and that isn't selected collapses to its node plus one summary card ("23 children · 21 done, 2 failed").
- The latest rounds and the running round stay expanded.

**Children.**
- A round with more children than fit is drawn like the run graph's for_each: the first few as cards, then a "+N more" card that expands in place.
- Failed and running children are always shown as cards.

**Messages.**
- Past a threshold, arcs are not all drawn at rest. The map shows each card's message count as a badge and draws a card's arcs on hover or selection, and every arc of the selected round.
- The Messages list and the filters always reach every message.
- Arcs between the units of one workflow are drawn inside its frame.

**Canvas.**
- Pan and zoom with the run graph's controls.
- "Fit" frames the whole map, and a running run follows its newest round unless the operator has panned away.

## Left to the builder

- Component structure, and how much of `RunGraph` and the graph node components are reused directly versus wrapped, so long as the cards are the same components.
- Layout algorithm, card dimensions and spacing; the arc routing; the exact collapse and arc thresholds.
- Motion timing within the motion vocabulary.
- How the round decision excerpt is cut and its length cap.
- Test strategy beyond: pure layout and data folding have unit tests, the new endpoints have Rust tests, including old runs without the new fields, and the UI is checked in the browser as stated under Constraints.
- Whether the work ships as one PR or a backend PR then a web PR.
