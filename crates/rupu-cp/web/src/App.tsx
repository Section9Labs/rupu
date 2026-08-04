import React, { Suspense, type ReactElement } from 'react';
import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom';
import { ErrorBoundary } from './components/ErrorBoundary';
import Layout from './components/Layout';
import { Spinner } from './components/ui/Spinner';
import { getShell, type ShellVersion } from './lib/shell';

// All page-level routes are lazy-loaded so each page lands in its own chunk
// and the main bundle only pays for the shell (Layout + router plumbing).
const Dashboard         = React.lazy(() => import('./pages/Dashboard'));
const Usage             = React.lazy(() => import('./pages/Usage'));
const RunDetail         = React.lazy(() => import('./pages/RunDetail'));
const Events            = React.lazy(() => import('./pages/Events'));
const Coverage          = React.lazy(() => import('./pages/Coverage'));
const Netflow           = React.lazy(() => import('./pages/Netflow'));
const CoverageDetail    = React.lazy(() => import('./pages/CoverageDetail'));
const CoverageTemplates = React.lazy(() => import('./pages/CoverageTemplates'));
const Findings          = React.lazy(() => import('./pages/Findings'));
const Workflows         = React.lazy(() => import('./pages/Workflows'));
const WorkflowDetail    = React.lazy(() => import('./pages/WorkflowDetail'));
const Agents            = React.lazy(() => import('./pages/Agents'));
const AgentNew          = React.lazy(() => import('./pages/AgentNew'));
const AgentDetail       = React.lazy(() => import('./pages/AgentDetail'));
const AutoflowsDefs     = React.lazy(() => import('./pages/AutoflowsDefs'));
const Sessions          = React.lazy(() => import('./pages/Sessions'));
const SessionDetail     = React.lazy(() => import('./pages/SessionDetail'));
const Workers           = React.lazy(() => import('./pages/Workers'));
const Hosts             = React.lazy(() => import('./pages/Hosts'));
const HostDetail        = React.lazy(() => import('./pages/HostDetail'));
const Settings          = React.lazy(() => import('./pages/Settings'));
const AgentRuns         = React.lazy(() => import('./pages/runs/AgentRuns'));
const WorkflowRuns      = React.lazy(() => import('./pages/runs/WorkflowRuns'));
const AutoflowRuns      = React.lazy(() => import('./pages/runs/AutoflowRuns'));
const Projects          = React.lazy(() => import('./pages/Projects'));
const ProjectDetail     = React.lazy(() => import('./pages/ProjectDetail'));
const ProjectDefinitions = React.lazy(() => import('./pages/ProjectDefinitions'));
const RunTranscript     = React.lazy(() => import('./pages/RunTranscript'));

// Shell v2 chrome + the composite destination pages that carry the 7-leaf IA
// (docs/redesign) over the existing v1 page bodies.
const ShellV2   = React.lazy(() => import('./components/v2/Shell'));
const ActivityV2 = React.lazy(() => import('./components/v2/pages/ActivityV2'));
const SecurityV2 = React.lazy(() => import('./components/v2/pages/SecurityV2'));
const LibraryV2  = React.lazy(() => import('./components/v2/pages/LibraryV2'));
const FleetV2    = React.lazy(() => import('./components/v2/pages/FleetV2'));

function PageFallback() {
  return (
    <div className="flex items-center justify-center h-48">
      <Spinner size="md" label="Loading…" />
    </div>
  );
}

// Wraps a lazily-loaded page element in the shared Suspense fallback — a
// pure refactor of the `<Suspense fallback={<PageFallback/>}>{el}</Suspense>`
// pattern that used to be repeated at every `<Route element=…>`.
function page(el: ReactElement) {
  return <Suspense fallback={<PageFallback />}>{el}</Suspense>;
}

