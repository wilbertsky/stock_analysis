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
}
