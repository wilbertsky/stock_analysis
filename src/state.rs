use std::sync::Arc;
use base64::Engine as _;
use lettre::{
    AsyncSmtpTransport, Tokio1Executor,
    transport::smtp::authentication::Credentials,
};
use reqwest::Client;
use sqlx::PgPool;
use crate::edgar::EdgarClient;
use crate::fmp::FmpClient;
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
    /// S&P 500 constituent list indexed by sector slug.
    pub sp500: Arc<Sp500>,
    /// Optional SMTP transport for sending password reset emails.
    /// None when SMTP_HOST is not configured.
    pub mailer: Option<Arc<AsyncSmtpTransport<Tokio1Executor>>>,
    /// From-address used for outgoing emails (SMTP_FROM env var).
    pub smtp_from: Option<Arc<str>>,
    /// App base URL used in reset email links (APP_URL env var, e.g. "https://app.example.com").
    pub app_url: Arc<str>,
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
        let sp500 = Arc::new(Sp500::load(&http_client).await);

        // Optional SMTP mailer — only built when SMTP_HOST is present.
        let (mailer, smtp_from) = build_mailer();
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
            mailer,
            smtp_from,
            app_url: Arc::from(app_url.as_str()),
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

/// Build an SMTP mailer from environment variables.
/// Returns `(None, None)` when `SMTP_HOST` is not set — email is disabled.
fn build_mailer() -> (Option<Arc<AsyncSmtpTransport<Tokio1Executor>>>, Option<Arc<str>>) {
    let host = match std::env::var("SMTP_HOST") {
        Ok(h) if !h.is_empty() => h,
        _ => return (None, None),
    };
    let user = std::env::var("SMTP_USERNAME").unwrap_or_default();
    let pass = std::env::var("SMTP_PASSWORD").unwrap_or_default();
    let port: u16 = std::env::var("SMTP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(587);
    let from = std::env::var("SMTP_FROM").unwrap_or_else(|_| format!("noreply@{}", host));

    let builder = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host)
        .expect("Invalid SMTP host")
        .port(port)
        .credentials(Credentials::new(user, pass));

    (Some(Arc::new(builder.build())), Some(Arc::from(from.as_str())))
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
            mailer: None,
            smtp_from: None,
            app_url: Arc::from("http://localhost:1420"),
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
        // Tests use an empty EDGAR client (no CIK map) so all calls route through FMP mock.
        let edgar = Arc::new(EdgarClient::new_empty(http_client.clone()));
        // Disabled so test price requests fall back to the FMP mock server
        let yahoo = Arc::new(YahooClient::new_disabled());
        let providers = Arc::new(Providers::new(edgar, yahoo));
        // Empty S&P 500 for tests — screener falls back to sectors.rs lists
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
            mailer: None,
            smtp_from: None,
            app_url: Arc::from("http://localhost:1420"),
        }
    }
}
