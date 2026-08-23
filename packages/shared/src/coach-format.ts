/** Normalize Coach chat text so the bubble never shows a JSON envelope or literal `\n`. */

const ENVELOPE_HINT = /needMoreContext|"clarifyingQuestions"|"patch"\s*:/;

function asText(raw: unknown): string {
  if (typeof raw === 'string') return raw;
  if (raw == null) return '';
  if (typeof raw === 'number' || typeof raw === 'boolean') return String(raw);
  return '';
}

function unwrapFence(text: string): string {
  const m = text.match(/^```(?:json|markdown|md|gfm)?\s*([\s\S]*?)\s*```$/i);
  return m?.[1] ? m[1].trim() : text;
}

function unescapeJsonish(text: string): string {
  let out = '';
  for (let i = 0; i < text.length; i += 1) {
    const c = text[i];
    if (c === '\\' && i + 1 < text.length) {
      const n = text[i + 1];
      if (n === 'n') {
        out += '\n';
        i += 1;
        continue;
      }
      if (n === 't') {
        out += '  ';
        i += 1;
        continue;
      }
      if (n === '"' || n === '\\' || n === '/') {
        out += n;
        i += 1;
        continue;
      }
    }
    out += c;
  }
  return out;
}

/** Read a JSON string that may contain raw (invalid) newlines. */
function readJsonString(src: string, startQuote: number): { value: string; end: number } | null {
  if (src[startQuote] !== '"') return null;
  let i = startQuote + 1;
  let out = '';
  while (i < src.length) {
    const c = src[i];
    if (c === '\\' && i + 1 < src.length) {
      const n = src[i + 1];
      if (n === 'n') out += '\n';
      else if (n === 't') out += '\t';
      else if (n === 'r') out += '';
      else if (n === '"' || n === '\\' || n === '/') out += n;
      else if (n === 'u' && i + 5 < src.length) {
        const hex = src.slice(i + 2, i + 6);
        const code = Number.parseInt(hex, 16);
        out += Number.isFinite(code) ? String.fromCharCode(code) : n;
        i += 4;
      } else out += n;
      i += 2;
      continue;
    }
    if (c === '"') return { value: out, end: i + 1 };
    out += c;
    i += 1;
  }
  return { value: out, end: src.length };
}

function extractEnvelopeSummary(text: string): string | null {
  if (!ENVELOPE_HINT.test(text) && !/"summary"\s*:/.test(text)) return null;
  const trimmed = text.trim();
  if (trimmed.startsWith('{')) {
    try {
      const obj = JSON.parse(trimmed) as { summary?: unknown };
      if (typeof obj.summary === 'string' && obj.summary.trim()) return obj.summary;
    } catch {
      // Broken JSON (often raw newlines inside summary) — fall through.
    }
  }
  const key = text.match(/"summary"\s*:\s*/);
  if (!key || key.index == null) return null;
  const start = key.index + key[0].length;
  if (text[start] !== '"') return null;
  const read = readJsonString(text, start);
  const value = read?.value.trim();
  return value || null;
}

function looksLikeEnvelope(text: string): boolean {
  const t = text.trim();
  return t.startsWith('{') && ENVELOPE_HINT.test(t);
}

function tidyWhitespace(text: string): string {
  return text
    .replace(/^\uFEFF/, '')
    .replace(/\r\n/g, '\n')
    .replace(/[ \t]+\n/g, '\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

/**
 * User-visible Coach text. Strips leaked JSON envelopes, unescapes `\n`,
 * and never returns leftover copilot JSON.
 */
export function formatCoachSummary(raw: unknown, fallback = ''): string {
  let text = tidyWhitespace(asText(raw));
  if (!text) return fallback;

  for (let i = 0; i < 3; i += 1) {
    const unfenced = unwrapFence(text);
    const extracted = extractEnvelopeSummary(unfenced);
    if (extracted != null) {
      text = tidyWhitespace(extracted);
      continue;
    }
    text = tidyWhitespace(unfenced);
    break;
  }

  if (!text.includes('\n') && /\\n/.test(text)) {
    text = tidyWhitespace(unescapeJsonish(text));
  }

  if (looksLikeEnvelope(text)) {
    const again = extractEnvelopeSummary(text);
    text = tidyWhitespace(again ?? '');
  }

  return text || fallback;
}

/** Clarifying questions / warnings: same cleanup, empty stays empty. */
export function formatCoachNote(raw: unknown): string {
  return formatCoachSummary(raw, '');
}

/**
 * Markdown collapses single newlines. Add hard breaks so tape lines stay stacked
 * without breaking lists, tables, or headings.
 */
export function applyCoachHardBreaks(md: string): string {
  const lines = md.split('\n');
  return lines
    .map((line, i) => {
      const next = lines[i + 1];
      if (next == null) return line;
      if (!line.trim() || !next.trim()) return line;
      if (isListOrTable(line) || isListOrTable(next)) return line;
      if (/^#{1,6}\s/.test(line) || /^#{1,6}\s/.test(next)) return line;
      if (/^```/.test(line) || /^```/.test(next)) return line;
      if (line.endsWith('  ') || line.endsWith('\\')) return line;
      return `${line}  `;
    })
    .join('\n');
}

function isListOrTable(line: string): boolean {
  return /^\s*(?:[-*+]|\d+\.)\s/.test(line) || /^\s*\|/.test(line) || /^\s*[-*|:]{3,}\s*$/.test(line);
}
