//! Crypto desk headlines from first-party RSS (no API key).
//!
//! Fail-open: a dead feed must not block a cycle or invent copy.
//! Headlines are titles only — already in the tape; not an entry signal.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};

const CACHE_TTL: Duration = Duration::from_secs(12 * 60);
const MAX_HEADLINES: usize = 12;
const MAX_SYMBOL_HITS: usize = 8;
const USER_AGENT: &str = "PulsarAI/1.0 (+https://github.com/zheeno/pulsar-ai; rss)";

const FEEDS: &[(&str, &str)] = &[
    ("CoinDesk", "https://www.coindesk.com/arc/outboundfeeds/rss/"),
    ("Decrypt", "https://decrypt.co/feed"),
    ("The Block", "https://www.theblock.co/rss.xml"),
];

const CRYPTO_MARKERS: &[&str] = &[
    "BITCOIN",
    "BTC",
    "ETHEREUM",
    "ETHER",
    "ETH ",
    " ETH",
    "CRYPTO",
    "STABLECOIN",
    "DEFI",
    "BLOCKCHAIN",
    "TOKEN",
    "ALTCOIN",
    "MEMECOIN",
    "ETF",
    "SEC ",
    "SEC,",
    "CFTC",
    "BINANCE",
    "COINBASE",
    "SOLANA",
    "XRP",
    "DOGE",
    "TETHER",
    "USDT",
    "USDC",
];

const ALIASES: &[(&str, &str)] = &[
    ("BITCOIN", "BTC"),
    ("ETHEREUM", "ETH"),
    ("ETHER", "ETH"),
    ("SOLANA", "SOL"),
    ("RIPPLE", "XRP"),
    ("DOGECOIN", "DOGE"),
    ("CARDANO", "ADA"),
    ("POLKADOT", "DOT"),
    ("AVALANCHE", "AVAX"),
    ("CHAINLINK", "LINK"),
    ("LITECOIN", "LTC"),
    ("POLYGON", "POL"),
    ("MATIC", "POL"),
    ("SHIBA", "SHIB"),
    ("UNISWAP", "UNI"),
    ("STELLAR", "XLM"),
    ("TRON", "TRX"),
    ("ALGORAND", "ALGO"),
    ("ARBITRUM", "ARB"),
    ("OPTIMISM", "OP"),
    ("SUI", "SUI"),
    ("NEAR", "NEAR"),
    ("TETHER", "USDT"),
    ("WORLDCOIN", "WLD"),
];

#[derive(Debug, Clone, Serialize)]
pub struct Headline {
    pub source: String,
    pub title: String,
    pub url: String,
    pub published_at: Option<String>,
    pub symbols: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NewsBundle {
    pub ok: bool,
    pub unavailable: bool,
    pub status: String,
    pub headlines: Vec<Headline>,
    pub outlets: Vec<String>,
}

struct NewsCache {
    fetched_at: Instant,
    headlines: Vec<Headline>,
    outlets_ok: Vec<String>,
}

static CACHE: Mutex<Option<NewsCache>> = Mutex::new(None);

pub async fn fetch_crypto_desk(focus: &[String]) -> NewsBundle {
    match fetch_crypto_desk_inner(focus).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(target: "news", error = %e, "crypto RSS failed");
            NewsBundle {
                ok: false,
                unavailable: true,
                status: "unavailable".into(),
                headlines: vec![],
                outlets: vec![],
            }
        }
    }
}

pub async fn coach_crypto_news(symbol: Option<&str>, query: Option<&str>) -> Value {
    let mut focus = Vec::new();
    if let Some(s) = symbol.map(normalize_symbol).filter(|s| !s.is_empty()) {
        focus.push(s);
    }
    if let Some(q) = query {
        for token in q.split(|c: char| !c.is_ascii_alphanumeric()) {
            let s = normalize_symbol(token);
            if s.len() >= 2 && s.len() <= 10 {
                focus.push(s);
            }
        }
    }
    let bundle = fetch_crypto_desk(&focus).await;
    if bundle.unavailable {
        return json!({
            "ok": false,
            "unavailable": true,
            "error": "Crypto RSS desks could not be reached. No headlines were invented.",
            "headlines": [],
        });
    }
    let mut headlines = bundle.headlines;
    if let Some(q) = query.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let needle = q.to_ascii_uppercase();
        headlines.retain(|h| h.title.to_ascii_uppercase().contains(&needle));
    }
    json!({
        "ok": true,
        "unavailable": false,
        "source": "rss",
        "outlets": bundle.outlets,
        "asOf": Utc::now().to_rfc3339(),
        "status": bundle.status,
        "headlines": headlines,
    })
}

pub fn headlines_json(bundle: &NewsBundle) -> Value {
    json!({
        "status": bundle.status,
        "outlets": bundle.outlets,
        "headlines": bundle.headlines,
    })
}

