export function formatNaira(n: number | null | undefined): string {
  const value = Number(n);
  return `₦${(Number.isFinite(value) ? value : 0).toLocaleString('en-NG', {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  })}`;
}

/** Matches sandbox fill accounting: BUY spends notional+fee; SELL recovers notional−fee. */
export function tradeCashImpact(
  side: string,
  quantity: number,
  fillPrice: number,
  fee: number,
): { label: 'Spent' | 'Proceeds'; amount: number } {
  const notional = Number(quantity) * Number(fillPrice);
  const feeAmt = Number(fee);
  const safeNotional = Number.isFinite(notional) ? notional : 0;
  const safeFee = Number.isFinite(feeAmt) ? feeAmt : 0;
  if (String(side).toUpperCase() === 'BUY') {
    return { label: 'Spent', amount: safeNotional + safeFee };
  }
  return { label: 'Proceeds', amount: Math.max(0, safeNotional - safeFee) };
}

