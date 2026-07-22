// Tools card — the `tools` allowlist. An absent/empty list is meaningful
// (grants the full default registry), so the hint changes based on that,
// not just cosmetic copy.
import { BUILTIN_TOOLS } from '../../../lib/agentBuilder/agentSpec';
import { ChipsInput, LabeledRow } from '../controls';
import type { CardProps } from './types';

export default function Tools({ draft, patch }: CardProps) {
  const list = draft.tools ?? [];
  return (
    <LabeledRow
      label="Tool allowlist"
      yamlKey="tools"
      hint={
        list.length === 0
          ? 'Empty list grants the full default registry.'
          : 'Exact tool names (built-ins + dotted MCP like scm.prs.get). Unknown names are silently dropped at load.'
      }
    >
      <ChipsInput
        list={list}
        suggestions={[...BUILTIN_TOOLS]}
        placeholder="add tool…"
        onChange={(next) => patch({ tools: next })}
      />
    </LabeledRow>
  );
}
