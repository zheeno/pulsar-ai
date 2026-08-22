import { useEffect } from 'react';
import { onExpired, onExpiring } from './auth-bridge';
import { playSessionRenewalAlert } from './notify-sound';
import { useToast } from './toast';

/** Global Busha auth-bridge session alerts (any page). */
export default function AuthBridgeSessionAlerts() {
  const toast = useToast();

  useEffect(() => {
    let unlistenExpiring: (() => void) | undefined;
    let unlistenExpired: (() => void) | undefined;

    void onExpiring((event) => {
      if (event.brokerId !== 'busha') return;
      playSessionRenewalAlert();
      toast.warning(
        'Busha session is expiring — sign in using the login window that just opened.',
        'Live broker',
        { sound: false },
      );
    }).then((fn) => {
      unlistenExpiring = fn;
    });

    void onExpired((event) => {
      if (event.brokerId !== 'busha') return;
      toast.warning(
        'Busha session expired. Reconnect to resume trading — crypto mode stays on.',
        'Live broker',
        { sound: true },
      );
    }).then((fn) => {
      unlistenExpired = fn;
    });

    return () => {
      unlistenExpiring?.();
      unlistenExpired?.();
    };
  }, [toast]);

  return null;
}
