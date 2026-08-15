export type MarketModule = 'stocks' | 'crypto';

/** Strip a USDT/USD/BUSD quote suffix from a Binance-style pair ticker. */
export function cryptoBaseAsset(pair: string | null | undefined): string {
  const s = String(pair || '').trim().toUpperCase();
  if (!s) return '';
  for (const quote of ['USDT', 'BUSD', 'USD', 'USDC']) {
    if (s.length > quote.length && s.endsWith(quote)) {
      return s.slice(0, -quote.length);
    }
  }
  return s;
}

export function formatNaira(n: number | null | undefined): string {
  const value = Number(n);
  return `₦${(Number.isFinite(value) ? value : 0).toLocaleString('en-NG', {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  })}`;
}

export function formatUsdt(n: number | null | undefined): string {
  const value = Number(n);
  const abs = Math.abs(Number.isFinite(value) ? value : 0);
  const maxFrac = abs >= 100 ? 2 : abs >= 1 ? 4 : 8;
  return `${(Number.isFinite(value) ? value : 0).toLocaleString('en-US', {
    minimumFractionDigits: 2,
    maximumFractionDigits: maxFrac,
  })} USDT`;
}

export function formatMoney(n: number | null | undefined, module: string | undefined): string {
  return module === 'crypto' ? formatUsdt(n) : formatNaira(n);
}

/** Format a crypto position/trade quantity in base-asset units (e.g. "61.453 AVAX"). */
export function formatCryptoQty(
  qty: number | null | undefined,
  pairOrBase: string | null | undefined,
): string {
  const value = Number(qty);
  const n = Number.isFinite(value) ? value : 0;
  const abs = Math.abs(n);
  const maxFrac = abs >= 1000 ? 2 : abs >= 1 ? 4 : 8;
  const base = cryptoBaseAsset(pairOrBase) || 'COIN';
  return `${n.toLocaleString('en-US', {
    minimumFractionDigits: 0,
    maximumFractionDigits: maxFrac,
  })} ${base}`;
}
