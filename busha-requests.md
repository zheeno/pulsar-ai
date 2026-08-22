# Busha API Integration Guide (Pulsar AI)

Reverse-engineered from traffic captured through the Auth Bridge module. This is Busha's **internal/private web API** — undocumented, unversioned in any public sense, and subject to change without notice. Treat every field/behavior here as "observed," not "guaranteed."

> **Security note:** this document is scrubbed of the live token and PII that were present in the raw capture. Do not commit the raw capture file to any repo — rotate/log out that session once you're done extracting what you need from it, since the token was captured in plaintext.

---

## 1. Hosts & Auth

Two separate backend hosts are in play — the Auth Bridge needs to be aware of both if `/me` data is needed:

| Host | Purpose |
|---|---|
| `api.busha.io` | Trading, balances, currencies, pairs, quotes, transfers, transactions |
| `api.ben.busha.team` | Identity/profile service (`/v3/me`) — note `sec-fetch-site: cross-site`, confirming it's a genuinely separate origin from `app.busha.io` |

**Auth scheme:** `Authorization: Bearer <JWT>` on every request. The JWT is a standard HS256 token; decoded payload includes `sub`/`uid` (user id), `email`, `name`, `role`, `scp` (scope, observed as `["*"]`), `lvl` (KYC level), device metadata, `iat`, `exp`.