export function AppRoutes({ shell }: { shell: ShellVersion }) {
  const v2 = shell === 'v2';
  const layoutEl = v2
    ? <Suspense fallback={<PageFallback />}><ShellV2 /></Suspense>
    : <Layout />;
  return (
    <Routes>
      <Route element={layoutEl}>
        {/* Index redirect */}
        <Route index element={<Navigate to={v2 ? '/overview' : '/dashboard'} replace />} />

        {/* v2 destinations — registered under BOTH shells so deep links work */}
        <Route path="/overview" element={page(<Dashboard />)} />
        <Route path="/activity" element={page(<ActivityV2 />)} />
        <Route path="/security" element={page(<SecurityV2 />)} />
        <Route path="/library" element={page(<LibraryV2 />)} />
        <Route path="/fleet" element={page(<FleetV2 />)} />

        {/* Pages — wrapped in Suspense so the eager Layout shell paints first.
            v1 list paths redirect into the v2 IA only when the flag is on. */}
        <Route path="/dashboard" element={v2 ? <Navigate to="/overview" replace /> : page(<Dashboard />)} />
        <Route path="/usage" element={page(<Usage />)} />
        {/* Run-stream pages — static segments MUST precede the :id wildcard */}
        <Route path="/runs/agents"    element={v2 ? <Navigate to="/activity?tab=agents" replace /> : page(<AgentRuns />)} />
        <Route path="/runs/workflows" element={v2 ? <Navigate to="/activity?tab=workflows" replace /> : page(<WorkflowRuns />)} />
        <Route path="/runs/autoflows" element={v2 ? <Navigate to="/activity?tab=autoflows" replace /> : page(<AutoflowRuns />)} />
        {/* Bare /runs → redirect to workflow runs (canonical execution list) */}
        <Route path="/runs" element={<Navigate to={v2 ? '/activity' : '/runs/workflows'} replace />} />
        {/* Run detail graph — wildcard must come after static /runs/* segments */}
        <Route path="/runs/:id" element={page(<RunDetail />)} />
        <Route path="/events" element={page(<Events />)} />
        <Route path="/coverage" element={v2 ? <Navigate to="/security?tab=coverage" replace /> : page(<Coverage />)} />
        <Route path="/coverage/templates" element={v2 ? <Navigate to="/security?tab=catalog" replace /> : page(<CoverageTemplates />)} />
        <Route path="/coverage/:target/catalog" element={page(<CoverageDetail tab="catalog" />)} />
        <Route path="/coverage/:target/audit" element={page(<CoverageDetail tab="audit" />)} />
        <Route path="/coverage/:target/gap" element={page(<CoverageDetail tab="gap" />)} />
        <Route path="/coverage/:target/diff" element={page(<CoverageDetail tab="diff" />)} />
        <Route path="/coverage/:target" element={page(<CoverageDetail />)} />
        <Route path="/netflow" element={page(<Netflow />)} />
        <Route path="/findings" element={v2 ? <Navigate to="/security?tab=findings" replace /> : page(<Findings />)} />
        <Route path="/workflows" element={v2 ? <Navigate to="/library?tab=workflows" replace /> : page(<Workflows />)} />
        <Route path="/workflows/:name" element={page(<WorkflowDetail />)} />
        <Route path="/agents" element={v2 ? <Navigate to="/library?tab=agents" replace /> : page(<Agents />)} />
        {/* Static /agents/new MUST precede the :name wildcard */}
        <Route path="/agents/new" element={page(<AgentNew />)} />
        <Route path="/agents/:name" element={page(<AgentDetail />)} />
        <Route path="/autoflows" element={v2 ? <Navigate to="/library?tab=autoflows" replace /> : page(<AutoflowsDefs />)} />
        <Route path="/sessions" element={v2 ? <Navigate to="/activity?tab=sessions" replace /> : page(<Sessions />)} />
        <Route path="/sessions/:id" element={page(<SessionDetail />)} />
        <Route path="/hosts" element={v2 ? <Navigate to="/fleet" replace /> : page(<Hosts />)} />
        <Route path="/hosts/:id" element={page(<HostDetail />)} />
        <Route path="/workers" element={v2 ? <Navigate to="/fleet?tab=workers" replace /> : page(<Workers />)} />
        <Route path="/settings" element={page(<Settings />)} />
        {/* Transcript-only page (agent/session/standalone runs with no DAG) */}
        <Route path="/transcript" element={page(<RunTranscript />)} />
        {/* Projects */}
        <Route path="/projects" element={page(<Projects />)} />
        {/* Static scoped sub-pages MUST come before the :wsId wildcard.
            The tabbed shell renders for overview + 5 tab routes; only
            Definitions stays a standalone page. */}
        <Route path="/projects/:wsId/runs" element={page(<ProjectDetail tab="runs" />)} />
        <Route path="/projects/:wsId/findings" element={page(<ProjectDetail tab="findings" />)} />
        <Route path="/projects/:wsId/code" element={page(<ProjectDetail tab="code" />)} />
        <Route path="/projects/:wsId/sessions" element={page(<ProjectDetail tab="sessions" />)} />
        <Route path="/projects/:wsId/coverage" element={page(<ProjectDetail tab="coverage" />)} />
        <Route path="/projects/:wsId/network" element={page(<ProjectDetail tab="network" />)} />
        <Route path="/projects/:wsId/config" element={page(<ProjectDetail tab="config" />)} />
        <Route path="/projects/:wsId/definitions" element={page(<ProjectDefinitions />)} />
        <Route path="/projects/:wsId" element={page(<ProjectDetail tab="overview" />)} />
      </Route>
    </Routes>
  );
}

export default function App() {
  const shell = getShell();
  return (
    <BrowserRouter>
      <ErrorBoundary>
        <AppRoutes shell={shell} />
      </ErrorBoundary>
    </BrowserRouter>
  );
}
