// kindBridge — the run graph's single import boundary onto the editor's
// per-kind visual language.
import { describe, it, expect } from 'vitest';
import { runKindToStepKind, runKindAccent, runKindIcon, runKindLabel } from './kindBridge';
import { KIND_ACCENT, KIND_ICON } from '../workflow-editor/kindVisuals';

describe('runKindToStepKind', () => {
  it('maps gate onto the editor approval_gate kind', () => {
    expect(runKindToStepKind('gate')).toBe('approval_gate');
  });

  it('passes every other run kind through unchanged', () => {
    expect(runKindToStepKind('step')).toBe('step');
    expect(runKindToStepKind('for_each')).toBe('for_each');
    expect(runKindToStepKind('parallel')).toBe('parallel');
    expect(runKindToStepKind('panel')).toBe('panel');
    expect(runKindToStepKind('action')).toBe('action');
  });
});

describe('runKindAccent / runKindIcon', () => {
  it('resolves through the editor palette so both graphs share one source', () => {
    expect(runKindAccent('gate')).toBe(KIND_ACCENT.approval_gate);
    expect(runKindAccent('parallel')).toBe(KIND_ACCENT.parallel);
    expect(runKindAccent('for_each')).toBe(KIND_ACCENT.for_each);
    expect(runKindIcon('action')).toBe(KIND_ICON.action);
    expect(runKindIcon('step')).toBe(KIND_ICON.step);
  });
});

describe('runKindLabel', () => {
  it('gives each kind a short human label for the node pill', () => {
    expect(runKindLabel('step')).toBe('step');
    expect(runKindLabel('for_each')).toBe('for each');
    expect(runKindLabel('gate')).toBe('gate');
    expect(runKindLabel('action')).toBe('action');
    expect(runKindLabel('parallel')).toBe('parallel');
    expect(runKindLabel('panel')).toBe('panel');
  });
});
