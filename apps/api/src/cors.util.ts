/** Build the list of browser origins allowed to call the API. */
export function getAllowedOrigins(): string[] {
  const origins = new Set<string>();

  const add = (value?: string) => {
    if (!value) return;
    const normalized = value.trim().replace(/\/$/, '');
    if (normalized) origins.add(normalized);
  };

  for (const entry of (process.env.CORS_ORIGIN || '').split(',')) {
    add(entry);
  }

  const webDomain = process.env.WEB_DOMAIN?.trim();
  if (webDomain) {
    add(`https://${webDomain}`);
    add(`http://${webDomain}`);
    if (!webDomain.startsWith('www.')) {
      add(`https://www.${webDomain}`);
      add(`http://www.${webDomain}`);
    }
  }

  if (origins.size === 0) {
    add('http://localhost:3000');
  }

  return [...origins];
}

export function isOriginAllowed(origin: string | undefined, allowedOrigins: string[]): boolean {
  if (!origin) return true;
  const normalized = origin.replace(/\/$/, '');
  return allowedOrigins.includes(normalized) || allowedOrigins.includes(origin);
}
