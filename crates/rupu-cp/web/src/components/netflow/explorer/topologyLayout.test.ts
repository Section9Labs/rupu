import { describe, expect, it } from 'vitest';
import type { SankeyView } from '../../../lib/netflow';
import {
  buildPairSets,
  isConnected,
  layoutTopology,
  linkTouchesHover,
  COLUMN_WIDTH,
  COLUMN_X,
} from './topologyLayout';

const node = (id: string, calls = 1, errors = 0) => ({ id, label: id, calls, errors });
const link = (from: string, to: string, calls: number, errors = 0) => ({
  from,
  to,
  calls,
  errors,
});

function sankey(): SankeyView {
  return {
    workflows: [node('review'), node('triage')],
    origins: [node('provider:anthropic'), node('scm:github')],
    orgs: [node('as13335'), node('as36459')],
    wf_origin: [
      link('review', 'provider:anthropic', 100),
      link('triage', 'scm:github', 10, 2),
    ],
    origin_org: [
      link('provider:anthropic', 'as13335', 100),
      link('scm:github', 'as36459', 10),
    ],
  };
}

describe('layoutTopology', () => {
  it('draws each stage between its own pair of column edges', () => {
    const laid = layoutTopology(sankey());
    const ab = laid.links.find((l) => l.key.startsWith('wf:review'))!;
    const bc = laid.links.find((l) => l.key.startsWith('or:provider:anthropic'))!;
    expect(ab.d.startsWith(`M ${COLUMN_X[0] + COLUMN_WIDTH} `)).toBe(true);
    expect(bc.d.startsWith(`M ${COLUMN_X[1] + COLUMN_WIDTH} `)).toBe(true);
  });

  it('weights ribbons by calls against the global max — never bytes', () => {
    const laid = layoutTopology(sankey());
    const heavy = laid.links.find((l) => l.from === 'review')!;
    const light = laid.links.find((l) => l.from === 'triage')!;
    expect(heavy.width).toBe(26); // 2 + 24 · 100/100
    expect(light.width).toBeCloseTo(2 + 24 * (10 / 100));
  });

  it('marks a link failed only above the 5% error-rate threshold', () => {
    const laid = layoutTopology(sankey());
    expect(laid.links.find((l) => l.from === 'triage')!.failed).toBe(true); // 2/10
    expect(laid.links.find((l) => l.from === 'review')!.failed).toBe(false);
  });

  it('skips a link whose endpoint is missing from the node universe instead of crashing', () => {
    const s = sankey();
    s.wf_origin.push(link('ghost', 'provider:anthropic', 5));
    const laid = layoutTopology(s);
    expect(laid.links.some((l) => l.from === 'ghost')).toBe(false);
  });

  it('sizes the canvas to the deepest column', () => {
    const s = sankey();
    s.orgs.push(node('as0'), node('unknown'));
    const four = layoutTopology(s).height;
    const two = layoutTopology(sankey()).height;
    expect(four).toBeGreaterThan(two);
  });
});

describe('hover connectivity', () => {
  const pairs = buildPairSets(sankey());

  it('a node is connected to itself and its adjacent column entries', () => {
    expect(isConnected({ dim: 'wf', key: 'review' }, 'wf', 'review', pairs)).toBe(true);
    expect(isConnected({ dim: 'wf', key: 'review' }, 'wf', 'triage', pairs)).toBe(false);
    expect(isConnected({ dim: 'wf', key: 'review' }, 'or', 'provider:anthropic', pairs)).toBe(
      true,
    );
    expect(isConnected({ dim: 'wf', key: 'review' }, 'or', 'scm:github', pairs)).toBe(false);
  });

  it('crosses two hops between workflows and orgs through shared origins', () => {
    expect(isConnected({ dim: 'wf', key: 'review' }, 'org', 'as13335', pairs)).toBe(true);
    expect(isConnected({ dim: 'wf', key: 'review' }, 'org', 'as36459', pairs)).toBe(false);
    expect(isConnected({ dim: 'org', key: 'as36459' }, 'wf', 'triage', pairs)).toBe(true);
    expect(isConnected({ dim: 'org', key: 'as36459' }, 'wf', 'review', pairs)).toBe(false);
  });

  it('linkTouchesHover flags only ribbons touching the hovered key', () => {
    const laid = layoutTopology(sankey());
    const ab = laid.links.find((l) => l.from === 'review')!;
    expect(linkTouchesHover(ab, { dim: 'wf', key: 'review' })).toBe(true);
    expect(linkTouchesHover(ab, { dim: 'wf', key: 'triage' })).toBe(false);
    expect(linkTouchesHover(ab, null)).toBe(false);
  });
});
