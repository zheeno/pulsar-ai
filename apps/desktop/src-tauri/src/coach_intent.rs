//! Soft Coach intent hints. Tool gating is conversation-first: the LLM
//! chooses tools; the host only refuses irreversible writes.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CoachIntentClass {
    Meta,
    Greeting,
    Research,
    Account,
    Strategy,
    Trade,
    #[default]
    Other,
}

impl CoachIntentClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Meta => "meta",
            Self::Greeting => "greeting",
            Self::Research => "research",
            Self::Account => "account",
            Self::Strategy => "strategy",
            Self::Trade => "trade",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CoachIntent {
    pub class: CoachIntentClass,
    pub defer: bool,
    pub wants_sl_tp: bool,
    pub drawdown_pct: Option<f64>,
    pub profit_pct: Option<f64>,
    pub already_asked: bool,
    pub greeting: bool,
    pub research: bool,
}

impl CoachIntent {
    /// True when the *user message* asked for a strategy decision (not from class).
    pub fn should_decide(&self) -> bool {
        self.defer || self.wants_sl_tp || self.drawdown_pct.is_some()
    }
}

pub const PERSONA_META: &str = "I'm Coach, Pulsar's desk copilot for NGX equities and Busha crypto. I can check the tape, a name's history, crypto headlines from CoinDesk / Decrypt / The Block RSS, help you tighten risk sliders, and propose trades. NGX news is not wired — I won't invent it. I won't silently place live orders or invent prices. What do you want to look at?";
pub const PERSONA_GREETING: &str = "Hey. Tape, a ticker, news, risk sliders, or a trade idea — your call.";
pub const PERSONA_THANKS: &str = "Anytime. Ping me if you want the tape, a name, or a trade idea.";
pub const PERSONA_BYE: &str = "See you. I'll be here when you want the next look.";

pub fn canned_social_reply(intent: &CoachIntent, message: &str) -> String {
    let t = message.trim().to_lowercase();
    match intent.class {
        CoachIntentClass::Meta => PERSONA_META.into(),
        CoachIntentClass::Greeting if t.contains("thank") || t.contains("cheers") || t == "ty" => {
            PERSONA_THANKS.into()
        }
        CoachIntentClass::Greeting
            if t.contains("bye") || t.contains("later") || t.contains("see ya") || t.contains("see you") =>
        {
            PERSONA_BYE.into()
        }
        _ => PERSONA_GREETING.into(),
    }
}

/// Read + propose tools the agent may call. `execute_trade` / `apply_strategy_patch`
/// exist in the catalog but the host always refuses them.
pub const COACH_AGENT_TOOLS: &[&str] = &[
    "get_account_snapshot",
    "get_holdings",
    "get_recent_trades",
    "get_strategy_params",
    "list_universe_quotes",
    "get_symbol_quote",
    "get_price_history",
    "get_indicators",
    "search_memory",
    "memory_search",
    "get_news",
    "run_symbol_screen",
    "explain_blocked_reason",
    "get_cycle_status",
    "get_trade_lessons",
    "get_dream_rules",
    "get_last_cycle",
    "get_confidence_journal",
    "get_desk_advice",
    "propose_strategy_patch",
    "propose_trade",
];

pub fn is_irreversible_coach_tool(name: &str) -> bool {
    matches!(name, "execute_trade" | "apply_strategy_patch")
}

pub fn allowed_tools_for_intent(_class: CoachIntentClass) -> &'static [&'static str] {
    COACH_AGENT_TOOLS
}

pub fn tool_allowed_for_intent(_class: &str, name: &str) -> bool {
    !is_irreversible_coach_tool(name)
}

/// Book is never pre-injected; the model fetches via tools when needed.
pub fn attaches_book(_class: CoachIntentClass) -> bool {
    false
}

fn looks_like_meta(t: &str) -> bool {
    [
        "tell me about yourself",
        "tell me about you",
        "about yourself",
        "who are you",
        "who're you",
        "who are u",
        "what are you",
        "what can you do",
        "what do you do",
        "how do you work",
        "how does this work",
        "how does coach work",
        "your capabilities",
        "introduce yourself",
        "what is coach",
        "what's your purpose",
        "whats your purpose",
        "what is your purpose",
    ]
    .iter()
    .any(|p| t.contains(p))
}

