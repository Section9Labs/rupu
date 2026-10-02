// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import fixture from '../../../../../../rupu-coverage/tests/fixtures/finding_report/valid_full.json';
import { codeHref, type ArtifactRef, type FindingReport } from '../../../lib/findingReport';
import { findingArtifactUrl, type FindingOut } from '../../../lib/api';
import CallChain from './CallChain';
import EvidenceClaims from './EvidenceClaims';
import FixSections from './FixSections';
import CommandBlock from './CommandBlock';
import ArtifactBrowser from './ArtifactBrowser';
import ReportHeader from './ReportHeader';
import CrossReferences from './CrossReferences';

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const report = fixture as unknown as FindingReport;
const WS = 'ws_1';

function withRouter(ui: React.ReactElement) {
  return render(<MemoryRouter>{ui}</MemoryRouter>);
}

describe('CallChain', () => {
  it('renders hops in source to sink order with the sink marked', () => {
    const { container } = withRouter(<CallChain chain={report.call_chain} wsId={WS} />);
    const items = Array.from(container.querySelectorAll('li'));
    expect(items).toHaveLength(3);
    expect(items[0]).toHaveTextContent('router: GET /api/notes/{id}');
    expect(items[1]).toHaveTextContent('get_note()');
    expect(items[2]).toHaveTextContent('NoteStore::find_by_id()');
    expect(items[0].querySelector('[aria-label="source"]')).not.toBeNull();
    expect(items[2].querySelector('[aria-label="sink"]')).not.toBeNull();
    expect(container.querySelectorAll('[aria-label="sink"]')).toHaveLength(1);
    expect(screen.getAllByRole('img', { name: 'sink' })).toHaveLength(1);
  });

  it('links each hop with a file to the code viewer at its first line', () => {
    withRouter(<CallChain chain={report.call_chain} wsId={WS} />);
    const link = screen.getByRole('link', { name: 'src/routes/notes.rs:40-58' });
    expect(link).toHaveAttribute('href', codeHref(WS, 'src/routes/notes.rs', 40));
    const sink = screen.getByRole('link', { name: 'src/store/notes.rs:88-97' });
    expect(sink).toHaveAttribute('href', codeHref(WS, 'src/store/notes.rs', 88));
  });

  it('renders a sentinel chain as a label with no list', () => {
    const { container } = withRouter(<CallChain chain="Not Provided — n/a" wsId={WS} />);
    expect(screen.getByText('Not provided: n/a')).toBeInTheDocument();
    expect(container.querySelector('ol')).toBeNull();
    expect(container.querySelector('li')).toBeNull();
  });
});

describe('EvidenceClaims', () => {
  it('flags a claim whose code changed since it was recorded', () => {
    withRouter(<EvidenceClaims claims={report.evidence} states={['changed']} wsId={WS} />);
    expect(screen.getByText('Code changed since recorded')).toBeInTheDocument();
  });

  it('shows no badge for a current claim, and renders the excerpt in a <pre>', () => {
    const { container } = withRouter(<EvidenceClaims claims={report.evidence} states={['current']} wsId={WS} />);
    expect(screen.queryByText('Code changed since recorded')).toBeNull();
    expect(screen.queryByText('File no longer present')).toBeNull();
    const pre = container.querySelector('pre');
    expect(pre).not.toBeNull();
    expect(pre).toHaveTextContent('store.find_by_id(id)');
  });
});

describe('EvidenceClaims language handling', () => {
  const claim = (lang: string) => [{ claim: 'c', excerpt: 'let x = 1;', lang }];

  it('falls back to a plain <pre> with the excerpt for an unknown language', () => {
    const { container } = withRouter(<EvidenceClaims claims={claim('cobol')} states={['current']} wsId={WS} />);
    const pre = container.querySelector('pre');
    expect(pre).toHaveTextContent('let x = 1;');
    expect(container.querySelector('.hljs')).toBeNull();
  });

  it('highlights aliased and mixed-case language tags', () => {
    for (const lang of ['rs', 'Rust', 'RS']) {
      const { container, unmount } = withRouter(<EvidenceClaims claims={claim(lang)} states={['current']} wsId={WS} />);
      expect(container.querySelector('.hljs')).not.toBeNull();
      unmount();
    }
  });
});

