import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { api } from './api';

export type AuthSessionStatusKind =
  | 'disconnected'
  | 'connecting'
  | 'connected'
  | 'expired'
  | 'cancelled'
  | 'timedOut';

export type AuthSessionStatus = {
  brokerId: string;
  displayName: string;
  status: AuthSessionStatusKind;
  expiresAt?: string | null;
  accountHint?: string | null;
};

export type AuthExpiredEvent = {
  brokerId: string;
};

function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export async function authenticate(
  brokerIdOrConfig: string | { id: string },
): Promise<AuthSessionStatus> {
  const brokerId =
    typeof brokerIdOrConfig === 'string' ? brokerIdOrConfig : brokerIdOrConfig.id;
  return api<AuthSessionStatus>('auth_bridge_authenticate', { brokerId });
}

export async function getSession(brokerId: string): Promise<AuthSessionStatus> {
  return api<AuthSessionStatus>('auth_bridge_session', { brokerId });
}

export async function listSessions(): Promise<AuthSessionStatus[]> {
  return api<AuthSessionStatus[]>('auth_bridge_list');
}

export async function revoke(brokerId: string): Promise<AuthSessionStatus> {
  return api<AuthSessionStatus>('auth_bridge_revoke', { brokerId });
}

export async function onExpired(
  callback: (event: AuthExpiredEvent) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) {
    return () => undefined;
  }
  return listen<AuthExpiredEvent>('auth-bridge:expired', (event) => {
    callback(event.payload);
  });
}
