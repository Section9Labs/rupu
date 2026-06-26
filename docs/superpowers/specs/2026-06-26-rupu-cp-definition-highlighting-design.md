# rupu CP — syntax-highlighted definition files (Build section)

Date: 2026-06-26
Status: approved (design)

## Problem

In the rupu Control Panel (CP) "Build" section, the definition files for
workflows, agents, and autoflows are shown as plain monospace text (or, for
autoflows, not shown at all). They should be displayed with **syntax
highlighting** so the YAML/markdown structure is legible.

Current state:

- **Workflows** (`WorkflowDetail.tsx`) — already renders raw YAML, but in a
  plain `<pre>` with no highlighting. Backend serves a `yaml` string from
  `GET /api/workflows/:name`.
- **Agents** (`AgentDetail.tsx`) — renders only the parsed `system_prompt`
  (the `.md` body) in a plain `<pre>`. The raw definition file is not served.
  Agent files are `.md` with a YAML frontmatter header.
- **Autoflows** (`AutoflowsDefs.tsx`) — a list only. No detail view, no YAML.

## Definitions (workflow vs autoflow)

- A **workflow** is any `.yaml` file under `workflows/` — a named set of steps
  plus a trigger (`trigger.on: manual | cron | event`).
- An **autoflow** is *not* a separate file type. It is a workflow whose
  `autoflow.enabled == true`. `GET /api/autoflows` returns exactly the subset
  of workflows where that flag is on (mirrors the CLI's `autoflow list`
  predicate). Autoflows live in the same `workflows/` directory.

Conclusion: every autoflow is a workflow; an autoflow is the subset wired to
run automatically on its cron/event trigger rather than launched manually.
Autoflows therefore reuse the workflow detail page; that page must make the
autoflow status legible.

## Behavior

A **content-aware** highlighter:

- Detect a leading YAML frontmatter block delimited by `---` … `---`.
- Highlight the frontmatter block as YAML.
- Highlight whatever follows the frontmatter as markdown.
- Files with no frontmatter (workflows, autoflows) highlight as YAML
  throughout.

## Frontend (`crates/rupu-cp/web`)

### New `src/components/CodeHighlight.tsx`

- Uses `highlight.js/lib/core` with only `yaml` and `markdown` languages
  registered (keeps the bundle lean), reusing the existing
  `highlight.js/styles/github.css` theme already imported by the transcript
  markdown renderer.
- `splitFrontmatter(raw): { frontmatter: string | null; body: string }` —
  detects a leading `---\n…\n---` block. Handles: frontmatter present,
  absent, frontmatter-only (no body), and CRLF line endings.
- `<CodeHighlight code={…} language="yaml" />` — highlights the whole string
  as one language. Used for workflows/autoflows.
- A frontmatter-aware mode (e.g. `<CodeHighlight code={…} frontmatter />` or a
  dedicated `<DefinitionFile code={…} />`) — renders the `---`-fenced
  frontmatter highlighted as YAML and the body highlighted as markdown inside
  a single styled `<pre>` matching the current panel styling
  (`bg-panel border border-border rounded-xl shadow-card p-4` etc.). Used for
  agents.
- Rendering uses `hljs.highlight(code, { language }).value` injected via
  `dangerouslySetInnerHTML` on a `<code>` element. Input is trusted local
  definition files; highlight.js escapes its output.

### `src/pages/WorkflowDetail.tsx`

- Replace `<pre>{detail.yaml}</pre>` with
  `<CodeHighlight code={detail.yaml} language="yaml" />`.
- When `workflow.autoflow.enabled` is true, show an **"Autoflow" chip** in the
  header next to the scope chip, plus a small trigger line
  (e.g. `cron: 0 */6 * * *` or `event: <kind>`). The detail endpoint already
  returns the full parsed `workflow` object including `autoflow` and
  `trigger`, so this is frontend-only — narrow the fields defensively the same
  way existing code reads `workflow.*`.

### `src/pages/AgentDetail.tsx`

- Render the raw `.md` definition (frontmatter-aware) under a **"Definition"**
  heading, replacing the plain `system_prompt` `<pre>`. Keeps the existing
  parsed meta chips (provider/model/effort/max_tokens) in the header.
- Requires `raw: string` added to the `AgentDetail` type in `lib/api.ts`.

### `src/pages/AutoflowsDefs.tsx`

- Make each row a `<Link to={`/workflows/${slug}`}>` so its (now highlighted)
  YAML shows on the existing workflow detail page. No new page/endpoint.
- Requires `slug: string` added to `AutoflowDefRow` in `lib/api.ts`.

### `vite.config.ts`

- Verify the Build detail routes are lazy-loaded; add/extend a manualChunk so
  `highlight.js` stays out of the main entry bundle (it is already grouped in
  the `markdown` chunk — ensure the new direct import lands there or in a
  dedicated `highlight` chunk).

## Backend

### `rupu-agent` — `AgentSpec.raw`

- Add `raw: String` to `AgentSpec`, populated in `AgentSpec::parse` (so it
  flows through `parse_file` and the loader). Holds the full original file
  text. Rationale: agents are matched by parsed `name`, not file stem, so the
  source path is not cleanly recoverable in the handler — storing the raw text
  on the spec is the clean home for it.

### `rupu-cp` — agents endpoint

- Add `raw: String` to `AgentDetailDto`, sourced from `spec.raw`. Served by
  `GET /api/agents/:name`.

### `rupu-cp` — autoflows endpoint

- Add `slug: String` (the file stem) to `AutoflowDefRow` so the frontend can
  link to the correct workflow detail route (the workflow detail endpoint is
  keyed by file stem, not parsed name).

## Testing

- **Backend**
  - Agent detail DTO includes `raw` containing the full file text.
  - Autoflow row carries the correct `slug` (file stem) distinct from the
    parsed `name` when they differ.
- **Frontend (vitest)**
  - `splitFrontmatter`: frontmatter present, absent, frontmatter-only, CRLF.

## Out of scope

- Editing definitions in the CP (read-only display only).
- Dark-mode theme switching (CP is light-only).
- A separate autoflow detail page/endpoint.