fn looks_like_greeting(message: &str) -> bool {
    let t = message
        .trim()
        .trim_matches(|c: char| matches!(c, '!' | '.' | ','))
        .trim()
        .to_lowercase();
    matches!(
        t.as_str(),
        "hi" | "hey"
            | "hello"
            | "yo"
            | "sup"
            | "hiya"
            | "howdy"
            | "gm"
            | "good morning"
            | "good afternoon"
            | "good evening"
            | "thanks"
            | "thank you"
            | "cheers"
            | "ty"
            | "bye"
            | "goodbye"
            | "good bye"
            | "later"
            | "see ya"
            | "see you"
            | "ok thanks"
            | "okay thanks"
    ) || ((t.starts_with("hi ") || t.starts_with("hey ") || t.starts_with("hello ") || t.starts_with("yo "))
        && t.len() <= 24)
        || ((t.starts_with("thanks") || t.starts_with("thank you")) && t.len() <= 40)
}

fn looks_like_advisory(t: &str) -> bool {
    t.contains("advisable")
        || t.contains("should i")
        || t.contains("would you buy")
        || t.contains("would you sell")
        || t.contains("worth buying")
        || t.contains("thoughts on")
        || t.contains("good buy")
        || t.contains("good sell")
}

fn looks_like_trade(t: &str) -> bool {
    t.contains("sell half")
        || t.contains("buy half")
        || t.contains("buy my")
        || t.contains("sell my")
        || t.contains("i want to buy")
        || t.contains("i want to sell")
        || t.contains("place an order")
        || t.contains("place the order")
        || t.contains("propose a trade")
        || t.contains("propose trade")
        || t.contains("execute the trade")
        || t.contains("execute this trade")
        || buy_sell_qty(t)
}

fn buy_sell_qty(t: &str) -> bool {
    for (i, _) in t.char_indices() {
        let rest = &t[i..];
        if rest.starts_with("buy ") || rest.starts_with("sell ") {
            let after = if rest.starts_with("buy ") { 4 } else { 5 };
            if rest
                .get(after..)
                .and_then(|s| s.chars().next())
                .is_some_and(|c| c.is_ascii_digit())
            {
                return true;
            }
        }
    }
    false
}

fn looks_like_strategy_text(t: &str) -> bool {
    [
        "tighten",
        "loosen",
        "stop loss",
        "stop-loss",
        "take profit",
        "take-profit",
        "drawdown",
        "slider",
        "cycle budget",
        "min confidence",
        "risk settings",
        "you decide",
        "your call",
        "you choose",
        "you pick",
        "you should decide",
        "tighten risk",
        "appropriate response",
    ]
    .iter()
    .any(|k| t.contains(k))
}

fn looks_like_account_text(t: &str) -> bool {
    [
        "cash",
        "balance",
        "holdings",
        "holding",
        "positions",
        "position",
        "portfolio",
        "pnl",
        "p&l",
        "my book",
        "how much do i",
        "how much have i",
        "what do i hold",
        "open lots",
    ]
    .iter()
    .any(|k| t.contains(k))
}

fn looks_like_research_text(t: &str, original: &str) -> bool {
    let keys = [
        "price", "quote", "moving", "movers", "gainer", "loser", "news", "headline", "history",
        "how is", "how's", "rsi", "sma", "indicator", "chart", "tape", "universe", "advisable",
        "should i buy", "should i sell", "worth buying", "thoughts on",
    ];
    if keys.iter().any(|k| t.contains(k)) || looks_like_advisory(t) {
        return true;
    }
    message_has_ticker(original)
}

fn message_has_ticker(message: &str) -> bool {
    for tok in message.split(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-') {
        if tok.is_empty() {
            continue;
        }
        let u = tok.to_ascii_uppercase();
        if u.len() >= 3 && u.len() <= 16 && crate::ngx::is_valid_ticker(&u) {
            let original_caps = tok.chars().any(|c| c.is_ascii_uppercase());
            if original_caps || u.len() >= 4 {
                return true;
            }
        }
    }
    false
}

