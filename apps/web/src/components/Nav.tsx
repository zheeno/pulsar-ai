'use client';

import Link from 'next/link';
import { usePathname } from 'next/navigation';

const nav = [
  { href: '/dashboard', label: 'Portfolio' },
  { href: '/signals', label: 'Signals' },
  { href: '/trades', label: 'Trades' },
  { href: '/strategy', label: 'Strategy' },
  { href: '/backtest', label: 'Backtest' },
];

export default function Nav() {
  const pathname = usePathname();
  return (
    <nav style={{ background: '#1e293b', padding: '12px 24px', display: 'flex', gap: 24, alignItems: 'center' }}>
      <strong style={{ marginRight: 'auto' }}>NGX AI Trading</strong>
      {nav.map((item) => (
        <Link key={item.href} href={item.href} style={{
          color: pathname === item.href ? '#60a5fa' : '#94a3b8',
          textDecoration: 'none', fontWeight: pathname === item.href ? 600 : 400,
        }}>
          {item.label}
        </Link>
      ))}
      <button onClick={() => { localStorage.removeItem('token'); window.location.href = '/login'; }}
        style={{ background: 'none', border: '1px solid #475569', color: '#94a3b8', padding: '4px 12px', borderRadius: 4, cursor: 'pointer' }}>
        Logout
      </button>
    </nav>
  );
}
