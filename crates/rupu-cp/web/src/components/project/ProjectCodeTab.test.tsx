// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, beforeAll, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { ThemeProvider } from '../theme/ThemeProvider';
import ProjectCodeTab from './ProjectCodeTab';
import { api } from '../../lib/api';

// jsdom doesn't implement scrollIntoView (CodeViewer's initialLine auto-scroll).
beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('ProjectCodeTab', () => {
  it('loads the tree and findings and shows an empty-state prompt before a file is picked', async () => {
    vi.spyOn(api, 'getProjectTree').mockResolvedValue({ path: '', parent: null, entries: [] });
    vi.spyOn(api, 'getFindings').mockResolvedValue({ findings: [], summary: { total: 0, critical: 0, high: 0, medium: 0, low: 0, info: 0 } } as never);
    render(
      <MemoryRouter initialEntries={['/projects/ws1/code']}>
        <ThemeProvider>
          <ProjectCodeTab wsId="ws1" />
        </ThemeProvider>
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText(/select a file/i)).toBeInTheDocument());
  });

  it('opens the file named by the ?path= deep-link', async () => {
    vi.spyOn(api, 'getProjectTree').mockResolvedValue({ path: '', parent: null, entries: [] });
    vi.spyOn(api, 'getFindings').mockResolvedValue({ findings: [], summary: { total: 0, critical: 0, high: 0, medium: 0, low: 0, info: 0 } } as never);
    const src = vi.spyOn(api, 'getProjectSource').mockResolvedValue({ available: true, path: 'src/a.rs', language: 'rust', totalLines: 1, lines: [{ n: 1, text: 'fn a() {}' }] });
    render(
      <MemoryRouter initialEntries={['/projects/ws1/code?path=src%2Fa.rs&line=1']}>
        <ThemeProvider>
          <ProjectCodeTab wsId="ws1" />
        </ThemeProvider>
      </MemoryRouter>,
    );
    await waitFor(() => expect(src).toHaveBeenCalledWith('ws1', 'src/a.rs'));
  });
});
