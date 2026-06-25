import React, { Suspense } from 'react';
import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom';
import { ErrorBoundary } from './components/ErrorBoundary';
import Layout from './components/Layout';

// All page-level routes are lazy-loaded so each page lands in its own chunk
// and the main bundle only pays for the shell (Layout + router plumbing).
const Dashboard         = React.lazy(() => import('./pages/Dashboard'));
const RunDetail         = React.lazy(() => import('./pages/RunDetail'));
const Events            = React.lazy(() => import('./pages/Events'));
const Coverage          = React.lazy(() => import('./pages/Coverage'));
const CoverageDetail    = React.lazy(() => import('./pages/CoverageDetail'));
const Findings          = React.lazy(() => import('./pages/Findings'));
const Workflows         = React.lazy(() => import('./pages/Workflows'));
const WorkflowDetail    = React.lazy(() => import('./pages/WorkflowDetail'));
const Agents            = React.lazy(() => import('./pages/Agents'));
const AgentDetail       = React.lazy(() => import('./pages/AgentDetail'));
const AutoflowsDefs     = React.lazy(() => import('./pages/AutoflowsDefs'));
const Sessions          = React.lazy(() => import('./pages/Sessions'));
const SessionDetail     = React.lazy(() => import('./pages/SessionDetail'));
const Workers           = React.lazy(() => import('./pages/Workers'));
const Settings          = React.lazy(() => import('./pages/Settings'));
const AgentRuns         = React.lazy(() => import('./pages/runs/AgentRuns'));
const WorkflowRuns      = React.lazy(() => import('./pages/runs/WorkflowRuns'));
const AutoflowRuns      = React.lazy(() => import('./pages/runs/AutoflowRuns'));
const Projects          = React.lazy(() => import('./pages/Projects'));
const ProjectDetail     = React.lazy(() => import('./pages/ProjectDetail'));
const ProjectDefinitions = React.lazy(() => import('./pages/ProjectDefinitions'));
const RunTranscript     = React.lazy(() => import('./pages/RunTranscript'));

function PageFallback() {
  return (
    <div className="flex items-center justify-center h-48 text-ink-dim text-sm">
      Loading…
    </div>
  );
}

export default function App() {
  return (
    <BrowserRouter>
      <ErrorBoundary>
        <Routes>
          <Route element={<Layout />}>
            {/* Index redirect */}
            <Route index element={<Navigate to="/dashboard" replace />} />
            {/* Pages — wrapped in Suspense so the eager Layout shell paints first */}
            <Route path="/dashboard" element={<Suspense fallback={<PageFallback />}><Dashboard /></Suspense>} />
            {/* Run-stream pages — static segments MUST precede the :id wildcard */}
            <Route path="/runs/agents"    element={<Suspense fallback={<PageFallback />}><AgentRuns /></Suspense>} />
            <Route path="/runs/workflows" element={<Suspense fallback={<PageFallback />}><WorkflowRuns /></Suspense>} />
            <Route path="/runs/autoflows" element={<Suspense fallback={<PageFallback />}><AutoflowRuns /></Suspense>} />
            {/* Bare /runs → redirect to workflow runs (canonical execution list) */}
            <Route path="/runs" element={<Navigate to="/runs/workflows" replace />} />
            {/* Run detail graph — wildcard must come after static /runs/* segments */}
            <Route path="/runs/:id" element={<Suspense fallback={<PageFallback />}><RunDetail /></Suspense>} />
            <Route path="/events" element={<Suspense fallback={<PageFallback />}><Events /></Suspense>} />
            <Route path="/coverage" element={<Suspense fallback={<PageFallback />}><Coverage /></Suspense>} />
            <Route path="/coverage/:target" element={<Suspense fallback={<PageFallback />}><CoverageDetail /></Suspense>} />
            <Route path="/findings" element={<Suspense fallback={<PageFallback />}><Findings /></Suspense>} />
            <Route path="/workflows" element={<Suspense fallback={<PageFallback />}><Workflows /></Suspense>} />
            <Route path="/workflows/:name" element={<Suspense fallback={<PageFallback />}><WorkflowDetail /></Suspense>} />
            <Route path="/agents" element={<Suspense fallback={<PageFallback />}><Agents /></Suspense>} />
            <Route path="/agents/:name" element={<Suspense fallback={<PageFallback />}><AgentDetail /></Suspense>} />
            <Route path="/autoflows" element={<Suspense fallback={<PageFallback />}><AutoflowsDefs /></Suspense>} />
            <Route path="/sessions" element={<Suspense fallback={<PageFallback />}><Sessions /></Suspense>} />
            <Route path="/sessions/:id" element={<Suspense fallback={<PageFallback />}><SessionDetail /></Suspense>} />
            <Route path="/workers" element={<Suspense fallback={<PageFallback />}><Workers /></Suspense>} />
            <Route path="/settings" element={<Suspense fallback={<PageFallback />}><Settings /></Suspense>} />
            {/* Transcript-only page (agent/session/standalone runs with no DAG) */}
            <Route path="/transcript" element={<Suspense fallback={<PageFallback />}><RunTranscript /></Suspense>} />
            {/* Projects */}
            <Route path="/projects" element={<Suspense fallback={<PageFallback />}><Projects /></Suspense>} />
            {/* Static scoped sub-pages MUST come before the :wsId wildcard.
                The tabbed shell renders for overview + 4 tab routes; only
                Definitions stays a standalone page. */}
            <Route path="/projects/:wsId/runs" element={<Suspense fallback={<PageFallback />}><ProjectDetail tab="runs" /></Suspense>} />
            <Route path="/projects/:wsId/findings" element={<Suspense fallback={<PageFallback />}><ProjectDetail tab="findings" /></Suspense>} />
            <Route path="/projects/:wsId/sessions" element={<Suspense fallback={<PageFallback />}><ProjectDetail tab="sessions" /></Suspense>} />
            <Route path="/projects/:wsId/coverage" element={<Suspense fallback={<PageFallback />}><ProjectDetail tab="coverage" /></Suspense>} />
            <Route path="/projects/:wsId/definitions" element={<Suspense fallback={<PageFallback />}><ProjectDefinitions /></Suspense>} />
            <Route path="/projects/:wsId" element={<Suspense fallback={<PageFallback />}><ProjectDetail tab="overview" /></Suspense>} />
          </Route>
        </Routes>
      </ErrorBoundary>
    </BrowserRouter>
  );
}
