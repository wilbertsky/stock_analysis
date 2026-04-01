# Stock Analysis API

A REST API built in Rust with [Axum](https://github.com/tokio-rs/axum) that provides fundamental stock analysis using value investing frameworks. Data is sourced from [Financial Modeling Prep (FMP)](https://financialmodelingprep.com/).

## Features

- **Core Fundamental Metrics** — Revenue, EPS, Book Value/Share, Free Cash Flow/Share, and ROIC over up to 5 years
- **Growth Rates** — Compound Annual Growth Rate (CAGR) for each fundamental metric
- **Intrinsic Value (DCF)** — Growth-adjusted discounted cash flow estimate with margin of safety (50% discount)
- **Graham Number** — Benjamin Graham's intrinsic value formula: √(22.5 × EPS × BVPS)
- **PEG Ratio** — Peter Lynch's growth-adjusted valuation ratio
- **Piotroski F-Score** — Nine-signal accounting quality score (0–9)
- **Dividend Metrics** — Yield, payout ratio, and sustainability assessment
- **Quality Score** — Composite business quality score from gross margin, ROE, and debt levels (0–100)
- **Momentum Score** — 3/6/12-month price returns relative to the S&P 500 (0–100)
- **Summary** — Fundamentals, valuations, and momentum in a single endpoint
- **Sector Screener** — Ranks large-cap stocks within a sector using a weighted composite of all four factor scores

Interactive API documentation is served via Swagger UI.

## Getting Started

### Prerequisites

- [Rust](https://rustup.rs/) (stable)
- A free API key from [Financial Modeling Prep](https://financialmodelingprep.com/)

### Setup

```bash
git clone <repo-url>
cd stock_analysis

cp .env.example .env
# Edit .env and set your FMP_API_KEY

cargo run
```

The server starts at `http://localhost:8080`.

### Swagger UI

Visit `http://localhost:8080/swagger-ui` to explore and test all endpoints interactively.

## API Endpoints

### Stock Analysis

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/health` | Service health check |
| `GET` | `/api/stock/{ticker}/fundamentals` | Core fundamental metrics, up to 5 years |
| `GET` | `/api/stock/{ticker}/growth-rates` | CAGR for each fundamental metric |
| `GET` | `/api/stock/{ticker}/intrinsic-value` | DCF intrinsic value estimate and margin of safety |
| `GET` | `/api/stock/{ticker}/graham-number` | Graham Number intrinsic value |
| `GET` | `/api/stock/{ticker}/peg` | PEG ratio |
| `GET` | `/api/stock/{ticker}/piotroski` | Piotroski F-Score with all 9 signals |
| `GET` | `/api/stock/{ticker}/dividends` | Dividend yield, payout ratio, and sustainability |
| `GET` | `/api/stock/{ticker}/quality` | Business quality score (0–100) |
| `GET` | `/api/stock/{ticker}/momentum` | Price momentum vs S&P 500 over 3/6/12 months |
| `GET` | `/api/stock/{ticker}/summary` | Complete analysis — all valuations and momentum |

### Sector Screener

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/screener/{sector}` | Ranked stock picks for a sector |

Supported sectors: `technology`, `healthcare`, `financials`, `energy`, `consumer-staples`, `consumer-discretionary`, `industrials`, `materials`, `real-estate`, `communication`, `utilities`

Each sector screens 10 representative large-cap stocks and ranks them by a weighted composite score. Expect 10–20 seconds response time as data is fetched concurrently.

### Examples

```bash
# Full analysis for a single stock
curl http://localhost:8080/api/stock/AAPL/summary

# DCF intrinsic value estimate
curl http://localhost:8080/api/stock/AAPL/intrinsic-value

# Piotroski F-Score
curl http://localhost:8080/api/stock/MSFT/piotroski

# Momentum vs S&P 500
curl http://localhost:8080/api/stock/NVDA/momentum

# Top-ranked technology stocks
curl http://localhost:8080/api/screener/technology
```

## Analysis Methods

### Intrinsic Value — Simplified DCF
Projects EPS 10 years forward using the historical EPS CAGR, applies a growth-adjusted P/E of 2× the growth rate percentage, then discounts back to today at a 15% minimum required rate of return. This approach is rooted in standard discounted cash flow theory as practiced by Benjamin Graham and Warren Buffett. The margin of safety price is 50% of the intrinsic value estimate — a concept introduced by Graham to account for uncertainty in any projection.

### Graham Number (Benjamin Graham)
Conservative intrinsic value estimate based purely on earnings and book value: `√(22.5 × EPS × BVPS)`. Works best for stable, asset-heavy companies.

### PEG Ratio (Peter Lynch)
Adjusts the P/E ratio for growth: `P/E ÷ EPS growth rate %`. Below 1.0 may indicate undervaluation relative to growth; below 0.5 was considered a bargain by Lynch.

### Piotroski F-Score (Joseph Piotroski)
Nine binary signals across three groups — profitability (F1–F4), leverage and liquidity (F5–F7), and operating efficiency (F8–F9). Scores ≥7 indicate a financially strong company; scores ≤2 indicate potential distress.

### Quality Score
Composite 0–100 score based on gross margin (pricing power), return on equity (capital efficiency), and debt-to-equity (financial risk). High-quality companies typically have wide margins, high ROE, and manageable debt — the combination most associated with durable competitive advantage.

### Momentum Score
Measures 3-month, 6-month, and 12-month price returns relative to the S&P 500 (SPY). Score starts at 50 (neutral) and shifts up for outperformance or down for underperformance across each period. Grounded in decades of academic research showing that recent outperformers tend to continue outperforming near-term.

### Sector Screener
Ranks stocks within a sector using a weighted composite of all four factor scores:
- **Piotroski F-Score** — 30%
- **Quality Score** — 25%
- **DCF Value Signal** — 25% (how current price compares to intrinsic value and margin of safety price)
- **Momentum Score** — 20%

## A Note on Signal Interpretation

The screener signals (Strong Buy, Buy, Hold, Avoid) reflect relative scoring within this model — they are a starting point for research, not a recommendation to buy or sell. A high composite score means a stock performs well across multiple quality and value dimensions compared to its sector peers. It does not guarantee future returns.

Key limitations to keep in mind:

- **Data depth** — The free FMP tier provides 5 years of history. CAGR calculations and trend analysis are more reliable with 10+ years of data.
- **No moat analysis** — Quantitative scores cannot capture *why* a company has a durable competitive advantage (brand, switching costs, network effects, cost structure). That qualitative judgement requires reading the business, not just the numbers.
- **Weights are not backtested** — The composite scoring weights are reasonable based on factor investing research, but have not been validated against historical returns for this specific combination.
- **No sector normalization** — Score thresholds are not adjusted for industry norms. A 30% gross margin means something very different for a retailer versus a software company.

Use these scores to build a shortlist of companies worth deeper investigation — not as a substitute for understanding the business.

## Notes

- Free FMP accounts are limited to 5 years of historical data
- 5-year and 10-year CAGRs require more data points than the free tier provides and will return `null`
- ROIC, Book Value/Share, and FCF/Share may return `null` if not available on your FMP plan
- The sector screener fetches data for 10 stocks plus SPY concurrently — allow 10–20 seconds

## License

MIT License

Copyright (c) 2026

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