describe('FixSections', () => {
  it('never renders an agent-written Markdown image as a fetched <img>', () => {
    const img = (n: string) => `![${n}](https://example.invalid/${n}.png)`;
    const patch = report.recommended_patch as Exclude<FindingReport['recommended_patch'], string>;
    const ci = report.ci_cd_detection as Exclude<FindingReport['ci_cd_detection'], string>;
    const reg = report.regression_test as Exclude<FindingReport['regression_test'], string>;
    const { container } = render(
      <FixSections
        report={{
          ...report,
          recommended_patch: { ...patch, notes: img('notes') },
          ci_cd_detection: { ...ci, body: img('ci') },
          regression_test: { ...reg, body: `${img('reg')} <img src="https://example.invalid/raw.png">` },
        }}
      />,
    );
    expect(container.querySelector('img')).toBeNull();
    for (const n of ['notes', 'ci', 'reg']) {
      expect(screen.getByRole('link', { name: `[image: ${n}]` })).toHaveAttribute('href', `https://example.invalid/${n}.png`);
    }
  });

  it('renders the recommended patch as a diff', () => {
    render(<FixSections report={report} />);
    expect(screen.getByText(/find_by_id_for_owner/)).toBeInTheDocument();
  });

  it('renders a regression_test sentinel as its label', () => {
    render(<FixSections report={{ ...report, regression_test: 'Not Provided — no harness' }} />);
    expect(screen.getByText('Not provided: no harness')).toBeInTheDocument();
    expect(screen.queryByText('Vulnerable build')).toBeNull();
  });

  it('shows both regression expectations', () => {
    render(<FixSections report={report} />);
    expect(screen.getByText('Vulnerable build')).toBeInTheDocument();
    expect(screen.getByText('Patched build')).toBeInTheDocument();
    expect(screen.getByText(/fails: got 200/)).toBeInTheDocument();
    expect(screen.getByText(/passes: got 404/)).toBeInTheDocument();
  });
});

describe('CommandBlock', () => {
  it('copies the command and confirms', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    render(<CommandBlock label="Command" command="cargo test --test notes_access" />);
    fireEvent.click(screen.getByRole('button', { name: 'Copy' }));
    expect(writeText).toHaveBeenCalledWith('cargo test --test notes_access');
    await waitFor(() => expect(screen.getByRole('button', { name: 'Copied' })).toBeInTheDocument());
  });
});

describe('CommandBlock feedback', () => {
  it('shows "Copy failed" when the clipboard rejects, then resets', async () => {
    vi.useFakeTimers();
    Object.assign(navigator, { clipboard: { writeText: vi.fn().mockRejectedValue(new Error('denied')) } });
    render(<CommandBlock label="Command" command="ls" />);
    fireEvent.click(screen.getByRole('button'));
    await act(async () => {});
    expect(screen.getByRole('button', { name: 'Copy failed' })).toBeInTheDocument();
    act(() => { vi.advanceTimersByTime(1600); });
    expect(screen.getByRole('button', { name: 'Copy' })).toBeInTheDocument();
  });

  it('announces status through a polite live region', async () => {
    Object.assign(navigator, { clipboard: { writeText: vi.fn().mockResolvedValue(undefined) } });
    render(<CommandBlock label="Command" command="ls" />);
    fireEvent.click(screen.getByRole('button'));
    const live = await screen.findByText('Copied');
    expect(live).toHaveAttribute('aria-live', 'polite');
  });

  it('a second copy restarts the reset timer instead of racing the first', async () => {
    vi.useFakeTimers();
    Object.assign(navigator, { clipboard: { writeText: vi.fn().mockResolvedValue(undefined) } });
    render(<CommandBlock label="Command" command="ls" />);
    fireEvent.click(screen.getByRole('button'));
    await act(async () => {});
    act(() => { vi.advanceTimersByTime(1000); });
    fireEvent.click(screen.getByRole('button'));
    await act(async () => {});
    // The first timer would have fired at 1500ms; it must have been cleared.
    act(() => { vi.advanceTimersByTime(1000); });
    expect(screen.getByRole('button', { name: 'Copied' })).toBeInTheDocument();
    act(() => { vi.advanceTimersByTime(600); });
    expect(screen.getByRole('button', { name: 'Copy' })).toBeInTheDocument();
  });
});

