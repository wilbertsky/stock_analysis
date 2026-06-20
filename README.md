# Stock Analysis API

A REST API built in Rust with [Axum](https://github.com/tokio-rs/axum) that provides fundamental stock analysis using value investing frameworks. Data is sourced from [SEC EDGAR](https://www.sec.gov/cgi-bin/browse-edgar) (fundamentals) and [Yahoo Finance](https://finance.yahoo.com/) (prices and search). No API key required.

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
- **Sector Screener** — Ranks large-cap stocks within a sector using a weighted composite score that adapts to sector characteristics
- **Discovery Screener** — Surfaces small/mid-cap stocks close to their DCF intrinsic value with fundamentals above a quality floor — names too small to ever enter the sector screener's large-cap-only universe

## Getting Started

### Prerequisites

- [Rust](https://rustup.rs/) (stable)
- PostgreSQL

### Setup

```bash
git clone <repo-url>
cd axum-api

cp .env.example .env
# Edit .env with your DATABASE_URL and other required vars

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

Each sector screens up to 20 large-cap stocks (pre-filtered by market cap) and ranks them by a weighted composite score. The scoring model adapts to the sector — see [Sector Screener Models](#sector-screener-models) below. Expect 15–30 seconds response time as data is fetched concurrently.

The response includes `scoring_model`, `score_labels`, and `score_weights` fields so clients can display the correct labels dynamically without hardcoding.

The large-cap candidate list is sourced from FMP's `/stable/company-screener` endpoint (market cap ≥ $10B per sector), not literal S&P 500 / Nasdaq 100 index membership — FMP's own index-constituent endpoints aren't available on the Starter plan, and the free third-party datasets this previously depended on are no longer reachable. A market-cap-band screen is a close approximation of "large cap" for scoring purposes but won't perfectly match actual index composition.

### Discovery Screener

| Method | Path | Description |
|--------|------|--------------|
| `GET` | `/api/discovery` | Near-miss small/mid-cap value candidates |

Optional query param: `sector` (same slugs as the sector screener; omit to screen across all sectors).

The sector screener only ever evaluates large-cap stocks (market cap ≥ $10B) — smaller companies never enter its candidate pool, regardless of how they'd score. The discovery screener is a separate, additive endpoint that sources a small/mid-cap universe (market cap $300M–$5B, via the same FMP company-screener) and surfaces candidates that are:

1. **Close to fair value** — current price within ±20% of the DCF intrinsic value estimate (`/stock/{ticker}/intrinsic-value`'s methodology), in either direction. Slightly-above candidates are included alongside slightly-below ones because the underlying DCF formula tends to be conservative for asset-light, high-growth businesses with low book value — the same companies most likely to be overlooked by a large-cap-only or Graham-Number-style screen.
2. **Above a quality floor, not a quality ceiling** — quality score ≥ 40, debt safety score ≥ 40 (both reuse the same 0–100 scores documented above), and Piotroski F-Score ≥ 4 out of 9. The Piotroski threshold is grounded in Piotroski's original research, which found the predictive edge concentrated in avoiding the distress decile (scores 0–2), not in requiring a top score. The quality/debt-safety floors reuse the sector screener's own "Average" tier boundary (≥40) rather than an arbitrary number.

Results are sorted by closeness to intrinsic value. The ±20% band and the $300M–$5B market-cap range are practical starting heuristics, not backtested constants — unlike the Piotroski cutoff, there's no equivalent published study defining "near miss to DCF intrinsic value." Treat them as tunable, not authoritative.

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

# Top-ranked technology stocks (Standard model)
curl http://localhost:8080/api/screener/technology

# Top-ranked financial stocks (Financials model)
curl http://localhost:8080/api/screener/financials

# Small/mid-cap near-miss value candidates, technology sector only
curl http://localhost:8080/api/discovery?sector=technology

# Same, across all sectors
curl http://localhost:8080/api/discovery
```

## Analysis Methods

### Intrinsic Value — Simplified DCF
Projects EPS 10 years forward using the historical EPS CAGR, applies a growth-adjusted P/E of 2× the growth rate percentage, then discounts back to today at a 15% minimum required rate of return. This approach is rooted in standard discounted cash flow theory as practiced by Benjamin Graham and Warren Buffett. The margin of safety price is 50% of the intrinsic value estimate — a concept introduced by Graham to account for uncertainty in any projection.

### Graham Number (Benjamin Graham)
Conservative intrinsic value estimate based purely on earnings and book value: `√(22.5 × EPS × BVPS)`. Works best for stable, asset-heavy companies.

### PEG Ratio (Peter Lynch)
Adjusts the P/E ratio for growth: `P/E ÷ EPS growth rate %`. Below 1.0 may indicate undervaluation relative to growth; below 0.5 was considered a bargain by Lynch.

### Piotroski F-Score (Joseph Piotroski)
Nine binary signals across three groups — profitability (F1–F4), leverage and liquidity (F5–F7), and operating efficiency (F8–F9). Scores ≥7 indicate a financially strong company; scores ≤2 indicate potential distress. Designed for operating businesses — not appropriate for banks or REITs.

### Quality Score
Composite 0–100 score based on gross margin (pricing power), return on equity (capital efficiency), and debt-to-equity (financial risk). High-quality companies typically have wide margins, high ROE, and manageable debt — the combination most associated with durable competitive advantage.

### Momentum Score
Measures 3-month, 6-month, and 12-month price returns relative to the S&P 500 (SPY). Score starts at 50 (neutral) and shifts up for outperformance or down for underperformance across each period. Grounded in decades of academic research showing that recent outperformers tend to continue outperforming near-term. Applicable to all sectors.

### Sector-Specific Score Helpers
Five additional scoring functions (all 0–100) are used by the sector screener models:

- **ROE Quality Score** — Return on equity tiered 0–100. ≥20% = 100, ≥15% = 80, ≥10% = 60, ≥7% = 40, ≥4% = 20. Used as the primary signal in the Financials model, where ROE is the best measure of profitability.
- **P/B Value Score** — Price-to-book ratio (price ÷ book value per share) inverted to a 0–100 score. ≤1.0 = 100, ≤1.5 = 80, ≤2.0 = 60, ≤3.0 = 40, ≤5.0 = 20. Lower P/B = closer to or below asset value.
- **Debt Safety Score** — Debt-to-equity ratio tiered 0–100. Net cash position or D/E < 0.3 = 100. Rewarded for conservative balance sheets; penalized for high leverage.
- **Dividend Quality Score** — Combines dividend yield (0–50 points) with payout sustainability (0–30 points), capped at 100. No dividend paid = 0. Used for REITs, Consumer Staples, and Utilities.
- **FCF Yield Score** — Free cash flow per share divided by price, tiered 0–100. ≥10% = 100, ≥7% = 80, ≥5% = 60, ≥3% = 40, ≥1% = 20. Used in the Energy model where cash generation matters more than reported earnings.

## Sector Screener Models

The screener applies one of five scoring models depending on the sector. Each model assembles four score slots (A–D, all 0–100) with different weights. The `score_labels` and `score_weights` fields in the API response always describe the model in use.

### Standard Model
*Sectors: Technology, Healthcare, Communication, Consumer Discretionary, Industrials, Materials*

| Slot | Signal | Weight | Description |
|------|--------|--------|-------------|
| A | Piotroski F-Score | 30% | Accounting quality across 9 binary signals |
| B | Business Quality | 25% | Gross margin, ROE, and debt levels |
| C | DCF Value Signal | 25% | Price vs intrinsic value and margin of safety |
| D | Momentum vs SPY | 20% | 3/6/12-month relative price performance |

### Financials Model
*Sectors: Financials*

Banks and financial companies have balance sheets where debt is a product, not a burden. Piotroski's asset-efficiency signals are not meaningful here. ROE and P/B are the standard lenses for financial sector valuation.

| Slot | Signal | Weight | Description |
|------|--------|--------|-------------|
| A | Return on Equity | 35% | Primary profitability measure for banks |
| B | Price-to-Book | 25% | Price vs net asset value |
| C | Momentum vs SPY | 25% | Relative price performance |
| D | Debt Safety | 15% | Leverage relative to equity |

### Real Estate Model
*Sectors: Real Estate*

REITs are required to distribute ≥90% of taxable income as dividends, making DCF and earnings-based metrics misleading. Dividend yield and asset value (P/B) are the primary signals.

| Slot | Signal | Weight | Description |
|------|--------|--------|-------------|
| A | Dividend Quality | 35% | Yield level and payout sustainability |
| B | Price-to-Book | 25% | Price vs underlying property asset value |
| C | Momentum vs SPY | 25% | Relative price performance |
| D | Debt Safety | 15% | Leverage (critical in rising rate environments) |

### Energy Model
*Sectors: Energy*

Energy companies are capital-intensive with earnings tied to commodity price cycles. Free cash flow generation is more reliable than reported EPS for evaluating cash return capacity.

| Slot | Signal | Weight | Description |
|------|--------|--------|-------------|
| A | Piotroski F-Score | 25% | Balance sheet and efficiency signals |
| B | FCF Yield | 30% | Free cash flow per share ÷ price |
| C | Momentum vs SPY | 30% | Relative price performance |
| D | Business Quality | 15% | Gross margin and debt quality |

### Dividend Model
*Sectors: Consumer Staples, Utilities*

Defensive sectors held primarily for stable, growing income. Dividend health is weighted heavily alongside traditional value and quality signals.

| Slot | Signal | Weight | Description |
|------|--------|--------|-------------|
| A | Dividend Quality | 30% | Yield level and payout sustainability |
| B | Business Quality | 25% | Gross margin, ROE, and debt levels |
| C | DCF Value Signal | 25% | Price vs intrinsic value |
| D | Momentum vs SPY | 20% | Relative price performance |

## A Note on Score Interpretation

The screener returns a `score_tier` label (High, Above Average, Average, Below Average) based on the composite score. This is an educational grouping reflecting how a stock ranks quantitatively within its sector peers — it is not a recommendation to buy or sell. A high composite score means a stock performs well across multiple quality and value dimensions. It does not guarantee future returns.

Key limitations to keep in mind:

- **Data depth** — SEC EDGAR filings provide 10-K history which varies by company. CAGR calculations and trend analysis are more reliable with 10+ years of data.
- **No moat analysis** — Quantitative scores cannot capture *why* a company has a durable competitive advantage (brand, switching costs, network effects, cost structure). That qualitative judgement requires reading the business, not just the numbers.
- **Weights are not backtested** — The composite scoring weights are based on factor investing research but have not been validated against historical returns for this specific combination.
- **Sector models are heuristics** — The five models reflect broadly accepted analytical frameworks for each sector type, but individual companies within a sector may warrant a different lens.

The discovery screener carries the same caveats, plus one more specific to its design: clearing a quality *floor* is a deliberately low bar — it excludes obvious distress, not weak fundamentals generally. A discovery result is a starting point for due diligence, not a vetted "good" company in the way a high sector-screener composite score is intended to suggest.

Use these scores to build a shortlist of companies worth deeper investigation — not as a substitute for understanding the business.

## Disclaimer

All scores and outputs provided by this API are for **educational purposes only**. They do not constitute investment advice, a recommendation to buy or sell any security, or a guarantee of future performance. Quantitative scores are based on publicly available historical financial data and make no prediction about future stock prices or returns. Always conduct your own research and consult a licensed financial advisor before making investment decisions.

## Notes

- Fundamental data comes from SEC EDGAR 10-K filings — availability varies by company age and filing history
- 5-year and 10-year CAGRs require sufficient filing history and will return `null` if data is unavailable
- ROIC, Book Value/Share, and FCF/Share may return `null` if not reported in EDGAR filings
- The sector screener fetches data for up to 20 stocks plus SPY concurrently — allow 15–30 seconds
- The discovery screener fetches up to 40 small/mid-cap candidates concurrently — also allow 15–30 seconds
- Both screeners source their candidate universe from FMP's `/stable/company-screener` (market-cap-band filtered), not literal index membership — see the Sector Screener and Discovery Screener sections above

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
