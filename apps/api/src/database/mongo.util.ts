export function docToApi<T extends Record<string, unknown>>(doc: unknown): T | null {
  if (!doc || typeof doc !== 'object') return null;
  const record = doc as { toObject?: (opts?: { virtuals?: boolean }) => Record<string, unknown> };
  const obj =
    typeof record.toObject === 'function'
      ? record.toObject({ virtuals: true })
      : { ...(doc as Record<string, unknown>) };
  const { _id, __v, ...rest } = obj;
  return { id: _id, ...rest } as unknown as T;
}

export function docsToApi<T extends Record<string, unknown>>(docs: unknown[]): T[] {
  return docs.map((doc) => docToApi<T>(doc)!);
}

export function startOfDay(date = new Date()): Date {
  const d = new Date(date);
  d.setHours(0, 0, 0, 0);
  return d;
}

export function endOfDay(date = new Date()): Date {
  const d = new Date(date);
  d.setHours(23, 59, 59, 999);
  return d;
}