describe('ArtifactBrowser', () => {
  const sha = 'a'.repeat(64);
  const textArt: ArtifactRef = { path: 'poc/request.http', sha256: sha, size: 20, kind: 'text', stored: 'copied' };
  const binArt: ArtifactRef = { path: 'poc/capture.pcap', sha256: 'b'.repeat(64), size: 2048, kind: 'binary', stored: 'copied' };
  const extArt: ArtifactRef = { path: 'poc/huge.bin', sha256: 'c'.repeat(64), size: 9_999_999, kind: 'binary', stored: 'external', host: 'kuki' };

  it('fetches and previews a text artifact in a <pre>', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('GET /api/notes/42'));
    const { container } = render(<ArtifactBrowser findingId="fnd_1" artifacts={[textArt]} />);
    fireEvent.click(screen.getByRole('button', { name: /poc\/request\.http/ }));
    await waitFor(() => expect(container.querySelector('pre')).toHaveTextContent('GET /api/notes/42'));
    expect(fetchSpy).toHaveBeenCalledWith(findingArtifactUrl('fnd_1', sha), expect.anything());
  });

  it('offers a binary artifact as a download and never fetches it', () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    render(<ArtifactBrowser findingId="fnd_1" artifacts={[binArt]} />);
    fireEvent.click(screen.getByRole('button', { name: /poc\/capture\.pcap/ }));
    const link = screen.getByRole('link', { name: 'Download' });
    expect(link).toHaveAttribute('href', findingArtifactUrl('fnd_1', binArt.sha256));
    expect(link).toHaveAttribute('download');
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it('offers a host binary artifact as a download that names its host, and never fetches', () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    render(<ArtifactBrowser findingId="fnd_1" artifacts={[extArt]} />);
    fireEvent.click(screen.getByRole('button', { name: /poc\/huge\.bin/ }));
    const link = screen.getByRole('link', { name: 'Download' });
    expect(link).toHaveAttribute('href', findingArtifactUrl('fnd_1', extArt.sha256));
    expect(link).toHaveAttribute('download');
    expect(screen.getByText('from host kuki')).toBeInTheDocument();
    expect(screen.getByText('Binary file. Use Download.')).toBeInTheDocument();
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it('fetches a small host text artifact on click and previews it in a <pre>', async () => {
    const hostText: ArtifactRef = { path: 'poc/trace.txt', sha256: 'd'.repeat(64), size: 30, kind: 'text', stored: 'external', host: 'kuki' };
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('probe reply: 204 no content'));
    const { container } = render(<ArtifactBrowser findingId="fnd_1" artifacts={[hostText]} />);
    // Never on render: a remote pull can cost an SSH invocation at the host.
    expect(fetchSpy).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: /poc\/trace\.txt/ }));
    await waitFor(() => expect(container.querySelector('pre')).toHaveTextContent('probe reply: 204 no content'));
    expect(fetchSpy).toHaveBeenCalledTimes(1);
    expect(fetchSpy).toHaveBeenCalledWith(findingArtifactUrl('fnd_1', hostText.sha256), expect.anything());
    expect(screen.getByText('from host kuki')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Download' })).toHaveAttribute('href', findingArtifactUrl('fnd_1', hostText.sha256));
  });

  it('does not preview a host text artifact over the limit, and does not fetch', () => {
    const bigHostText: ArtifactRef = { path: 'poc/dump.log', sha256: 'e'.repeat(64), size: 300 * 1024, kind: 'text', stored: 'external', host: 'kuki' };
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    render(<ArtifactBrowser findingId="fnd_1" artifacts={[bigHostText]} />);
    fireEvent.click(screen.getByRole('button', { name: /poc\/dump\.log/ }));
    expect(screen.getByRole('alert')).toHaveTextContent(/Too large to preview/);
    expect(screen.getByRole('link', { name: 'Download' })).toBeInTheDocument();
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it('shows the unavailable reason when the host pull fails with a 404 {"unavailable"}', async () => {
    const hostText: ArtifactRef = { path: 'poc/trace.txt', sha256: 'd'.repeat(64), size: 30, kind: 'text', stored: 'external', host: 'kuki' };
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ unavailable: 'host kuki: node offline' }), { status: 404 }),
    );
    render(<ArtifactBrowser findingId="fnd_1" artifacts={[hostText]} />);
    fireEvent.click(screen.getByRole('button', { name: /poc\/trace\.txt/ }));
    expect(await screen.findByRole('alert')).toHaveTextContent('host kuki: node offline');
    expect(screen.queryByText(/"unavailable"/)).toBeNull();
  });

  describe('robustness', () => {
    const artA: ArtifactRef = { path: 'poc/a.txt', sha256: '1'.repeat(64), size: 10, kind: 'text', stored: 'copied' };
    const artB: ArtifactRef = { path: 'poc/b.txt', sha256: '2'.repeat(64), size: 10, kind: 'text', stored: 'copied' };

    it('drops a stale response that resolves after a newer selection', async () => {
      const resolvers: Record<string, (r: Response) => void> = {};
      vi.spyOn(globalThis, 'fetch').mockImplementation((input) =>
        new Promise<Response>((resolve) => { resolvers[String(input)] = resolve; }),
      );
      const { container } = render(<ArtifactBrowser findingId="fnd_1" artifacts={[artA, artB]} />);
      fireEvent.click(screen.getByRole('button', { name: /poc\/a\.txt/ }));
      fireEvent.click(screen.getByRole('button', { name: /poc\/b\.txt/ }));
      expect(screen.getByRole('status')).toHaveTextContent('Loading');
      // B lands first, then A's slow response arrives late.
      await act(async () => { resolvers[findingArtifactUrl('fnd_1', artB.sha256)](new Response('BODY-B')); });
      await waitFor(() => expect(container.querySelector('pre')).toHaveTextContent('BODY-B'));
      await act(async () => { resolvers[findingArtifactUrl('fnd_1', artA.sha256)](new Response('BODY-A')); });
      expect(container.querySelector('pre')).toHaveTextContent('BODY-B');
      expect(screen.queryByText('BODY-A')).toBeNull();
      expect(screen.queryByRole('status')).toBeNull();
    });

    it('drops a stale response even when the newer one is still pending', async () => {
      const resolvers: Record<string, (r: Response) => void> = {};
      vi.spyOn(globalThis, 'fetch').mockImplementation((input) =>
        new Promise<Response>((resolve) => { resolvers[String(input)] = resolve; }),
      );
      const { container } = render(<ArtifactBrowser findingId="fnd_1" artifacts={[artA, artB]} />);
      fireEvent.click(screen.getByRole('button', { name: /poc\/a\.txt/ }));
      fireEvent.click(screen.getByRole('button', { name: /poc\/b\.txt/ }));
      await act(async () => { resolvers[findingArtifactUrl('fnd_1', artA.sha256)](new Response('BODY-A')); });
      expect(container.querySelector('pre')).toBeNull();
      expect(screen.getByRole('status')).toHaveTextContent('Loading');
    });

    it('shows the message from a JSON error body, not the JSON', async () => {
      vi.spyOn(globalThis, 'fetch').mockResolvedValue(
        new Response(JSON.stringify({ error: 'artifact is no longer in the workspace' }), { status: 404 }),
      );
      render(<ArtifactBrowser findingId="fnd_1" artifacts={[artA]} />);
      fireEvent.click(screen.getByRole('button', { name: /poc\/a\.txt/ }));
      expect(await screen.findByText('artifact is no longer in the workspace')).toBeInTheDocument();
      expect(screen.queryByText(/"error"/)).toBeNull();
    });

    it('falls back to the raw text, then the status text, for non-JSON errors', async () => {
      const spy = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('upstream exploded', { status: 502 }));
      render(<ArtifactBrowser findingId="fnd_1" artifacts={[artA, artB]} />);
      fireEvent.click(screen.getByRole('button', { name: /poc\/a\.txt/ }));
      expect(await screen.findByText('upstream exploded')).toBeInTheDocument();
      spy.mockResolvedValue(new Response('', { status: 500, statusText: 'Internal Server Error' }));
      fireEvent.click(screen.getByRole('button', { name: /poc\/b\.txt/ }));
      expect(await screen.findByText('Internal Server Error')).toBeInTheDocument();
    });

    it('marks only the selected artifact as pressed, even when two paths share a sha', async () => {
      vi.spyOn(globalThis, 'fetch').mockImplementation(() => Promise.resolve(new Response('same bytes')));
      const dup: ArtifactRef = { ...artA, path: 'poc/a-copy.txt' };
      render(<ArtifactBrowser findingId="fnd_1" artifacts={[artA, dup, artB]} />);
      const btn = (re: RegExp) => screen.getByRole('button', { name: re });
      // nothing selected yet
      for (const re of [/poc\/a\.txt/, /poc\/a-copy\.txt/, /poc\/b\.txt/]) expect(btn(re)).toHaveAttribute('aria-pressed', 'false');
      fireEvent.click(btn(/poc\/a\.txt/));
      expect(btn(/poc\/a\.txt/)).toHaveAttribute('aria-pressed', 'true');
      expect(btn(/poc\/a-copy\.txt/)).toHaveAttribute('aria-pressed', 'false');
      expect(btn(/poc\/b\.txt/)).toHaveAttribute('aria-pressed', 'false');
      fireEvent.click(btn(/poc\/a-copy\.txt/));
      expect(btn(/poc\/a\.txt/)).toHaveAttribute('aria-pressed', 'false');
      expect(btn(/poc\/a-copy\.txt/)).toHaveAttribute('aria-pressed', 'true');
      await screen.findByText('same bytes');
    });

    it('announces a preview error with role="alert"', async () => {
      vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(JSON.stringify({ error: 'gone' }), { status: 404 }));
      render(<ArtifactBrowser findingId="fnd_1" artifacts={[artA]} />);
      fireEvent.click(screen.getByRole('button', { name: /poc\/a\.txt/ }));
      expect(await screen.findByRole('alert')).toHaveTextContent('gone');
    });

    it('refuses to preview an oversized text artifact and never fetches it', () => {
      const fetchSpy = vi.spyOn(globalThis, 'fetch');
      const big: ArtifactRef = { path: 'poc/big.log', sha256: '3'.repeat(64), size: 300 * 1024, kind: 'text', stored: 'copied' };
      render(<ArtifactBrowser findingId="fnd_1" artifacts={[big]} />);
      fireEvent.click(screen.getByRole('button', { name: /poc\/big\.log/ }));
      expect(screen.getByText(/Too large to preview \(300\.0 KB\)/)).toBeInTheDocument();
      expect(fetchSpy).not.toHaveBeenCalled();
    });
  });
});

