// Static card registry driving the Agent Builder palette. Each card names a
// field group of `AgentDraft`; `id` is the stable key later tasks (palette
// drag/drop, card components, form wiring) reference — do not rename an `id`
// without updating every consumer.
import {
  Brain,
  Cpu,
  FileJson,
  FileText,
  IdCard,
  Layers,
  ListChecks,
  Send,
  Shield,
  Sparkles,
  Wrench,
  type LucideIcon,
} from 'lucide-react';

export interface CardMeta {
  id: string;
  label: string;
  yamlKeys: string;
  group: 'Core' | 'Runtime' | 'Advanced';
  required?: boolean;
  icon: LucideIcon;
}

export const CARD_REGISTRY: CardMeta[] = [
  // Core
  { id: 'identity', label: 'Identity', yamlKeys: 'name · description', group: 'Core', required: true, icon: IdCard },
  { id: 'prompt', label: 'Prompt', yamlKeys: 'markdown body', group: 'Core', required: true, icon: FileText },
  // Runtime
  { id: 'model', label: 'Model', yamlKeys: 'provider · model · auth', group: 'Runtime', icon: Cpu },
  { id: 'tools', label: 'Tools', yamlKeys: 'tools', group: 'Runtime', icon: Wrench },
  { id: 'permission', label: 'Permission', yamlKeys: 'permissionMode', group: 'Runtime', icon: Shield },
  { id: 'reasoning', label: 'Reasoning', yamlKeys: 'effort · maxTurns · maxTokens', group: 'Runtime', icon: Brain },
  // Advanced
  {
    id: 'context',
    label: 'Context',
    yamlKeys: 'contextWindow · contextWindowTokens · compactAtPercent',
    group: 'Advanced',
    icon: Layers,
  },
  { id: 'output', label: 'Output', yamlKeys: 'outputFormat · outputSchema', group: 'Advanced', icon: FileJson },
  { id: 'dispatch', label: 'Dispatch', yamlKeys: 'dispatchableAgents', group: 'Advanced', icon: Send },
  {
    id: 'anthropic',
    label: 'Anthropic',
    yamlKeys: 'anthropicSpeed · anthropicContextManagement · anthropicTaskBudget · anthropicOauthPrefix',
    group: 'Advanced',
    icon: Sparkles,
  },
  { id: 'concerns', label: 'Concerns', yamlKeys: 'concerns', group: 'Advanced', icon: ListChecks },
];
