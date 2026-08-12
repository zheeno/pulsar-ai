import { Component, type ErrorInfo, type ReactNode } from 'react';
import { IconShieldAlert } from './Icons';

type Props = { children: ReactNode };
type State = { error: Error | null };

export default class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error('UI crashed:', error, info.componentStack);
  }

  render() {
    if (this.state.error) {
      return (
        <div className="boot-screen">
          <div className="onboarding-card" style={{ textAlign: 'left' }}>
            <div className="onboarding-brand">
              <span className="nav-rail__brand-mark" style={{ color: 'var(--status-bad)', background: 'var(--status-bad-soft)' }}>
                <IconShieldAlert size={18} />
              </span>
              Pulsar AI
            </div>
            <h1 style={{ marginTop: 0 }}>Something went wrong</h1>
            <pre className="pre-log" style={{ color: 'var(--status-bad)' }}>{this.state.error.message}</pre>
            <div className="btn-row">
              <button type="button" className="btn btn-primary" onClick={() => this.setState({ error: null })}>
                Try again
              </button>
            </div>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
