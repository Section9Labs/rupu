import { describe, it, expect } from 'vitest';
import { resolveWorkflowEditorUi } from './useWorkflowEditorUi';
describe('resolveWorkflowEditorUi', () => {
  it('localStorage override wins', () => {
    expect(resolveWorkflowEditorUi({ workflow_editor_ui: 'classic' }, 'next')).toBe('next');
    expect(resolveWorkflowEditorUi({ workflow_editor_ui: 'next' }, 'classic')).toBe('classic');
  });
  it('falls back to server config when no override', () => {
    expect(resolveWorkflowEditorUi({ workflow_editor_ui: 'next' }, null)).toBe('next');
  });
  it('defaults to classic when unset or unknown', () => {
    expect(resolveWorkflowEditorUi(null, null)).toBe('classic');
    expect(resolveWorkflowEditorUi({ workflow_editor_ui: 'bogus' }, null)).toBe('classic');
  });
});
