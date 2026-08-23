import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import { hideNativeSplash } from './lib/native-splash';
import './styles.css';

function showBootError(message: string) {
  hideNativeSplash();
  const el = document.getElementById('boot-error');
  if (!el) return;
  el.textContent = message;
  el.classList.add('is-visible');
}

window.addEventListener('error', (event) => {
  showBootError(`Pulsar AI failed to start: ${event.message}`);
});

window.addEventListener('unhandledrejection', (event) => {
  const reason = event.reason instanceof Error ? event.reason.message : String(event.reason);
  showBootError(`Pulsar AI failed to start: ${reason}`);
});

const root = document.getElementById('root');
if (!root) {
  showBootError('Pulsar AI failed to start: missing #root element.');
} else {
  ReactDOM.createRoot(root).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}
