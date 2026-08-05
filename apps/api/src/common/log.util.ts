import { Logger } from '@nestjs/common';

/** Format optional metadata for log lines. */
export function metaString(meta?: Record<string, unknown>): string {
  if (!meta || Object.keys(meta).length === 0) return '';
  try {
    return ` ${JSON.stringify(meta)}`;
  } catch {
    return ' [unserializable metadata]';
  }
}

/** Log method entry; returns a helper to log completion. */
export function logStart(logger: Logger, method: string, meta?: Record<string, unknown>) {
  logger.log(`→ ${method}${metaString(meta)}`);
  return {
    done: (result?: Record<string, unknown>) =>
      logger.log(`✓ ${method}${metaString(result)}`),
    debug: (message: string, detail?: Record<string, unknown>) =>
      logger.debug(`  ${method}: ${message}${metaString(detail)}`),
    warn: (message: string, detail?: Record<string, unknown>) =>
      logger.warn(`  ${method}: ${message}${metaString(detail)}`),
    fail: (err: unknown) =>
      logger.error(`✗ ${method}: ${err instanceof Error ? err.message : String(err)}`),
  };
}
