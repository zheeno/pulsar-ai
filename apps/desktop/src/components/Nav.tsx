import { useState } from 'react';
import { Link, useLocation } from 'react-router-dom';
import { useCycle } from '../lib/cycle';
import { useSession } from '../lib/session';
import {
  IconHome,
  IconLogout,
  IconMemory,
  IconSettings,
  IconSignals,
  IconSpinner,
  IconStrategy,
  IconTrades,
} from './Icons';
import pulsarLogo from '../assets/pulsar-logo.svg';
import pulsarLogoFull from '../assets/pulsar-logo-full.svg';

const links = [
  { href: '/', label: 'Home', Icon: IconHome },
  { href: '/signals', label: 'Signals', Icon: IconSignals },
  { href: '/trades', label: 'Trades', Icon: IconTrades },
  { href: '/memory', label: 'Memory', Icon: IconMemory },
  { href: '/coach', label: 'Coach', Icon: IconStrategy },
  { href: '/settings', label: 'Settings', Icon: IconSettings },
];

export default function Nav() {
  const location = useLocation();
  const { logout } = useSession();
  const { running: cycleRunning } = useCycle();
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
        <img
          src={pulsarLogoFull}
          alt="Pulsar AI"
          className="nav-rail__brand-logo nav-rail__brand-logo--full"
        />
        <img
          src={pulsarLogo}
          alt="Pulsar AI"
          className="nav-rail__brand-logo nav-rail__brand-logo--mark"
        />
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
        {cycleRunning && (
          <div className="cycle-indicator" role="status" aria-live="polite">
            <IconSpinner size={14} />
            <span className="nav-rail__link-label">Cycle running…</span>
          </div>
        )}
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
