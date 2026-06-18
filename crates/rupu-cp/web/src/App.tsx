import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom';
import { ErrorBoundary } from './components/ErrorBoundary';
import Layout from './components/Layout';
import Dashboard from './pages/Dashboard';
import Runs from './pages/Runs';
import RunDetail from './pages/RunDetail';
import Events from './pages/Events';
import Coverage from './pages/Coverage';
import Workflows from './pages/Workflows';
import Agents from './pages/Agents';
import Sessions from './pages/Sessions';
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
            <Route path="/workflows/*" element={<Workflows />} />
            <Route path="/agents/*" element={<Agents />} />
            <Route path="/sessions/*" element={<Sessions />} />
            <Route path="/workers/*" element={<Workers />} />
            <Route path="/settings" element={<Settings />} />
          </Route>
        </Routes>
      </ErrorBoundary>
    </BrowserRouter>
  );
}
