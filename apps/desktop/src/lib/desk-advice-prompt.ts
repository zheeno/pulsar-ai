const STORAGE_KEY = 'pulsar.deskAdvice.prompt.v1';

export type DeskAdvicePrompt = {
  dismissed: boolean;
  silenced: boolean;
};

type StoredPrompt = {
  cycleId: string;
  dismissed?: boolean;
  silenced?: boolean;
};

export function deskAdviceCycleKey(lastCycleId?: string | null): string {
  const id = lastCycleId?.trim();
  return id ? id : 'none';
}

export function readDeskAdvicePrompt(cycleId: string): DeskAdvicePrompt {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { dismissed: false, silenced: false };
    const parsed = JSON.parse(raw) as StoredPrompt;
    if (!parsed || parsed.cycleId !== cycleId) {
      return { dismissed: false, silenced: false };
    }
    return {
      dismissed: Boolean(parsed.dismissed),
      silenced: Boolean(parsed.silenced),
    };
  } catch {
    return { dismissed: false, silenced: false };
  }
}

export function writeDeskAdvicePrompt(
  cycleId: string,
  patch: Partial<DeskAdvicePrompt>,
): DeskAdvicePrompt {
  const next = { ...readDeskAdvicePrompt(cycleId), ...patch };
  try {
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        cycleId,
        dismissed: next.dismissed,
        silenced: next.silenced,
      } satisfies StoredPrompt),
    );
  } catch {
    /* private mode */
  }
  return next;
}