async fn fetch_crypto_desk_inner(focus: &[String]) -> Result<NewsBundle> {
    if let Some(cached) = cached_raw() {
        return Ok(select_bundle(cached.0, cached.1, focus));
    }

    let client = crate::http_client::http_client()?;
    let mut joined: Vec<Headline> = Vec::new();
    let mut outlets_ok = Vec::new();
    let mut errors = 0usize;

    for (name, url) in FEEDS {
        match fetch_feed(&client, name, url).await {
            Ok(mut items) => {
                if !items.is_empty() {
                    outlets_ok.push((*name).to_string());
                    joined.append(&mut items);
                } else {
                    errors += 1;
                }
            }
            Err(e) => {
                errors += 1;
                tracing::warn!(target: "news", outlet = %name, error = %e, "feed failed");
            }
        }
    }

    joined.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    dedupe_headlines(&mut joined);
    store_cache(joined.clone(), outlets_ok.clone());

    let mut bundle = select_bundle(joined, outlets_ok, focus);
    if errors == FEEDS.len() && bundle.headlines.is_empty() {
        bundle.ok = false;
        bundle.unavailable = true;
        bundle.status = "unavailable".into();
    }
    Ok(bundle)
}

fn cached_raw() -> Option<(Vec<Headline>, Vec<String>)> {
    let guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.as_ref()?;
    if cache.fetched_at.elapsed() > CACHE_TTL {
        return None;
    }
    Some((cache.headlines.clone(), cache.outlets_ok.clone()))
}

fn store_cache(headlines: Vec<Headline>, outlets_ok: Vec<String>) {
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some(NewsCache {
        fetched_at: Instant::now(),
        headlines,
        outlets_ok,
    });
}

async fn fetch_feed(client: &reqwest::Client, source: &str, url: &str) -> Result<Vec<Headline>> {
    let res = client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(
            reqwest::header::ACCEPT,
            "application/rss+xml, application/xml, text/xml, */*",
        )
        .send()
        .await?;
    if !res.status().is_success() {
        anyhow::bail!("{} HTTP {}", source, res.status());
    }
    let body = res.text().await?;
    Ok(parse_rss_items(source, &body))
}

fn select_bundle(all: Vec<Headline>, outlets: Vec<String>, focus: &[String]) -> NewsBundle {
    let focus: HashSet<String> = focus.iter().map(|s| normalize_symbol(s)).filter(|s| !s.is_empty()).collect();
    let selected = select_headlines(&all, &focus);
    let status = if selected.is_empty() {
        if all.is_empty() {
            "empty"
        } else {
            "no-match"
        }
    } else {
        "ok"
    };
    NewsBundle {
        ok: true,
        unavailable: false,
        status: status.into(),
        headlines: selected,
        outlets,
    }
}

fn select_headlines(all: &[Headline], focus: &HashSet<String>) -> Vec<Headline> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let push = |out: &mut Vec<Headline>, seen: &mut HashSet<String>, h: &Headline| {
        if out.len() >= MAX_HEADLINES {
            return;
        }
        let key = h.title.to_ascii_uppercase();
        if !seen.insert(key) {
            return;
        }
        out.push(h.clone());
    };

    for h in all {
        if out.len() >= MAX_SYMBOL_HITS {
            break;
        }
        if h.symbols.iter().any(|s| focus.contains(s)) {
            push(&mut out, &mut seen, h);
        }
    }
    for h in all {
        if out.len() >= MAX_HEADLINES {
            break;
        }
        if is_crypto_relevant(&h.title) {
            push(&mut out, &mut seen, h);
        }
    }
    out
}

fn dedupe_headlines(items: &mut Vec<Headline>) {
    let mut seen = HashSet::new();
    items.retain(|h| seen.insert(h.title.to_ascii_uppercase()));
}

pub fn parse_rss_items(source: &str, xml: &str) -> Vec<Headline> {
    let mut out = Vec::new();
    let lower = xml.to_ascii_lowercase();
    let mut search_from = 0usize;
    while let Some(rel) = lower[search_from..].find("<item") {
        let start = search_from + rel;
        let Some(gt) = xml[start..].find('>') else {
            break;
        };
        let body_start = start + gt + 1;
        let Some(end_rel) = lower[body_start..].find("</item>") else {
            break;
        };
        let block = &xml[body_start..body_start + end_rel];
        search_from = body_start + end_rel + 7;
        let title = match tag_text(block, "title") {
            Some(t) if !t.is_empty() => t,
            _ => continue,
        };
        if !is_crypto_relevant(&title) {
            continue;
        }
        let url = tag_text(block, "link").unwrap_or_default();
        let published_at = tag_text(block, "pubDate")
            .and_then(|raw| parse_pubdate(&raw))
            .map(|dt| dt.to_rfc3339());
        let symbols = symbols_in_title(&title);
        out.push(Headline {
            source: source.to_string(),
            title,
            url,
            published_at,
            symbols,
        });
        if out.len() >= 40 {
            break;
        }
    }
    out
}