describe('ReportHeader', () => {
  const finding = {
    id: 'fnd_1', ws_id: WS, project: 'notebin', target_id: 't', workflow_name: 'audit',
    severity: 'critical', summary: 's',
  } as unknown as FindingOut;

  it('renders title, severity chip, gap-styled owner, and CWE links', () => {
    render(<ReportHeader finding={finding} report={report} />);
    expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent(report.title);
    expect(screen.getByText('critical')).toBeInTheDocument();
    const owner = screen.getAllByText('Unknown').find((el) => el.tagName === 'DD' && el.previousElementSibling?.textContent === 'Owner');
    expect(owner).toHaveAttribute('data-gap', 'true');
    expect(screen.getByText('Notebin (sample app)')).toHaveAttribute('data-gap', 'false');
    expect(screen.getByRole('link', { name: 'CWE-639' })).toHaveAttribute('href', 'https://cwe.mitre.org/data/definitions/639.html');
    expect(screen.getByRole('link', { name: 'CWE-862' })).toHaveAttribute('href', 'https://cwe.mitre.org/data/definitions/862.html');
  });
});

describe('ReportHeader sentinels', () => {
  it('renders a "Not Provided" cell through sentinelLabel and styles it as a gap', () => {
    const finding = { id: 'fnd_1', ws_id: WS, project: 'p', target_id: 't', severity: 'high', summary: 's' } as unknown as FindingOut;
    render(<ReportHeader finding={finding} report={{ ...report, tickets: 'Not Provided — no tracker' }} />);
    const cell = screen.getByText('Not provided: no tracker');
    expect(cell).toHaveAttribute('data-gap', 'true');
    expect(screen.queryByText(/Not Provided —/)).toBeNull();
  });
});

