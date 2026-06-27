import { describe, it, expect } from 'vitest';
import { completionsFor, expressionReference, type ExprContext } from './workflowExpressions';

function ctx(over: Partial<ExprContext>): ExprContext {
  return {
    nodeKind: 'step',
    isForEachPrompt: false,
    isPanelField: false,
    inputNames: [],
    priorSteps: [],
    ...over,
  };
}

const inserts = (c: ExprContext): string[] => completionsFor(c).map((e) => e.insert);

describe('completionsFor', () => {
  it('offers item / loop.* inside a for_each prompt', () => {
    const xs = inserts(ctx({ nodeKind: 'for_each', isForEachPrompt: true }));
    expect(xs).toContain('item');
    expect(xs).toContain('loop.index');
    expect(xs).toContain('loop.index0');
    expect(xs).toContain('loop.last');
  });

  it('excludes item / loop.* outside a for_each prompt', () => {
    const xs = inserts(ctx({ nodeKind: 'step', isForEachPrompt: false }));
    expect(xs).not.toContain('item');
    expect(xs.some((x) => x.startsWith('loop.'))).toBe(false);
  });

  it('offers ONLY prior step ids (and nothing for an empty prior list)', () => {
    const xs = inserts(
      ctx({ priorSteps: [{ id: 'build', kind: 'step' }] }),
    );
    expect(xs).toContain('steps.build.output');
    expect(xs.some((x) => x.startsWith('steps.deploy'))).toBe(false);
    // No prior steps → no steps.* entries at all.
    const none = inserts(ctx({}));
    expect(none.some((x) => x.startsWith('steps.'))).toBe(false);
  });

  it('a panel prior step contributes .findings / .max_severity / .iterations / .resolved', () => {
    const xs = inserts(ctx({ priorSteps: [{ id: 'review', kind: 'panel' }] }));
    expect(xs).toContain('steps.review.findings');
    expect(xs).toContain('steps.review.max_severity');
    expect(xs).toContain('steps.review.iterations');
    expect(xs).toContain('steps.review.resolved');
    // panel does NOT expose .results
    expect(xs).not.toContain('steps.review.results');
  });

  it('a for_each / parallel prior step contributes .results; parallel also .sub_results', () => {
    const fe = inserts(ctx({ priorSteps: [{ id: 'fan', kind: 'for_each' }] }));
    expect(fe).toContain('steps.fan.results');
    expect(fe).not.toContain('steps.fan.sub_results');

    const par = inserts(ctx({ priorSteps: [{ id: 'par', kind: 'parallel' }] }));
    expect(par).toContain('steps.par.results');
    expect(par).toContain('steps.par.sub_results');
  });

  it('a plain step prior contributes only output/success/skipped', () => {
    const xs = inserts(ctx({ priorSteps: [{ id: 'a', kind: 'step' }] })).filter((x) => x.startsWith('steps.a.'));
    expect(xs.sort()).toEqual(['steps.a.output', 'steps.a.skipped', 'steps.a.success']);
  });

  it('offers inputs.subject only inside a panel field', () => {
    expect(inserts(ctx({ isPanelField: true }))).toContain('inputs.subject');
    expect(inserts(ctx({ isPanelField: false }))).not.toContain('inputs.subject');
  });

  it('surfaces declared input names as inputs.<name>', () => {
    const xs = inserts(ctx({ inputNames: ['repo', 'branch'] }));
    expect(xs).toContain('inputs.repo');
    expect(xs).toContain('inputs.branch');
  });

  it('always offers event / issue / read_file / filters', () => {
    const xs = inserts(ctx({}));
    expect(xs).toContain('event.action');
    expect(xs).toContain('issue.number');
    expect(xs).toContain("read_file('path')");
    expect(xs).toContain('length'); // filter insert (label is `| length`)
    expect(completionsFor(ctx({})).some((e) => e.kind === 'filter' && e.label === '| length')).toBe(true);
  });

  it('every entry carries a non-empty detail', () => {
    for (const e of completionsFor(ctx({ inputNames: ['x'], priorSteps: [{ id: 'a', kind: 'step' }] }))) {
      expect(e.detail.length).toBeGreaterThan(0);
    }
  });
});

describe('expressionReference', () => {
  it('exposes the full grouped vocabulary', () => {
    const groups = expressionReference().map((g) => g.group);
    expect(groups).toEqual(['Inputs', 'Steps', 'Loop (for_each)', 'Event', 'Issue', 'Functions', 'Filters']);
    for (const g of expressionReference()) {
      expect(g.entries.length).toBeGreaterThan(0);
    }
  });
});
