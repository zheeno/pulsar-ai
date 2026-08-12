import { useState } from 'react';
import { Link, useLocation } from 'react-router-dom';
import { useSession } from '../lib/session';

const links = [
  { href: '/', label: 'Dashboard' },
  { href: '/signals', label: 'Signals' },
  { href: '/trades', label: 'Trades' },
  { href: '/strategy', label: 'Strategy' },
  { href: '/backtest', label: 'Backtest' },
  { href: '/settings', label: 'Settings' },
];

export default function Nav() {
  const location = useLocation();
  const { logout } = useSession();
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);

  async function handleLogout() {
    if (!confirming) {
      setConfirming(true);
      return;
    }
    setBusy(true);
    try {
      await logout();
    } catch (e) {
      setBusy(false);
      setConfirming(false);
      console.error('Logout failed', e);
      // Prefer inline status over alert() — WKWebView often blocks window.alert/confirm
      window.dispatchEvent(new CustomEvent('pulsar:toast', { detail: `Logout failed: ${e}` }));
    }
  }

  return (
    <nav style={{
      display: 'flex', alignItems: 'center', gap: 16, padding: '12px 24px',
      background: '#0f172a', borderBottom: '1px solid #1e293b',
    }}>
      <span style={{ fontWeight: 700, color: '#38bdf8', marginRight: 16 }}>Pulsar AI</span>
      {links.map((link) => (
        <Link
          key={link.href}
          to={link.href}
          style={{
            color: location.pathname === link.href ? '#38bdf8' : '#94a3b8',
            textDecoration: 'none', fontSize: 14,
          }}
        >
          {link.label}
        </Link>
      ))}
      <div style={{ marginLeft: 'auto', display: 'flex', gap: 8, alignItems: 'center' }}>
        {confirming && (
          <button
            type="button"
            disabled={busy}
            onClick={() => setConfirming(false)}
            style={{
              background: 'transparent',
              border: '1px solid #475569',
              color: '#94a3b8',
              padding: '6px 12px',
              borderRadius: 6,
              cursor: 'pointer',
              fontSize: 13,
            }}
          >
            Cancel
          </button>
        )}
        <button
          type="button"
          disabled={busy}
          onClick={() => void handleLogout()}
          style={{
            background: confirming ? '#7f1d1d' : 'transparent',
            border: `1px solid ${confirming ? '#ef4444' : '#475569'}`,
            color: confirming ? '#fecaca' : '#94a3b8',
            padding: '6px 12px',
            borderRadius: 6,
            cursor: busy ? 'wait' : 'pointer',
            fontSize: 13,
          }}
        >
          {busy ? 'Logging out…' : confirming ? 'Confirm log out' : 'Log out'}
        </button>
      </div>
    </nav>
  );
}