describe('CrossReferences', () => {
  it('renders the None sentinel', () => {
    withRouter(<CrossReferences refs="None" references={report.references} />);
    expect(screen.getByText(/Related findings:/).parentElement).toHaveTextContent(/Related findings:\s*None$/);
  });

  it('renders a "Not Provided" sentinel through sentinelLabel', () => {
    withRouter(<CrossReferences refs="Not Provided — none found" references="x" />);
    expect(screen.getByText(/Related findings:/).parentElement).toHaveTextContent('Not provided: none found');
  });

  it('keys rows by id, relation and index so repeated ids do not collide', () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => {});
    withRouter(
      <CrossReferences
        refs={[
          { finding_id: 'fnd_9', relation: 'sibling' },
          { finding_id: 'fnd_9', relation: 'duplicate' },
        ]}
        references="x"
      />,
    );
    expect(screen.getAllByRole('link', { name: 'fnd_9' })).toHaveLength(2);
    expect(err).not.toHaveBeenCalled();
  });

  it('links related findings to their detail page', () => {
    withRouter(
      <CrossReferences
        refs={[{ finding_id: 'fnd_9', relation: 'sibling', note: 'same handler' }]}
        references="CWE-639"
      />,
    );
    expect(screen.getByRole('link', { name: 'fnd_9' })).toHaveAttribute('href', '/findings/fnd_9');
    expect(screen.getByText(/same handler/)).toBeInTheDocument();
  });
});