fn looks_like_change_request(message: &str) -> bool {
    [
        "tighten",
        "loosen",
        "increase",
        "decrease",
        "set ",
        "change",
        "adjust",
        "update",
        "you decide",
        "your call",
        "you choose",
        "you pick",
        "you should decide",
        "appropriate response",
    ]
    .iter()
    .any(|k| message.contains(k))
}

pub fn classify_coach_intent(message: &str) -> CoachIntentClass {
    extract_coach_intent(message, &[]).class
}

pub fn extract_coach_intent(message: &str, history: &[(String, String)]) -> CoachIntent {
    let current = message.to_lowercase();
    let class = if looks_like_meta(&current) {
        CoachIntentClass::Meta
    } else if looks_like_greeting(message) {
        CoachIntentClass::Greeting
    } else if looks_like_trade(&current) && !looks_like_advisory(&current) {
        CoachIntentClass::Trade
    } else if looks_like_strategy_text(&current) {
        CoachIntentClass::Strategy
    } else if looks_like_account_text(&current) {
        CoachIntentClass::Account
    } else if looks_like_research_text(&current, message) {
        CoachIntentClass::Research
    } else {
        CoachIntentClass::Other
    };

    let defer = [
        "you decide",
        "your call",
        "you should decide",
        "appropriate response",
        "i don't have a strategy",
        "i dont have a strategy",
        "where you come in",
        "that's on you",
        "thats on you",
        "you choose",
        "you pick",
    ]
    .iter()
    .any(|p| current.contains(p));
    let mentions_sl_tp = current.contains("stop loss")
        || current.contains("stop-loss")
        || current.contains("take profit")
        || current.contains("take-profit");
    let wants_sl_tp = mentions_sl_tp && (looks_like_change_request(&current) || defer);
    let already_asked = history.iter().any(|(role, content)| {
        role.eq_ignore_ascii_case("assistant")
            && (content.contains('?') || content.to_lowercase().contains("need more"))
    });
    let skip_pct = matches!(
        class,
        CoachIntentClass::Meta | CoachIntentClass::Greeting | CoachIntentClass::Research
    );
    CoachIntent {
        class,
        defer,
        wants_sl_tp,
        drawdown_pct: if skip_pct {
            None
        } else {
            parse_pct_near(
                &current,
                &["drop", "drawdown", "tolerate", "tolerance", "loss", "dd"],
            )
        },
        profit_pct: if skip_pct {
            None
        } else {
            parse_pct_near(&current, &["profit", "gain", "target", "return"])
        },
        already_asked,
        greeting: class == CoachIntentClass::Greeting,
        research: class == CoachIntentClass::Research,
    }
}

