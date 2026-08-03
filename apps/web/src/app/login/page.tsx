'use client';

import { useState } from 'react';
import { useRouter } from 'next/navigation';
import { login } from '@/lib/api';

export default function LoginPage() {
  const [email, setEmail] = useState('admin@ngx.local');
  const [password, setPassword] = useState('admin123');
  const [error, setError] = useState('');
  const router = useRouter();

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const result = await login(email, password);
    if (result.access_token) {
      localStorage.setItem('token', result.access_token);
      router.push('/dashboard');
    } else {
      setError(result.error || 'Login failed');
    }
  }

  return (
    <div style={{ minHeight: '100vh', display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
      <form onSubmit={handleSubmit} style={{ background: '#1e293b', padding: 32, borderRadius: 12, width: 360 }}>
        <h1 style={{ margin: '0 0 24px', fontSize: 24 }}>NGX Trading Assistant</h1>
        {error && <p style={{ color: '#f87171', marginBottom: 16 }}>{error}</p>}
        <input
          type="email" value={email} onChange={(e) => setEmail(e.target.value)}
          placeholder="Email" style={inputStyle}
        />
        <input
          type="password" value={password} onChange={(e) => setPassword(e.target.value)}
          placeholder="Password" style={inputStyle}
        />
        <button type="submit" style={buttonStyle}>Sign In</button>
      </form>
    </div>
  );
}

const inputStyle: React.CSSProperties = {
  width: '100%', padding: '10px 12px', marginBottom: 12, borderRadius: 6,
  border: '1px solid #334155', background: '#0f172a', color: '#e2e8f0', boxSizing: 'border-box',
};
const buttonStyle: React.CSSProperties = {
  width: '100%', padding: '12px', borderRadius: 6, border: 'none',
  background: '#3b82f6', color: 'white', fontWeight: 600, cursor: 'pointer',
};
