import { Component, type ErrorInfo, type ReactNode } from 'react';
import { AlertTriangle, RefreshCcw } from 'lucide-react';
import { Button } from './ui/Button';

interface State {
  error: Error | null;
  componentStack: string | null;
}

// Top-level error boundary used at the app root. Catches render errors
// from any page so a single bad component doesn't take the whole UI to
// a white screen — instead the operator sees the error message + a
// reload button.
export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null, componentStack: null };

  static getDerivedStateFromError(error: Error): State {
    return { error, componentStack: null };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    this.setState({ error, componentStack: info.componentStack ?? null });
    console.error('UI ErrorBoundary caught:', error, info);
  }

  render() {
    if (this.state.error) {
      return (
        <div className="min-h-screen flex items-start justify-center px-4 py-16 bg-bg">
          <div className="max-w-2xl w-full bg-panel border border-red-200 rounded-xl shadow-card p-6">
            <div className="flex items-center gap-2 text-red-700 mb-3">
              <AlertTriangle size={18} />
              <h1 className="text-lg font-semibold">UI crashed during render</h1>
            </div>
            <p className="text-sm text-ink-dim mb-4">
              A React component threw while rendering this page. The error is below.
              This is a bug — please share the message so it can be fixed.
            </p>
            <pre className="text-xs bg-red-50 border border-red-200 rounded-md p-3 overflow-auto max-h-48 text-red-900 font-mono whitespace-pre-wrap">
{this.state.error.message}
{this.state.error.stack && '\n\n' + this.state.error.stack}
            </pre>
            {this.state.componentStack && (
              <details className="mt-3">
                <summary className="text-xs text-ink-dim cursor-pointer">component stack</summary>
                <pre className="text-note bg-slate-50 border border-border rounded-md p-3 overflow-auto max-h-48 mt-2 text-slate-800 font-mono whitespace-pre-wrap">
{this.state.componentStack}
                </pre>
              </details>
            )}
            <div className="mt-5 flex items-center gap-2">
              <Button onClick={() => location.reload()} className="gap-1.5 text-sm">
                <RefreshCcw size={13} />
                Reload page
              </Button>
              <Button
                variant="secondary"
                onClick={() => this.setState({ error: null, componentStack: null })}
                className="text-sm"
              >
                Dismiss
              </Button>
            </div>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
