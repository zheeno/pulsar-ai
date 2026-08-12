import { Component, type ErrorInfo, type ReactNode } from 'react';

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
        <div style={{ padding: 48, color: '#e2e8f0', background: '#0b1220', minHeight: '100vh' }}>
          <h1 style={{ marginTop: 0 }}>Something went wrong</h1>
          <pre style={{ color: '#f87171', whiteSpace: 'pre-wrap' }}>{this.state.error.message}</pre>
          <button
            type="button"
            style={{ marginTop: 16, background: '#22c55e', color: 'white', border: 'none', padding: '10px 20px', borderRadius: 6, cursor: 'pointer' }}
            onClick={() => this.setState({ error: null })}
          >
            Try again
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}
