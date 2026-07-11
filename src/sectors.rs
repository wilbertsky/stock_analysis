/// Returns the curated list of large-cap ticker symbols for a given sector.
/// Accepts common aliases (e.g. "tech", "health", "finance").
pub fn tickers_for_sector(sector: &str) -> Option<&'static [&'static str]> {
    match sector.to_lowercase().replace(' ', "-").as_str() {
        "technology" | "tech" => Some(&[
            "AAPL", "MSFT", "NVDA", "GOOGL", "AVGO", "ORCL", "ADBE", "CRM", "AMD", "QCOM",
        ]),
        "healthcare" | "health" => Some(&[
            "JNJ", "UNH", "LLY", "ABT", "MRK", "TMO", "ABBV", "DHR", "BMY", "AMGN",
        ]),
        "financials" | "finance" | "financial-services" => Some(&[
            "JPM", "V", "MA", "BAC", "WFC", "GS", "MS", "AXP", "BLK", "SCHW",
        ]),
        "energy" => Some(&[
            "XOM", "CVX", "SLB", "COP", "EOG", "MPC", "VLO", "PSX", "OXY", "BKR",
        ]),
        "consumer-staples" | "staples" | "consumer-defensive" => Some(&[
            "PG", "KO", "PEP", "WMT", "COST", "PM", "MO", "MDLZ", "CL", "GIS",
        ]),
        "consumer-discretionary" | "discretionary" | "consumer-cyclical" => Some(&[
            "AMZN", "TSLA", "HD", "MCD", "NKE", "SBUX", "LOW", "TJX", "BKNG", "CMG",
        ]),
        "industrials" => Some(&[
            "RTX", "HON", "UPS", "CAT", "DE", "LMT", "GE", "BA", "MMM", "EMR",
        ]),
        "materials" | "basic-materials" => Some(&[
            "LIN", "APD", "ECL", "SHW", "FCX", "NEM", "DD", "PPG", "ALB", "CF",
        ]),
        "real-estate" | "realestate" => Some(&[
            "PLD", "AMT", "EQIX", "CCI", "PSA", "WELL", "DLR", "O", "SPG", "AVB",
        ]),
        "communication" | "communication-services" | "telecom" => Some(&[
            "NFLX", "DIS", "CMCSA", "VZ", "T", "TMUS", "WBD", "EA", "OMC", "IPG",
        ]),
        "utilities" => Some(&[
            "NEE", "DUK", "SO", "D", "AEP", "EXC", "SRE", "XEL", "ED", "WEC",
        ]),
        _ => None,
    }
}

/// All supported sector slugs for documentation.
pub const SUPPORTED_SECTORS: &str =
    "technology, healthcare, financials, energy, consumer-staples, \
     consumer-discretionary, industrials, materials, real-estate, \
     communication, utilities";

/// Maps a sector slug to FMP's own `/stable/company-screener` sector taxonomy.
/// Confirmed live against the company-screener endpoint: FMP returns exactly these
/// eleven sector strings (Basic Materials, Communication Services, Consumer Cyclical,
/// Consumer Defensive, Energy, Financial Services, Healthcare, Industrials, Real Estate,
/// Technology, Utilities) — this is the inverse of that taxonomy, normalized to our slugs.
/// Used by both `sp500.rs` (large-cap universe) and `routes/discovery.rs` (small/mid-cap
/// universe) so both sides of the FMP screener integration share one mapping.
pub fn slug_to_fmp_sector(slug: &str) -> Option<&'static str> {
    match slug.to_lowercase().replace(' ', "-").as_str() {
        "technology" | "tech" => Some("Technology"),
        "healthcare" | "health" => Some("Healthcare"),
        "financials" | "finance" | "financial-services" => Some("Financial Services"),
        "energy" => Some("Energy"),
        "consumer-staples" | "staples" | "consumer-defensive" => Some("Consumer Defensive"),
        "consumer-discretionary" | "discretionary" | "consumer-cyclical" => Some("Consumer Cyclical"),
        "industrials" => Some("Industrials"),
        "materials" | "basic-materials" => Some("Basic Materials"),
        "real-estate" | "realestate" => Some("Real Estate"),
        "communication" | "communication-services" | "telecom" => Some("Communication Services"),
        "utilities" => Some("Utilities"),
        _ => None,
    }
}

/// All eleven sector slugs in canonical form, for iterating when building a full
/// large-cap universe across every sector (see `sp500.rs::load`).
pub const ALL_SECTOR_SLUGS: &[&str] = &[
    "technology", "healthcare", "financials", "energy", "consumer-staples",
    "consumer-discretionary", "industrials", "materials", "real-estate",
    "communication", "utilities",
];

/// Supported exchange slugs. "us" is the default (NYSE/NASDAQ via country=US filter).
/// "lse" maps to FMP's `exchange=LSE` (London Stock Exchange).
pub const SUPPORTED_EXCHANGES: &str = "us, lse";

/// All exchange slugs for background refresh iteration.
pub const ALL_EXCHANGE_SLUGS: &[&str] = &["us", "lse"];

/// Returns `true` if the exchange slug is supported.
pub fn is_valid_exchange(exchange: &str) -> bool {
    matches!(exchange, "us" | "lse")
}

/// Returns the ISO 4217 currency code for prices/financials on a given exchange.
/// US equities are denominated in USD; LSE in GBP.
pub fn exchange_currency(exchange: &str) -> &'static str {
    match exchange {
        "lse" => "GBP",
        _ => "USD",
    }
}

/// Returns the FMP `exchange=` parameter value for non-US exchanges, or `None` for
/// US (which uses `country=US` instead of an exchange filter).
pub fn exchange_fmp_code(exchange: &str) -> Option<&'static str> {
    match exchange {
        "lse" => Some("LSE"),
        _ => None,
    }
}

/// Human-readable exchange label for display and logging.
pub fn exchange_label(exchange: &str) -> &'static str {
    match exchange {
        "lse" => "London Stock Exchange (LSE)",
        _ => "US (NYSE/NASDAQ)",
    }
}