pub fn symbols_in_title(title: &str) -> Vec<String> {
    let upper = title.to_ascii_uppercase();
    let mut found = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (alias, sym) in ALIASES {
        if upper.contains(alias) && seen.insert((*sym).to_string()) {
            found.push((*sym).to_string());
        }
    }
    for cap in regex_ticker_candidates(&upper) {
        if seen.insert(cap.clone()) {
            found.push(cap);
        }
    }
    found
}

fn regex_ticker_candidates(upper: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, ch) in upper.char_indices() {
        if ch != '$' && ch != '(' {
            continue;
        }
        let rest = &upper[i + ch.len_utf8()..];
        let tick: String = rest
            .chars()
            .take_while(|c| c.is_ascii_uppercase() || *c == '-')
            .collect();
        if (2..=10).contains(&tick.len()) {
            if ch == '(' && !rest[tick.len()..].starts_with(')') {
                continue;
            }
            out.push(tick);
        }
    }
    out
}

fn is_crypto_relevant(title: &str) -> bool {
    let t = format!(" {} ", title.to_ascii_uppercase());
    CRYPTO_MARKERS.iter().any(|m| t.contains(m)) || ALIASES.iter().any(|(a, _)| t.contains(a))
}

fn normalize_symbol(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('$')
        .trim_end_matches("NGN")
        .to_ascii_uppercase()
}

fn tag_text(block: &str, name: &str) -> Option<String> {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let lower = block.to_ascii_lowercase();
    let open_l = open.to_ascii_lowercase();
    let close_l = close.to_ascii_lowercase();
    let start = lower.find(&open_l)?;
    let after_name = start + open.len();
    let gt = block[after_name..].find('>')?;
    let inner_start = after_name + gt + 1;
    let end = lower[inner_start..].find(&close_l)?;
    Some(clean_text(&block[inner_start..inner_start + end]))
}

fn clean_text(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    if let Some(inner) = s
        .strip_prefix("<![CDATA[")
        .and_then(|r| r.strip_suffix("]]>"))
    {
        s = inner.to_string();
    }
    decode_entities(&s)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
}

fn parse_pubdate(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(raw)
        .ok()
        .map(|d| d.with_timezone(&Utc))
        .or_else(|| DateTime::parse_from_rfc3339(raw).ok().map(|d| d.with_timezone(&Utc)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
    <rss><channel>
      <item>
        <title><![CDATA[Bitcoin and ether ETFs draw $2.6 billion]]></title>
        <link>https://www.theblock.co/news/btc-etf</link>
        <pubDate>Sat, 22 Aug 2026 19:31:35 +0000</pubDate>
      </item>
      <item>
        <title>Microsoft Fixes Perfect 10 Exploit</title>
        <link>https://decrypt.co/microsoft</link>
        <pubDate>Sat, 22 Aug 2026 17:31:03 +0000</pubDate>
      </item>
      <item>
        <title>Solana (SOL) validators halt after client bug</title>
        <link>https://www.coindesk.com/sol</link>
        <pubDate>Sat, 22 Aug 2026 16:00:00 +0000</pubDate>
      </item>
    </channel></rss>
    "#;

    #[test]
    fn parse_skips_non_crypto_and_extracts_symbols() {
        let items = parse_rss_items("The Block", SAMPLE);
        assert_eq!(items.len(), 2);
        assert!(items[0].title.contains("Bitcoin"));
        assert!(items[0].symbols.contains(&"BTC".into()));
        assert!(items[0].symbols.contains(&"ETH".into()));
        assert!(items.iter().any(|h| h.symbols.contains(&"SOL".into())));
        assert!(!items.iter().any(|h| h.title.contains("Microsoft")));
    }

    #[test]
    fn select_prefers_focus_symbols() {
        let items = parse_rss_items("Desk", SAMPLE);
        let focus = HashSet::from(["SOL".into()]);
        let picked = select_headlines(&items, &focus);
        assert_eq!(picked[0].symbols.contains(&"SOL".into()), true);
    }

    #[test]
    fn decode_entities_and_cdata() {
        let xml = r#"<rss><channel><item>
            <title>Ripple &amp; Stellar: the duopoly</title>
            <link>https://x.test/a</link>
            <pubDate>Sat, 22 Aug 2026 12:00:00 +0000</pubDate>
        </item></channel></rss>"#;
        let items = parse_rss_items("CoinDesk", xml);
        assert_eq!(items.len(), 1);
        assert!(items[0].title.contains('&'));
        assert!(items[0].symbols.contains(&"XRP".into()));
        assert!(items[0].symbols.contains(&"XLM".into()));
    }
}
