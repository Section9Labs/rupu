import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom';
import { ErrorBoundary } from './components/ErrorBoundary';
import Layout from './components/Layout';
import Dashboard from './pages/Dashboard';
import Runs from './pages/Runs';
import RunDetail from './pages/RunDetail';
import Events from './pages/Events';
import Coverage from './pages/Coverage';
import CoverageDetail from './pages/CoverageDetail';
import Workflows from './pages/Workflows';
import WorkflowDetail from './pages/WorkflowDetail';
import Agents from './pages/Agents';
import AgentDetail from './pages/AgentDetail';
import Sessions from './pages/Sessions';
import SessionDetail from './pages/SessionDetail';
import Workers from './pages/Workers';
import Settings from './pages/Settings';

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
            <Route path="/runs" element={<Runs />} />
            <Route path="/runs/:id" element={<RunDetail />} />
            <Route path="/events" element={<Events />} />
            <Route path="/coverage" element={<Coverage />} />
            <Route path="/coverage/:target" element={<CoverageDetail />} />
            <Route path="/workflows" element={<Workflows />} />
            <Route path="/workflows/:name" element={<WorkflowDetail />} />
            <Route path="/agents" element={<Agents />} />
            <Route path="/agents/:name" element={<AgentDetail />} />
            <Route path="/sessions" element={<Sessions />} />
            <Route path="/sessions/:id" element={<SessionDetail />} />
            <Route path="/workers" element={<Workers />} />
            <Route path="/settings" element={<Settings />} />
          </Route>
        </Routes>
      </ErrorBoundary>
    </BrowserRouter>
  );
}
