use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use base64::Engine as _;
use reqwest::Client;
use sqlx::PgPool;
use tokio::sync::RwLock;
use crate::edgar::EdgarClient;
use crate::fmp::FmpClient;
use crate::models::{DiscoveryResponse, SectorScreenerResponse};
use crate::providers::Providers;
use crate::sp500::Sp500;
use crate::yahoo::YahooClient;

#[derive(Clone)]
pub struct AppState {
    /// Unified data provider: EDGAR + Yahoo by default, FMP as fallback.
    pub providers: Arc<Providers>,
    /// Global FMP client — kept for direct FMP access (per-user keys, test overrides).
    pub fmp: Arc<FmpClient>,
    pub db: PgPool,
    /// HS256 secret for signing/verifying JWT tokens.
    pub jwt_secret: Arc<[u8]>,
    /// AES-256-GCM key for encrypting/decrypting stored FMP API keys.
    pub fmp_enc_key: Arc<[u8; 32]>,
    /// Shared HTTP client — all per-user FmpClient instances reuse this connection pool.
    pub http_client: Client,
    /// Large-cap candidate list (FMP company-screener, not literal index membership)
    /// indexed by sector slug.
    pub sp500: Arc<Sp500>,
    /// Resend API key for sending password reset emails via HTTPS (no SMTP ports needed).
    /// None when RESEND_API_KEY is not configured — email is disabled.
    pub resend_api_key: Option<Arc<str>>,
    /// From-address for outgoing emails (EMAIL_FROM env var).
    /// Defaults to onboarding@resend.dev when not set (works without domain verification).
    pub email_from: Arc<str>,
    /// App base URL used in reset email links (APP_URL env var).
    pub app_url: Arc<str>,
    /// Cached discovery results per sector slug. Populated on first request and refreshed
    /// by the background task. Serves subsequent requests instantly without FMP calls.
    pub discovery_cache: Arc<RwLock<HashMap<String, (Instant, DiscoveryResponse)>>>,
    /// Cached screener results per sector slug. Same pattern as discovery_cache.
    pub screener_cache: Arc<RwLock<HashMap<String, (Instant, SectorScreenerResponse)>>>,
    /// Base URL of the RAG app (RAG_URL env var). None when not configured — chat endpoint
    /// returns 503 rather than panicking so the rest of the API stays healthy.
    pub rag_url: Option<Arc<str>>,
}

impl AppState {
    /// Production constructor. Reads env vars internally.
    pub async fn new(api_key: String) -> Self {
        let database_url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let jwt_secret = std::env::var("JWT_SECRET")
            .expect("JWT_SECRET must be set")
            .into_bytes();
        let enc_key_b64 =
            std::env::var("FMP_ENC_KEY").expect("FMP_ENC_KEY must be set (32 bytes, base64)");
        let enc_key_bytes = base64::engine::general_purpose::STANDARD
            .decode(enc_key_b64)
            .expect("FMP_ENC_KEY must be valid base64");
        assert!(enc_key_bytes.len() == 32, "FMP_ENC_KEY must decode to exactly 32 bytes");
        let mut enc_key = [0u8; 32];
        enc_key.copy_from_slice(&enc_key_bytes);

        let db = PgPool::connect(&database_url).await.expect("Failed to connect to database");
        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("Failed to build HTTP client");

        let fmp = Arc::new(FmpClient::with_shared_client(
            http_client.clone(),
            api_key,
            "https://financialmodelingprep.com/stable".to_owned(),
        ));
        let edgar = Arc::new(EdgarClient::new(http_client.clone()).await);
        let yahoo = Arc::new(YahooClient::new(http_client.clone()));
        let providers = Arc::new(Providers::new(edgar, yahoo));
        let sp500 = Arc::new(Sp500::load(&fmp).await);

        let rag_url = std::env::var("RAG_URL").ok()
            .filter(|u| !u.is_empty())
            .map(|u| {
                tracing::info!("RAG service configured at {u}");
                Arc::from(u.as_str())
            });
        if rag_url.is_none() {
            tracing::warn!("RAG_URL not set — /api/chat proxy disabled");
        }

        let resend_api_key = std::env::var("RESEND_API_KEY").ok()
            .filter(|k| !k.is_empty())
            .map(|k| { tracing::info!("Resend email configured"); Arc::from(k.as_str()) });
        if resend_api_key.is_none() {
            tracing::warn!("RESEND_API_KEY not set — password reset emails disabled");
        }

        let email_from = std::env::var("EMAIL_FROM")
            .unwrap_or_else(|_| "onboarding@resend.dev".to_owned());
        let app_url = std::env::var("APP_URL")
            .unwrap_or_else(|_| "https://stockanalysis-production-fbca.up.railway.app".to_owned());

        Self {
            providers,
            fmp,
            db,
            jwt_secret: Arc::from(jwt_secret.as_slice()),
            fmp_enc_key: Arc::new(enc_key),
            http_client,
            sp500,
            resend_api_key,
            email_from: Arc::from(email_from.as_str()),
            app_url: Arc::from(app_url.as_str()),
            discovery_cache: Arc::new(RwLock::new(HashMap::new())),
            screener_cache: Arc::new(RwLock::new(HashMap::new())),
            rag_url,
        }
    }

