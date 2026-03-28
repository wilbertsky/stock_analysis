# Stock Analysis API

A REST API built in Rust with [Axum](https://github.com/tokio-rs/axum) that provides fundamental stock analysis using value investing frameworks. Data is sourced from [Financial Modeling Prep (FMP)](https://financialmodelingprep.com/).

## Features

- **Big Five Fundamentals** — Revenue, EPS, Book Value/Share, Free Cash Flow/Share, and ROIC over up to 5 years
- **Growth Rates** — Compound Annual Growth Rate (CAGR) for each Big Five metric
- **Rule #1 Sticker Price** — Phil Town's fair value estimate with margin of safety (50% discount)
- **Graham Number** — Benjamin Graham's intrinsic value formula: √(22.5 × EPS × BVPS)
- **PEG Ratio** — Peter Lynch's growth-adjusted valuation ratio
- **Summary** — All valuations in a single endpoint

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

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/health` | Service health check |
| `GET` | `/api/stock/{ticker}/fundamentals` | Big Five raw data, up to 5 years |
| `GET` | `/api/stock/{ticker}/growth-rates` | CAGR for each Big Five metric |
| `GET` | `/api/stock/{ticker}/rule-number-one` | Rule #1 sticker price and margin of safety |
| `GET` | `/api/stock/{ticker}/graham-number` | Graham Number intrinsic value |
| `GET` | `/api/stock/{ticker}/peg` | PEG ratio |
| `GET` | `/api/stock/{ticker}/summary` | All valuations in one response |

### Example

```bash
curl http://localhost:8080/api/stock/AAPL/summary
```

## Valuation Methods

### Rule #1 (Phil Town)
Projects EPS 10 years forward using historical CAGR, applies a default P/E of 2× the growth rate, then discounts back to today at a 15% minimum acceptable rate of return. The margin of safety price is 50% of the sticker price — the target buy price.

### Graham Number (Benjamin Graham)
Conservative intrinsic value estimate based purely on earnings and book value: `√(22.5 × EPS × BVPS)`. Works best for stable, asset-heavy companies.

### PEG Ratio (Peter Lynch)
Adjusts the P/E ratio for growth: `P/E ÷ EPS growth rate %`. Below 1.0 may indicate undervaluation relative to growth; below 0.5 was considered a bargain by Lynch.

## Notes

- Free FMP accounts are limited to 5 years of historical data and 5 requests per second
- 5-year and 10-year CAGRs require more data points than the free tier provides and will return `null`
- ROIC, Book Value/Share, and FCF/Share may return `null` if not available on your FMP plan

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
