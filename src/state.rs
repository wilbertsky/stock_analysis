use std::sync::Arc;
use base64::Engine as _;
use reqwest::Client;
use sqlx::PgPool;
use crate::fmp::FmpClient;

#[derive(Clone)]
pub struct AppState {
    /// Global FMP client using the server-level API key. Used by all public endpoints
    /// and as a fallback for authenticated endpoints when the user has no stored key.
    pub fmp: Arc<FmpClient>,
    pub db: PgPool,
    /// HS256 secret for signing/verifying JWT tokens.
    pub jwt_secret: Arc<[u8]>,
    /// AES-256-GCM key for encrypting/decrypting stored FMP API keys.
    pub fmp_enc_key: Arc<[u8; 32]>,
    /// Shared HTTP client — all per-user FmpClient instances reuse this connection pool.
    pub http_client: Client,
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

        Self {
            fmp: Arc::new(FmpClient::with_shared_client(
                http_client.clone(),
                api_key,
                "https://financialmodelingprep.com/stable".to_owned(),
            )),
            db,
            jwt_secret: Arc::from(jwt_secret.as_slice()),
            fmp_enc_key: Arc::new(enc_key),
            http_client,
        }
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

    /// Alternate constructor pointing FMP at a custom base URL (for integration tests).
    #[cfg(test)]
    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        let http_client = Client::new();
        Self {
            fmp: Arc::new(FmpClient::with_base_url(api_key, base_url)),
            db: PgPool::connect_lazy("postgres://localhost/test_db")
                .expect("lazy pool creation"),
            jwt_secret: Arc::from(b"test_jwt_secret_key_32bytes_pad_".as_slice()),
            fmp_enc_key: Arc::new([0u8; 32]),
            http_client,
        }
    }
}
