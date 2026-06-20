import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom';
import { ErrorBoundary } from './components/ErrorBoundary';
import Layout from './components/Layout';
import Dashboard from './pages/Dashboard';
import RunDetail from './pages/RunDetail';
import Events from './pages/Events';
import Coverage from './pages/Coverage';
import CoverageDetail from './pages/CoverageDetail';
import Workflows from './pages/Workflows';
import WorkflowDetail from './pages/WorkflowDetail';
import Agents from './pages/Agents';
import AgentDetail from './pages/AgentDetail';
import AutoflowsDefs from './pages/AutoflowsDefs';
import Sessions from './pages/Sessions';
import SessionDetail from './pages/SessionDetail';
import Workers from './pages/Workers';
import Settings from './pages/Settings';
import AgentRuns from './pages/runs/AgentRuns';
import WorkflowRuns from './pages/runs/WorkflowRuns';
import AutoflowRuns from './pages/runs/AutoflowRuns';
import Projects from './pages/Projects';
import ProjectDetail from './pages/ProjectDetail';
import ProjectRuns from './pages/ProjectRuns';
import ProjectSessions from './pages/ProjectSessions';
import ProjectCoverage from './pages/ProjectCoverage';
import ProjectDefinitions from './pages/ProjectDefinitions';
import RunTranscript from './pages/RunTranscript';

export default function App() {
  return (
    <BrowserRouter>
      <ErrorBoundary>
        <Routes>
          <Route element={<Layout />}>
            {/* Index redirect */}
            <Route index element={<Navigate to="/dashboard" replace />} />
            {/* Pages */}
            <Route path="/dashboard" element={<Dashboard />} />
            {/* Run-stream pages — static segments MUST precede the :id wildcard */}
            <Route path="/runs/agents"    element={<AgentRuns />} />
            <Route path="/runs/workflows" element={<WorkflowRuns />} />
            <Route path="/runs/autoflows" element={<AutoflowRuns />} />
            {/* Bare /runs → redirect to workflow runs (canonical execution list) */}
            <Route path="/runs" element={<Navigate to="/runs/workflows" replace />} />
            {/* Run detail graph — wildcard must come after static /runs/* segments */}
            <Route path="/runs/:id" element={<RunDetail />} />
            <Route path="/events" element={<Events />} />
            <Route path="/coverage" element={<Coverage />} />
            <Route path="/coverage/:target" element={<CoverageDetail />} />
            <Route path="/workflows" element={<Workflows />} />
            <Route path="/workflows/:name" element={<WorkflowDetail />} />
            <Route path="/agents" element={<Agents />} />
            <Route path="/agents/:name" element={<AgentDetail />} />
            <Route path="/autoflows" element={<AutoflowsDefs />} />
            <Route path="/sessions" element={<Sessions />} />
            <Route path="/sessions/:id" element={<SessionDetail />} />
            <Route path="/workers" element={<Workers />} />
            <Route path="/settings" element={<Settings />} />
            {/* Transcript-only page (agent/session/standalone runs with no DAG) */}
            <Route path="/transcript" element={<RunTranscript />} />
            {/* Projects */}
            <Route path="/projects" element={<Projects />} />
            {/* Static scoped sub-pages MUST come before the :wsId wildcard */}
            <Route path="/projects/:wsId/runs" element={<ProjectRuns />} />
            <Route path="/projects/:wsId/sessions" element={<ProjectSessions />} />
            <Route path="/projects/:wsId/coverage" element={<ProjectCoverage />} />
            <Route path="/projects/:wsId/definitions" element={<ProjectDefinitions />} />
            <Route path="/projects/:wsId" element={<ProjectDetail />} />
          </Route>
        </Routes>
      </ErrorBoundary>
    </BrowserRouter>
  );
}