    /// Returns a Providers instance that uses EDGAR/Yahoo as primary sources and the
    /// server-level FMP client as a fallback. Use this for all authenticated endpoints
    /// so that data is always available even when EDGAR is missing a ticker or field.
    pub fn providers_with_server_fmp(&self) -> Providers {
        self.providers.with_fmp(self.fmp.clone())
    }

    /// Build an FmpClient scoped to a specific user's API key (if they have one stored),
    /// falling back to the global client. Callers should call this to get the right key
    /// for authenticated portfolio/analysis requests.
    pub fn fmp_for_key(&self, user_key: Option<String>) -> Arc<FmpClient> {
        match user_key {
            Some(key) => Arc::new(FmpClient::with_shared_client(
                self.http_client.clone(),
                key,
                "https://financialmodelingprep.com/stable".to_owned(),
            )),
            None => self.fmp.clone(),
        }
    }
}

impl AppState {
    /// Constructor for integration tests: real local DB + mock FMP server URL.
    /// Reads DATABASE_URL from the environment (`.env` via dotenvy).
    #[cfg(test)]
    pub async fn with_db_and_base_url(api_key: String, base_url: String) -> Self {
        dotenvy::dotenv().ok();
        let database_url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set for integration tests");
        let http_client = Client::new();
        let fmp = Arc::new(FmpClient::with_base_url(api_key, base_url.clone()));
        let edgar = Arc::new(EdgarClient::new_empty(http_client.clone()));
        let yahoo = Arc::new(YahooClient::new_disabled());
        let providers = Arc::new(Providers::new(edgar, yahoo));
        let sp500 = Arc::new(Sp500::empty());
        let db = PgPool::connect(&database_url)
            .await
            .expect("Failed to connect to test database");
        Self {
            providers,
            fmp,
            db,
            jwt_secret: Arc::from(b"test_jwt_secret_key_32bytes_pad_".as_slice()),
            fmp_enc_key: Arc::new([0u8; 32]),
            http_client,
            sp500,
            resend_api_key: None,
            email_from: Arc::from("onboarding@resend.dev"),
            app_url: Arc::from("http://localhost:1420"),
            discovery_cache: Arc::new(RwLock::new(HashMap::new())),
            screener_cache: Arc::new(RwLock::new(HashMap::new())),
            rag_url: None,
        }
    }

    /// Alternate constructor pointing FMP at a custom base URL (for integration tests).
    #[cfg(test)]
    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        use crate::edgar::EdgarClient;
        use crate::providers::Providers;
        use crate::sp500::Sp500;
        use crate::yahoo::YahooClient;

        let http_client = Client::new();
        let fmp = Arc::new(FmpClient::with_base_url(api_key, base_url.clone()));
        let edgar = Arc::new(EdgarClient::new_empty(http_client.clone()));
        let yahoo = Arc::new(YahooClient::new_disabled());
        let providers = Arc::new(Providers::new(edgar, yahoo));
        let sp500 = Arc::new(Sp500::empty());
        Self {
            providers,
            fmp,
            db: PgPool::connect_lazy("postgres://localhost/test_db")
                .expect("lazy pool creation"),
            jwt_secret: Arc::from(b"test_jwt_secret_key_32bytes_pad_".as_slice()),
            fmp_enc_key: Arc::new([0u8; 32]),
            http_client,
            sp500,
            resend_api_key: None,
            email_from: Arc::from("onboarding@resend.dev"),
            app_url: Arc::from("http://localhost:1420"),
            discovery_cache: Arc::new(RwLock::new(HashMap::new())),
            screener_cache: Arc::new(RwLock::new(HashMap::new())),
            rag_url: None,
        }
    }
}
