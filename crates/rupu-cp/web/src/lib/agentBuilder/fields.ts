// Static card registry driving the Agent Builder palette. Each card names a
// field group of `AgentDraft`; `id` is the stable key later tasks (palette
// drag/drop, card components, form wiring) reference — do not rename an `id`
// without updating every consumer.
export interface CardMeta {
  id: string;
  label: string;
  yamlKeys: string;
  group: 'Core' | 'Runtime' | 'Advanced';
  required?: boolean;
}

export const CARD_REGISTRY: CardMeta[] = [
  // Core
  { id: 'identity', label: 'Identity', yamlKeys: 'name · description', group: 'Core', required: true },
  { id: 'prompt', label: 'Prompt', yamlKeys: 'markdown body', group: 'Core', required: true },
  // Runtime
  { id: 'model', label: 'Model', yamlKeys: 'provider · model · auth', group: 'Runtime' },
  { id: 'tools', label: 'Tools', yamlKeys: 'tools', group: 'Runtime' },
  { id: 'permission', label: 'Permission', yamlKeys: 'permissionMode', group: 'Runtime' },
  { id: 'reasoning', label: 'Reasoning', yamlKeys: 'effort · maxTurns · maxTokens', group: 'Runtime' },
  // Advanced
  {
    id: 'context',
    label: 'Context',
    yamlKeys: 'contextWindow · contextWindowTokens · compactAtPercent',
    group: 'Advanced',
  },
  { id: 'output', label: 'Output', yamlKeys: 'outputFormat · outputSchema', group: 'Advanced' },
  { id: 'dispatch', label: 'Dispatch', yamlKeys: 'dispatchableAgents', group: 'Advanced' },
  {
    id: 'anthropic',
    label: 'Anthropic',
    yamlKeys: 'anthropicSpeed · anthropicContextManagement · anthropicTaskBudget · anthropicOauthPrefix',
    group: 'Advanced',
  },
  { id: 'concerns', label: 'Concerns', yamlKeys: 'concerns', group: 'Advanced' },
];
