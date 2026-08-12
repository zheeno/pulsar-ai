import { createContext, useContext } from 'react';

type SessionApi = {
  logout: () => Promise<void>;
};

export const SessionContext = createContext<SessionApi | null>(null);

export function useSession(): SessionApi {
  const ctx = useContext(SessionContext);
  if (!ctx) {
    throw new Error('useSession must be used within SessionContext');
  }
  return ctx;
}