- **Token lifetime observed: 30 minutes** (`exp - iat = 1800s` in the capture). Pulsar's session manager should treat ~30 min as the refresh/re-auth window, not something longer — this is short-lived by design.
- **Required non-standard header:** `x-bu-profile-id: <profile_id>` — required on `api.busha.io` calls that touch balances/trading. Equal to the user's `id`/`sub` in this account, but per Busha's own data model `profile_id` is a distinct field from `user_id` (they happened to match here — don't assume they always will, e.g. business/sub-profiles).
- Standard browser fingerprint headers are also sent (`sec-ch-ua*`, `accept-language`, `origin`, `referer`, `user-agent`) — worth replicating from the captured webview session rather than a generic Tauri UA, since anomalous headers on an undocumented API are one of the more common ways automated traffic gets flagged.

## 2. Endpoints

### `GET /v1/currencies`
List of all currencies (crypto + fiat) with market stats. Static-ish reference data — cache client-side, poll infrequently.

```json
{
  "status": "success",
  "data": [
    {
      "code": "AAVE",
      "name": "Aave",
      "display_name": "AAVE",
      "type": "crypto",
      "decimals": "8",
      "deposit": false,
      "withdrawal": false,
      "supported_networks": [],
      "description": "...",
      "stats": {
        "circulating_supply": { "amount": "...", "currency": "USD" },
        "market_cap": { "amount": "...", "currency": "USD" },
        "total_volume": { "amount": "...", "currency": "USD" }
      }
    }
  ]
}
```

### `GET /v1/currencies/{code}`
Single-currency detail — same shape as one item above. Note `deposit`/`withdrawal` were `false` across sampled currencies — this account/asset combo doesn't support on-chain movement, only in-platform conversion (see §3).

### `GET /v1/pairs?counter=NGN&sort_by=market_cap&sort=desc`
Tradable pairs against a counter currency, with live pricing. This is the market-data endpoint to poll for pricing the UI.

```json
{
  "data": [
    {
      "id": "BTCNGN",
      "base": "BTC",
      "counter": "NGN",
      "buy_price": { "amount": "93501695.36", "currency": "NGN" },
      "sell_price": { "amount": "88919802.32", "currency": "NGN" },
      "is_buy_supported": true,
      "is_sell_supported": true,
      "min_buy_amount": { "amount": "0.000005", "counter": {"amount":"467.51","currency":"NGN"}, "currency": "BTC" },
      "max_buy_amount": { "amount": "1", "counter": {"amount":"93501695.36","currency":"NGN"}, "currency": "BTC" },
      "min_sell_amount": { "...": "mirrors min_buy_amount shape" },
      "max_sell_amount": { "...": "mirrors max_buy_amount shape" },
      "percentage_change": "1.1"
    }
  ]
}
```
Note the visible spread: `buy_price` and `sell_price` differ (~5% in the sampled BTC/NGN pair) — Busha's "rate" isn't a single mid-market number, it's already spread-adjusted per side. Use `is_buy_supported`/`is_sell_supported` and the min/max fields to validate order size client-side before ever hitting `/quotes`.

### `GET /v1/currencies/ohlc/{PAIR}?period={PERIOD}`
Trend history for a tradable pair against NGN. `{PAIR}` is the pair id (e.g. `BTCNGN`, `ARKMNGN`). `{PERIOD}` is optional:

| `period` | Meaning (observed) |
|---|---|
| `1d` | Intraday series (~5-minute bars for the last day) |
| `1m` | ~1 month of history |
| `1y` | ~1 year of history |
| *(omit)* | All-time history |

```json
{
  "status": "success",
  "message": "OHLC fetched successfully",
  "data": {
    "symbol": "ARKMNGN",
    "change": "3.73",
    "high": "161.93",
    "low": "140.5",
    "market_cap": "0",
    "price": "147.49",
    "price_data": [
      { "time": "2026-08-21T10:40:00Z", "price": "142.12" }
    ]
  }
}
```

All numeric fields arrive as strings. Pulsar stores raw points in `busha_ohlc_points` (keyed by base symbol + period + timestamp), snapshot metadata in `busha_ohlc_meta`, and rolls daily closes into `price_history` for long-window indicators. Cache TTLs in-app: `1d` ≈ 5 min, `1m` ≈ 1 hr, `1y`/all-time ≈ 24 hr. Requires the same auth headers as other `api.busha.io` trading calls.

### `GET /v1/balances`
Per-currency balance breakdown (fiat and crypto), including `available`, `pending`, `savings`, `investments`, `total`, each as `{amount, currency}` plus a nested `fiat` conversion. Also carries `trade`/`deposit`/`withdrawal` booleans per asset — check `trade: true` before offering a currency in Pulsar's trade UI, several sampled assets had `trade: false`.

### `GET /v1/balances/overview`
Aggregated view: `crypto`, `fiat`, and `total` blocks, each with `available`, `balance`, `change_24h` (amount + percentage), `escrow`, `holding`, `investments`, `locked`, `pending`, `savings` — all normalized to the account's display currency (NGN here). This is the endpoint for a dashboard summary widget; `/v1/balances` is for the per-asset list.

### `GET https://api.ben.busha.team/v3/me`
Full account/KYC profile — name, DOB, address, phone, BVN, verification level, `available_to_trade`, `status`. **Do not cache or log this response beyond what Pulsar's UI actually needs to render** (e.g. `available_to_trade`, `level`, `currency_code`) — the rest is regulated PII (BVN especially) that has no business living in Pulsar's local storage or logs.

### `GET /v1/transactions?currency={code}`
Transaction history filtered by asset. Each entry has `status` (`completed` observed), `type` (`buys` observed — presumably `sells`/others exist), `amount`, `is_credit`, and a `meta.conversion` block mirroring the quote/transfer that created it. Useful for confirming a trade actually settled after `/transfers` returns `pending`.

## 3. Trade Flow (Buy or Sell)

Busha's trading model here is **instant balance-to-balance conversion**, not an order book — you get a fixed quote, then confirm it. Two calls, in sequence:

**Step 1 — `POST /v1/quotes`**
```json
// Buy ARKM with NGN
{ "source_currency": "NGN", "target_currency": "ARKM", "source_amount": "650" }

// Sell ARKM for NGN
{ "source_currency": "ARKM", "target_currency": "NGN", "source_amount": "2.1" }
```
Response includes `id` (`QUO_...`), the locked `rate` (`type: "FIXED"`), computed `target_amount`, and `expires_at`. **Observed quote lifetime: 30 minutes** (same as token TTL — plausibly not a coincidence, but treat them as independently expiring). Direction (`side: "buy"`/`"sell"`) is inferred server-side from which of `source_currency`/`target_currency` is the fiat leg — there's no explicit `side` param on the request.

**Step 2 — `POST /v1/transfers`**
```json
{ "quote_id": "QUO_PffLl5DprKTr" }
```
Response is a transfer object (`TRF_...`) echoing the quote's amounts/rate, with `status: "pending"` and a `timeline` block (`total_steps`, `current_step`, `events`) that presumably updates as the conversion settles — the capture doesn't show a polling call against a specific transfer, so **confirm completion via `GET /v1/transactions?currency={target}` and match on `reference` == the quote id**, as shown in the capture (transaction's `reference: "CNV_ZDymeHnRJ4VM"` ties back to the transfer's `id: "TRF_ZDymeHnRJ4VM"`, not the quote id — note the `CNV_`/`TRF_` prefix swap, worth defensive matching on both).

Implementation-wise for Pulsar: **quote is a preview, transfer is the trade** — never call `/transfers` without a fresh, unexpired `quote_id`, and treat a stale quote (past `expires_at`) as needing a new `/quotes` call rather than retrying the same id.

## 4. Gaps — not covered by this capture

The doc above only documents what was observed; before building against it, be aware the sample does **not** include:
- Error response shapes (4xx/5xx) — unknown what validation failures, expired-quote attempts, or insufficient-balance responses look like. Worth capturing a few deliberately before writing error handling.
- Deposit/withdrawal endpoints (all sampled currencies had `deposit`/`withdrawal: false`, so these paths never fired).
- Rate limiting behavior/headers.
- Pagination beyond a single page (`pagination.current_entries_size` appeared but no `next`/cursor field was exercised).
- Token refresh mechanism — nothing in the capture shows how the app gets a new JWT without a full re-login. Given the 30-minute TTL, Pulsar's Auth Bridge session manager should assume **re-auth via the webview**, not a refresh-token call, unless a refresh endpoint turns up in a longer capture.