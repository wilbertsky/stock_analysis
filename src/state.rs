use std::sync::Arc;
use crate::fmp::FmpClient;

#[derive(Clone)]
pub struct AppState {
    pub fmp: Arc<FmpClient>,
}

impl AppState {
    pub fn new(api_key: String) -> Self {
        Self {
            fmp: Arc::new(FmpClient::new(api_key)),
        }
    }

    /// Alternate constructor pointing the FMP client at a custom base URL.
    /// Used in integration tests to point at a mock HTTP server.
    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            fmp: Arc::new(FmpClient::with_base_url(api_key, base_url)),
        }
    }
}
