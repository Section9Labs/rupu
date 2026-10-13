// Skills card — `skills` (what the agent may load on demand) and
// `preloadSkills` (always in its prompt). Suggestions are the installed
// skill names the shell fetched, plus the two wildcards. An unknown name
// still saves; the agent then lists it in its load error.
import { ChipsInput, LabeledRow } from '../controls';
import type { CardProps } from './types';

export interface SkillsCardProps extends CardProps {
  skillNames?: string[];
}

const WILDCARDS = ['*', 'project:*'];

export default function Skills({ draft, patch, skillNames }: SkillsCardProps) {
  const suggestions = [...(skillNames ?? []), ...WILDCARDS];
  return (
    <>
      <LabeledRow
        label="Skills the agent may load"
        yamlKey="skills"
        hint="* means every global skill; project skills need their exact name or project:*"
      >
        <div data-testid="ab-skills">
          <ChipsInput
            list={draft.skills ?? []}
            aria-label="Skills it may load"
            suggestions={suggestions}
            placeholder="skill…"
            onChange={(next) => patch({ skills: next })}
          />
        </div>
      </LabeledRow>
      <LabeledRow label="Always in its prompt (preload)" yamlKey="preloadSkills">
        <div data-testid="ab-preload-skills">
          <ChipsInput
            list={draft.preloadSkills ?? []}
            aria-label="Skills always in its prompt"
            suggestions={suggestions}
            placeholder="skill…"
            onChange={(next) => patch({ preloadSkills: next })}
          />
        </div>
      </LabeledRow>
    </>
  );
}
