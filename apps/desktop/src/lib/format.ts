export type MarketModule = 'stocks' | 'crypto';

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
