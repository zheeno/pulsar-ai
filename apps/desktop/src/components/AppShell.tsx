import { Outlet } from 'react-router-dom';
import Nav from './Nav';

export default function AppShell() {
  return (
    <div className="app-shell">
      <Nav />
      <main className="app-main">
        <Outlet />
      </main>
    </div>
  );
}
