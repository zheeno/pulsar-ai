import { useState } from 'react';
import { Link, useLocation } from 'react-router-dom';
import { useSession } from '../lib/session';
import {
  IconBacktest,
  IconHome,
  IconLogout,
  IconSettings,
  IconSignals,
  IconSpinner,
  IconShieldCheck,
  IconTrades,
} from './Icons';

const links = [
  { href: '/', label: 'Home', Icon: IconHome },
  { href: '/signals', label: 'Signals', Icon: IconSignals },
  { href: '/trades', label: 'Trades', Icon: IconTrades },
  { href: '/backtest', label: 'Backtest', Icon: IconBacktest },
  { href: '/settings', label: 'Settings', Icon: IconSettings },
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
    }
  }

  return (
    <aside className="nav-rail" aria-label="Primary">
      <div className="nav-rail__brand">
        <span className="nav-rail__brand-mark"><IconShieldCheck size={18} /></span>
        <span className="nav-rail__brand-text">Pulsar AI</span>
      </div>
      <nav className="nav-rail__links" aria-label="Main">
        {links.map(({ href, label, Icon }) => {
          const active = location.pathname === href;
          return (
            <Link
              key={href}
              to={href}
              className={`nav-rail__link${active ? ' is-active' : ''}`}
              aria-current={active ? 'page' : undefined}
            >
              <Icon size={18} />
              <span className="nav-rail__link-label">{label}</span>
            </Link>
          );
        })}
      </nav>
      <div className="nav-rail__footer">
        {confirming && (
          <button
            type="button"
            className="btn btn-ghost"
            disabled={busy}
            onClick={() => setConfirming(false)}
          >
            Cancel
          </button>
        )}
        <button
          type="button"
          className={`btn ${confirming ? 'btn-danger' : 'btn-ghost'}`}
          disabled={busy}
          onClick={() => void handleLogout()}
        >
          {busy ? <IconSpinner /> : <IconLogout size={16} />}
          {busy ? 'Logging out…' : confirming ? 'Confirm log out' : 'Log out'}
        </button>
      </div>
    </aside>
  );
}
