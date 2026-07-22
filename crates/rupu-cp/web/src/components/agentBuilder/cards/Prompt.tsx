// Prompt card — the markdown body. Verbatim, no templating: what's typed
// here is exactly what follows the `---` frontmatter fence in the .md file.
import { LabeledRow } from '../controls';
import type { CardProps } from './Identity';

export default function Prompt({ draft, patch }: CardProps) {
  return (
    <LabeledRow
      label="System prompt"
      yamlKey="markdown body"
      hint="The markdown after the frontmatter IS the system prompt, verbatim (no templating)."
    >
      <textarea
        className="ab-txt"
        style={{ minHeight: '140px' }}
        value={draft.body}
        onChange={(e) => patch({ body: e.target.value })}
        aria-label="system prompt"
      />
    </LabeledRow>
  );
}