fn parse_pct_near(blob: &str, keywords: &[&str]) -> Option<f64> {
    let bytes = blob.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            let Some(num) = blob.get(start..i).and_then(|s| s.parse::<f64>().ok()) else {
                continue;
            };
            let rest_trim = blob.get(i..).unwrap_or("").trim_start();
            let is_pct = rest_trim.starts_with('%')
                || rest_trim.starts_with("percent")
                || rest_trim.starts_with("per cent");
            if is_pct && num > 0.0 && num <= 100.0 {
                let window_start = start.saturating_sub(48);
                let window_end = (i + 24).min(blob.len());
                let window = blob.get(window_start..window_end).unwrap_or("");
                if keywords.iter().any(|k| window.contains(k)) {
                    return Some(num / 100.0);
                }
            }
            continue;
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_required_fixtures() {
        assert_eq!(
            classify_coach_intent("tell me about yourself"),
            CoachIntentClass::Meta
        );
        assert_eq!(classify_coach_intent("hi"), CoachIntentClass::Greeting);
        assert_eq!(classify_coach_intent("thanks"), CoachIntentClass::Greeting);
        assert_eq!(
            classify_coach_intent("is it advisable to buy GTCO?"),
            CoachIntentClass::Research
        );
        assert_eq!(
            classify_coach_intent("tighten risk"),
            CoachIntentClass::Strategy
        );
        assert_eq!(
            classify_coach_intent("sell half my MTNN"),
            CoachIntentClass::Trade
        );
        assert_eq!(
            classify_coach_intent("tell me about GTCO"),
            CoachIntentClass::Research
        );
    }

    #[test]
    fn agent_loop_refuses_only_irreversible_writes() {
        assert!(tool_allowed_for_intent("meta", "get_symbol_quote"));
        assert!(tool_allowed_for_intent("greeting", "get_account_snapshot"));
        assert!(tool_allowed_for_intent("research", "propose_trade"));
        assert!(allowed_tools_for_intent(CoachIntentClass::Meta).contains(&"get_symbol_quote"));
        assert!(!tool_allowed_for_intent("trade", "execute_trade"));
        assert!(!tool_allowed_for_intent("strategy", "apply_strategy_patch"));
        assert!(is_irreversible_coach_tool("execute_trade"));
        assert!(is_irreversible_coach_tool("apply_strategy_patch"));
    }

    #[test]
    fn gtco_then_meta_is_persona_not_market() {
        let t1 = extract_coach_intent("is it advisable to buy GTCO?", &[]);
        assert_eq!(t1.class, CoachIntentClass::Research);
        assert!(tool_allowed_for_intent("research", "get_symbol_quote"));

        let history = vec![
            ("user".into(), "is it advisable to buy GTCO?".into()),
            (
                "assistant".into(),
                "GTCO last ₦46.20 as-of today; cash is ₦5485.".into(),
            ),
        ];
        let t2 = extract_coach_intent("tell me about yourself", &history);
        assert_eq!(t2.class, CoachIntentClass::Meta);
        assert!(!attaches_book(t2.class));
        let reply = canned_social_reply(&t2, "tell me about yourself");
        let lower = reply.to_lowercase();
        assert!(lower.contains("copilot") || lower.contains("pulsar"));
        assert!(!lower.contains("gtco"));
        assert!(!lower.contains("46.20"));
        assert!(!reply.contains('₦'));
        assert!(!lower.contains("5485"));
    }

    #[test]
    fn book_never_preattached() {
        assert!(!attaches_book(CoachIntentClass::Strategy));
        assert!(!attaches_book(CoachIntentClass::Account));
        assert!(!attaches_book(CoachIntentClass::Trade));
        assert!(!attaches_book(CoachIntentClass::Research));
    }

    #[test]
    fn curly_apostrophe_does_not_panic_intent() {
        let msg = "I thought that\u{2019}s what you\u{2019}re here for?";
        let intent = extract_coach_intent(msg, &[]);
        assert!(!intent.should_decide());
        assert_eq!(
            classify_coach_intent("buy 100 GTCO"),
            CoachIntentClass::Trade
        );
    }

    #[test]
    fn persona_names_busha_and_coindesk_not_unwired_news() {
        let reply = canned_social_reply(
            &extract_coach_intent("who are you", &[]),
            "who are you",
        );
        assert!(reply.contains("Busha"));
        assert!(reply.contains("CoinDesk"));
        assert!(reply.contains("NGX"));
        assert!(!reply.to_lowercase().contains("news when it's wired"));
        assert!(!reply.to_lowercase().contains("news when it is wired"));
        assert!(COACH_AGENT_TOOLS.contains(&"get_trade_lessons"));
        assert!(COACH_AGENT_TOOLS.contains(&"get_dream_rules"));
        assert!(COACH_AGENT_TOOLS.contains(&"get_last_cycle"));
        assert!(COACH_AGENT_TOOLS.contains(&"get_confidence_journal"));
        assert!(COACH_AGENT_TOOLS.contains(&"get_desk_advice"));
    }

    #[test]
    fn soft_and_offtopic_are_not_strategy_decisions() {
        for msg in [
            "really?",
            "tell me about donald trump",
            "but you are unable to hold a real conversation",
            "hello",
        ] {
            let intent = extract_coach_intent(msg, &[]);
            assert!(
                !intent.should_decide(),
                "{msg} should not force a slider patch"
            );
            assert!(!attaches_book(intent.class));
        }
    }
}
