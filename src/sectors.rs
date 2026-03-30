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
