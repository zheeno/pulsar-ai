import { Link, useLocation } from 'react-router-dom';

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

  return (
    <nav style={{
      display: 'flex', gap: 16, padding: '12px 24px',
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
    </nav>
  );
}
