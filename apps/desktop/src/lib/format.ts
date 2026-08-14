export function formatNaira(n: number | null | undefined): string {
  const value = Number(n);
  return `₦${(Number.isFinite(value) ? value : 0).toLocaleString('en-NG', {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  })}`;
}
