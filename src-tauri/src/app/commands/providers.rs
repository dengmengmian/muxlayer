use std::time::Instant;
use tauri::State;

use crate::app::state::AppState;
use crate::errors::AppError;
use crate::models::provider::{
    CreateProviderInput, ProviderTestResult, ProviderView, UpdateProviderInput,
};
use crate::storage;

// ── Provider Commands ──────────────────────────────────────────

/// Discover local Ollama / LM Studio / common OpenAI-compatible ports.
/// 探测是阻塞 TCP + 阻塞 HTTP(最长约 2.4s),放到 blocking 线程,不占主线程。
#[tauri::command]
#[specta::specta]
pub async fn discover_local_endpoints(
) -> Result<Vec<crate::tools::local_discovery::LocalEndpoint>, AppError> {
    super::run_blocking(|| Ok(crate::tools::local_discovery::discover())).await
}

/// Refiner recommendation for a provider type (never force-on).
#[tauri::command]
#[specta::specta]
pub fn get_refiner_hint(provider_type: String) -> Result<Option<String>, AppError> {
    Ok(crate::gateway::refiner_hints::suggestion_for(&provider_type).map(str::to_string))
}

/// Auto-derive capabilities for a list of model IDs given a provider type.
/// Used by the "Auto-detect" button in the capability matrix editor to fill
/// in sensible defaults without forcing the user to tick every box.
#[tauri::command]
#[specta::specta]
pub fn seed_model_capabilities(
    provider_type: String,
    model_ids: Vec<String>,
) -> Result<std::collections::HashMap<String, Vec<String>>, AppError> {
    Ok(crate::providers::capabilities::seed_for_models(
        &provider_type,
        &model_ids,
    ))
}

/// Seed-fill the model_capabilities matrix for a provider and persist it.
/// Only fills models that are missing from the existing matrix — never
/// overwrites manual edits. Used by the "测试" button after connectivity
/// succeeds, so newly added models pick up sensible defaults without the
/// user needing to open the form dialog.
#[tauri::command]
#[specta::specta]
pub fn autofill_provider_capabilities(
    id: String,
    state: State<'_, AppState>,
) -> Result<usize, AppError> {
    let conn = state
        .db
        .get()
        .map_err(|_| AppError::internal("DB lock failed"))?;
    let provider = storage::providers::get_by_id(&conn, &id)?;

    // Existing matrix — preserve user edits.
    let mut matrix: std::collections::HashMap<String, Vec<String>> = provider
        .model_capabilities
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    // Target models: supported_models ∪ default_model ∪ reasoning_model.
    let mut targets: std::collections::BTreeSet<String> = Default::default();
    if let Some(ref sm) = provider.supported_models {
        if let Ok(list) = serde_json::from_str::<Vec<String>>(sm) {
            for m in list {
                targets.insert(m);
            }
        }
    }
    if !provider.default_model.is_empty() {
        targets.insert(provider.default_model.clone());
    }
    if let Some(ref rm) = provider.reasoning_model {
        if !rm.is_empty() {
            targets.insert(rm.clone());
        }
    }

    let mut filled = 0usize;
    for model in targets {
        if let std::collections::hash_map::Entry::Vacant(slot) = matrix.entry(model) {
            let caps =
                crate::providers::capabilities::seed_for_model(&provider.provider_type, slot.key());
            if !caps.is_empty() {
                slot.insert(caps);
                filled += 1;
            }
        }
    }

    if filled > 0 {
        let json = serde_json::to_string(&matrix)
            .map_err(|e| AppError::internal(format!("serialize matrix: {e}")))?;
        storage::providers::update_model_capabilities(&conn, &id, &json)?;
    }

    Ok(filled)
}

#[tauri::command]
#[specta::specta]
pub fn list_providers(state: State<'_, AppState>) -> Result<Vec<ProviderView>, AppError> {
    let conn = state
        .db
        .get()
        .map_err(|_| AppError::internal("DB lock failed"))?;
    let providers = storage::providers::list_all(&conn)?;
    Ok(providers.into_iter().map(|p| p.to_view()).collect())
}

#[tauri::command]
#[specta::specta]
pub fn get_provider(id: String, state: State<'_, AppState>) -> Result<ProviderView, AppError> {
    let conn = state
        .db
        .get()
        .map_err(|_| AppError::internal("DB lock failed"))?;
    let provider = storage::providers::get_by_id(&conn, &id)?;
    Ok(provider.to_view())
}

/// Return the plain-text api keys for a provider, in storage order.
///
/// Used by the edit form to repopulate every key slot so users can see
/// which key is which (the masked view alone hides that). Calling code
/// must keep the keys in memory only as long as the dialog is open;
/// they're not redacted in any subsequent log path.
#[tauri::command]
#[specta::specta]
pub fn get_provider_keys(id: String, state: State<'_, AppState>) -> Result<Vec<String>, AppError> {
    let conn = state
        .db
        .get()
        .map_err(|_| AppError::internal("DB lock failed"))?;
    let provider = storage::providers::get_by_id(&conn, &id)?;
    let raw = match provider.api_key {
        Some(k) if !k.trim().is_empty() => k,
        _ => return Ok(Vec::new()),
    };
    let trimmed = raw.trim();
    if trimmed.starts_with('[') {
        if let Ok(keys) = serde_json::from_str::<Vec<String>>(trimmed) {
            return Ok(keys.into_iter().filter(|k| !k.trim().is_empty()).collect());
        }
    }
    Ok(vec![trimmed.to_string()])
}

#[tauri::command]
#[specta::specta]
pub fn create_provider(
    mut input: CreateProviderInput,
    state: State<'_, AppState>,
) -> Result<ProviderView, AppError> {
    if input.name.trim().is_empty() {
        return Err(AppError::validation("Provider name is required"));
    }
    if input.base_url.trim().is_empty() {
        return Err(AppError::validation("Base URL is required"));
    }
    if input.default_model.trim().is_empty() {
        return Err(AppError::validation("Default model is required"));
    }
    if let Some(t) = input.timeout_seconds {
        if t <= 0 {
            return Err(AppError::validation("Timeout must be greater than 0"));
        }
    }
    storage::recommended_mappings::apply_to_create_input(&mut input);

    let conn = state
        .db
        .get()
        .map_err(|_| AppError::internal("DB lock failed"))?;
    let provider = storage::providers::create(&conn, input)?;

    // Auto-add new provider to all existing route profiles
    let profile_ids: Vec<String> = conn
        .prepare("SELECT id FROM route_profiles")?
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    for pid in &profile_ids {
        let _ = storage::route_profiles::add_provider(
            &conn,
            pid,
            &provider.id,
            crate::models::route_profile::AddProviderToRouteInput {
                priority: None,
                model_override: None,
                cooldown_seconds: None,
                failover_on_status_codes: None,
                failover_on_error_keywords: None,
                routing_conditions: None,
            },
        );
    }

    Ok(provider.to_view())
}

#[tauri::command]
#[specta::specta]
pub fn update_provider(
    id: String,
    input: UpdateProviderInput,
    state: State<'_, AppState>,
) -> Result<ProviderView, AppError> {
    if let Some(ref name) = input.name {
        if name.trim().is_empty() {
            return Err(AppError::validation("Provider name cannot be empty"));
        }
    }
    if let Some(ref url) = input.base_url {
        if url.trim().is_empty() {
            return Err(AppError::validation("Base URL cannot be empty"));
        }
    }
    if let Some(ref model) = input.default_model {
        if model.trim().is_empty() {
            return Err(AppError::validation("Default model cannot be empty"));
        }
    }
    if let Some(t) = input.timeout_seconds {
        if t <= 0 {
            return Err(AppError::validation("Timeout must be greater than 0"));
        }
    }

    let conn = state
        .db
        .get()
        .map_err(|_| AppError::internal("DB lock failed"))?;
    let provider = storage::providers::update(&conn, &id, input)?;
    Ok(provider.to_view())
}

#[tauri::command]
#[specta::specta]
pub fn delete_provider(id: String, state: State<'_, AppState>) -> Result<bool, AppError> {
    let conn = state
        .db
        .get()
        .map_err(|_| AppError::internal("DB lock failed"))?;
    storage::providers::delete(&conn, &id)
}

#[tauri::command]
#[specta::specta]
pub fn set_active_provider(
    app_handle: tauri::AppHandle,
    id: String,
    state: State<'_, AppState>,
) -> Result<ProviderView, AppError> {
    let provider = {
        let conn = state
            .db
            .get()
            .map_err(|_| AppError::internal("DB lock failed"))?;
        storage::providers::set_active(&conn, &id)?
    };
    crate::app::tray::refresh_tray(&app_handle);
    Ok(provider.to_view())
}

#[tauri::command]
#[specta::specta]
pub async fn fetch_provider_models(
    id: String,
    state: State<'_, AppState>,
) -> Result<Vec<String>, AppError> {
    let (base_url, api_key, timeout_seconds) = {
        let conn = state
            .db
            .get()
            .map_err(|_| AppError::internal("DB lock failed"))?;
        let provider = storage::providers::get_by_id(&conn, &id)?;
        (
            provider.base_url,
            provider.api_key,
            provider.timeout_seconds,
        )
    };

    let api_key = match api_key {
        Some(k) if !k.is_empty() => k,
        _ => {
            return Err(AppError::new(
                crate::errors::codes::PROVIDER_API_KEY_MISSING,
                "API key is not set",
            ))
        }
    };

    let client = crate::gateway::http_client::apply_optional_proxy(
        reqwest::Client::builder().timeout(std::time::Duration::from_secs(timeout_seconds as u64)),
        crate::gateway::http_client::proxy_from_db(&state.db),
    )
    .build()
    .map_err(|e| AppError::internal(format!("HTTP client error: {e}")))?;

    // Try /models then /v1/models
    let base = base_url.trim_end_matches('/');
    let urls = vec![format!("{base}/models"), format!("{base}/v1/models")];

    for url in &urls {
        let resp = client
            .get(url)
            .header("Authorization", format!("Bearer {api_key}"))
            .send()
            .await;

        if let Ok(resp) = resp {
            if resp.status().is_success() {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    let models: Vec<String> = body
                        .get("data")
                        .and_then(|d| d.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|m| {
                                    m.get("id").and_then(|id| id.as_str()).map(String::from)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    if !models.is_empty() {
                        // Auto-save to provider
                        let models_json = serde_json::to_string(&models).unwrap_or_default();
                        let conn = state.db.get().map_err(|_| AppError::internal("DB lock"))?;
                        let _ = conn.execute(
                            "UPDATE providers SET supported_models=?1, updated_at=?2 WHERE id=?3",
                            rusqlite::params![&models_json, chrono::Utc::now().to_rfc3339(), &id],
                        );
                        let _ = storage::recommended_mappings::supplement_provider(
                            &conn,
                            &id,
                            storage::recommended_mappings::MappingProfile::All,
                        );
                        return Ok(models);
                    }
                }
            }
        }
    }

    Err(AppError::new(
        crate::errors::codes::PROVIDER_REQUEST_FAILED,
        "Could not fetch models from provider",
    ))
}

/// Speedtest a single provider — sends a 1-token probe request and reports
/// connect / TTFB / total latency. User-triggered only (never automatic) to
/// avoid burning tokens.
#[tauri::command]
#[specta::specta]
pub async fn provider_speedtest(
    id: String,
    state: State<'_, AppState>,
) -> Result<crate::diagnostics::speedtest::ProviderSpeedReport, AppError> {
    let provider = {
        let conn = state
            .db
            .get()
            .map_err(|_| AppError::internal("DB lock failed"))?;
        storage::providers::get_by_id(&conn, &id)?
    };
    Ok(crate::diagnostics::speedtest::probe(&provider).await)
}

/// Speedtest every enabled provider in parallel. Heavier than single-provider
/// probe — confirm the user wants this in UI before calling.
#[tauri::command]
#[specta::specta]
pub async fn provider_speedtest_all(
    state: State<'_, AppState>,
) -> Result<Vec<crate::diagnostics::speedtest::ProviderSpeedReport>, AppError> {
    let providers = {
        let conn = state
            .db
            .get()
            .map_err(|_| AppError::internal("DB lock failed"))?;
        storage::providers::list_all(&conn)?
            .into_iter()
            .filter(|p| p.enabled)
            .collect::<Vec<_>>()
    };
    crate::diagnostics::speedtest::probe_many(&providers).await
}

#[tauri::command]
#[specta::specta]
pub async fn test_provider(
    id: String,
    state: State<'_, AppState>,
) -> Result<ProviderTestResult, AppError> {
    let (base_url, api_key, timeout_seconds) = {
        let conn = state
            .db
            .get()
            .map_err(|_| AppError::internal("DB lock failed"))?;
        let provider = storage::providers::get_by_id(&conn, &id)?;
        (
            provider.base_url,
            provider.api_key,
            provider.timeout_seconds,
        )
    };

    let (provider_type, extra_headers) = {
        let conn = state
            .db
            .get()
            .map_err(|_| AppError::internal("DB lock failed"))?;
        let p = storage::providers::get_by_id(&conn, &id)?;
        (p.provider_type, p.extra_headers)
    };

    let api_key = match api_key {
        Some(k) if !k.is_empty() => {
            // Multi-key field stores JSON array `["sk-a","sk-b"]`; pick the
            // first non-empty entry — sending the raw `[...]` string as a
            // bearer token would 401. Mirrors detect_provider_cache.
            let trimmed = k.trim();
            if trimmed.starts_with('[') {
                serde_json::from_str::<Vec<String>>(trimmed)
                    .ok()
                    .and_then(|v| v.into_iter().find(|s| !s.is_empty()))
                    .unwrap_or_else(|| trimmed.to_string())
            } else {
                trimmed.to_string()
            }
        }
        _ => {
            let raw = "API key is not set. Please configure your API key first.";
            return Ok(ProviderTestResult {
                success: false,
                status: "failed".to_string(),
                message: raw.to_string(),
                latency_ms: None,
                supports_vision: None,
                diagnostic: Some(crate::diagnostics::test_failure::TestDiagnostic {
                    code: "missing_api_key".to_string(),
                    title: "还没配置 API key".to_string(),
                    hint: "在「API key」字段粘贴 Provider 控制台拿到的 key 再测连接。".to_string(),
                    action_url: None,
                    action_label: None,
                    raw: raw.to_string(),
                }),
            });
        }
    };

    let client = crate::gateway::http_client::apply_optional_proxy(
        reqwest::Client::builder().timeout(std::time::Duration::from_secs(timeout_seconds as u64)),
        crate::gateway::http_client::proxy_from_db(&state.db),
    )
    .build()
    .map_err(|e| AppError::internal(format!("Failed to create HTTP client: {e}")))?;

    let urls = vec![
        format!("{}/models", base_url.trim_end_matches('/')),
        format!("{}/v1/models", base_url.trim_end_matches('/')),
    ];

    let start = Instant::now();
    let mut last_error = String::new();
    let mut last_status: Option<u16> = None;
    let mut last_body = String::new();

    for url in &urls {
        // 必须复用 provider 的 extra_headers——Kimi catalog 默认注入
        // `User-Agent: KimiCLI/1.40.0`，Moonshot 服务端对部分 plan key
        // 做 UA 校验，UA 不对一律 401，看起来像 "key 无效" 但其实是 UA。
        // 同理给 Anthropic-beta 等自定义 header 留口子。
        let mut req_builder = client
            .get(url)
            .header("Authorization", format!("Bearer {api_key}"));
        if let Some(ref eh) = extra_headers {
            if let Ok(headers) =
                serde_json::from_str::<std::collections::HashMap<String, String>>(eh)
            {
                for (k, v) in headers {
                    if let (Ok(name), Ok(val)) = (
                        reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                        reqwest::header::HeaderValue::from_str(&v),
                    ) {
                        req_builder = req_builder.header(name, val);
                    }
                }
            }
        }
        let result = req_builder.send().await;

        match result {
            Ok(resp) if resp.status().is_success() => {
                let latency = start.elapsed().as_millis() as u64;
                {
                    let conn = state
                        .db
                        .get()
                        .map_err(|_| AppError::internal("DB lock failed"))?;
                    storage::providers::update_status(&conn, &id, "connected")?;
                    let _ = storage::recommended_mappings::supplement_provider(
                        &conn,
                        &id,
                        storage::recommended_mappings::MappingProfile::All,
                    );
                }
                return Ok(ProviderTestResult {
                    success: true,
                    status: "connected".to_string(),
                    message: format!("Connection successful via {url}"),
                    latency_ms: Some(latency),
                    supports_vision: None,
                    diagnostic: None,
                });
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                let sanitized = body.replace(&api_key, "sk-***REDACTED***");
                last_status = Some(status.as_u16());
                last_body = sanitized.clone();
                last_error = format!("HTTP {status}: {sanitized}");
            }
            Err(e) => {
                last_status = None;
                last_body.clear();
                last_error = format!("Connection error: {e}");
            }
        }
    }

    {
        let conn = state
            .db
            .get()
            .map_err(|_| AppError::internal("DB lock failed"))?;
        storage::providers::update_status(&conn, &id, "failed")?;
    }

    let latency = start.elapsed().as_millis() as u64;
    let diagnostic = Some(crate::diagnostics::test_failure::diagnose(
        &provider_type,
        last_status,
        &last_body,
        &last_error,
    ));
    Ok(ProviderTestResult {
        success: false,
        status: "failed".to_string(),
        message: last_error,
        latency_ms: Some(latency),
        supports_vision: None,
        diagnostic,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn detect_provider_vision(
    id: String,
    state: State<'_, AppState>,
) -> Result<ProviderTestResult, AppError> {
    let provider = {
        let conn = state
            .db
            .get()
            .map_err(|_| AppError::internal("DB lock failed"))?;
        storage::providers::get_by_id(&conn, &id)?
    };

    let api_key = match provider.api_key {
        Some(ref k) if !k.is_empty() => k.clone(),
        _ => {
            return Ok(ProviderTestResult {
                success: false,
                status: "failed".to_string(),
                message: "API key is not set".to_string(),
                latency_ms: None,
                supports_vision: None,
                diagnostic: None,
            });
        }
    };

    let client = crate::gateway::http_client::apply_optional_proxy(
        reqwest::Client::builder().timeout(std::time::Duration::from_secs(
            provider.timeout_seconds as u64,
        )),
        crate::gateway::http_client::proxy_from_db(&state.db),
    )
    .build()
    .map_err(|e| AppError::internal(format!("HTTP client error: {e}")))?;

    // 1x1 red PNG, ~68 bytes base64
    let tiny_image = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg==";

    let probe_body = serde_json::json!({
        "model": provider.default_model,
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": "hi"},
                {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{tiny_image}")}}
            ]
        }],
        "max_tokens": 1
    });

    let base = provider.base_url.trim_end_matches('/');
    let url = if provider.protocol == "anthropic_messages" {
        if let Some(ref abu) = provider.anthropic_base_url {
            format!("{}/v1/messages", abu.trim_end_matches('/'))
        } else {
            format!("{}/v1/messages", base)
        }
    } else {
        format!("{}/v1/chat/completions", base)
    };

    let start = Instant::now();

    let mut req_builder = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json");

    // Add extra headers if configured
    if let Some(ref eh) = provider.extra_headers {
        if let Ok(headers) = serde_json::from_str::<std::collections::HashMap<String, String>>(eh) {
            for (k, v) in headers {
                if let (Ok(name), Ok(val)) = (
                    reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                    reqwest::header::HeaderValue::from_str(&v),
                ) {
                    req_builder = req_builder.header(name, val);
                }
            }
        }
    }

    let result = req_builder.json(&probe_body).send().await;

    let latency = start.elapsed().as_millis() as u64;

    // Vision detection by status code:
    //   2xx                → supported (model accepted image input)
    //   401 / 403          → inconclusive (auth issue, can't tell capability)
    //   5xx, network error → inconclusive (server-side issue)
    //   any other 4xx      → NOT supported (model rejected image input)
    //
    // The "any 4xx" rule is critical: providers reject image input with
    // different status codes — OpenAI/DeepSeek use 400 ("invalid image"),
    // MiMo's pro/flash variants use 404 ("No endpoints found that support
    // image input"), some return 422. Treating only 400 as "not supported"
    // (the previous logic) false-positived MiMo's 404 as supports_vision.
    let supports_vision: Option<bool> = match result {
        Ok(resp) => {
            let code = resp.status().as_u16();
            if resp.status().is_success() {
                Some(true)
            } else if code == 401 || code == 403 || code >= 500 {
                None
            } else {
                Some(false)
            }
        }
        Err(_) => None,
    };

    // Persist only when conclusive — keep stale state out of the DB on
    // auth/server errors so the user retries with a fresh signal.
    if let Some(v) = supports_vision {
        let conn = state
            .db
            .get()
            .map_err(|_| AppError::internal("DB lock failed"))?;
        storage::providers::update_supports_vision(&conn, &id, v)?;
    }

    let (status_str, message) = match supports_vision {
        Some(true) => ("detected", "Vision supported".to_string()),
        Some(false) => ("detected", "Vision not supported".to_string()),
        None => (
            "inconclusive",
            "Vision detection inconclusive (auth / server error / network)".to_string(),
        ),
    };

    Ok(ProviderTestResult {
        success: supports_vision.is_some(),
        status: status_str.to_string(),
        message,
        latency_ms: Some(latency),
        supports_vision,
        diagnostic: None,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use r2d2::Pool;
    use r2d2_sqlite::SqliteConnectionManager;

    use super::*;
    use crate::app::state::AppState;
    use crate::models::provider::{CreateProviderInput, UpdateProviderInput};

    fn test_state() -> AppState {
        let manager = SqliteConnectionManager::memory();
        let pool = Pool::builder().max_size(1).build(manager).unwrap();
        {
            let conn = pool.get().unwrap();
            crate::storage::migrations::run_migrations(&conn).unwrap();
        }
        AppState {
            db: pool,
            gateway_runtime: Arc::new(Mutex::new(
                crate::models::gateway::GatewayRuntimeState::default(),
            )),
            wake: crate::wake::WakeManager::new(),
            pet_click_through: Arc::new(Mutex::new(false)),
        }
    }

    unsafe fn as_state<'r>(state: &'r AppState) -> tauri::State<'r, AppState> {
        std::mem::transmute(state)
    }

    fn sample_create_input() -> CreateProviderInput {
        CreateProviderInput {
            name: "Test Provider".to_string(),
            provider_type: "openai".to_string(),
            base_url: "https://api.openai.com".to_string(),
            api_key: Some("sk-test".to_string()),
            default_model: "gpt-4".to_string(),
            protocol: r#"["openai_chat_completions"]"#.to_string(),
            timeout_seconds: Some(120),
            ..Default::default()
        }
    }

    #[test]
    fn seed_model_capabilities_returns_sensible_defaults() {
        let caps = seed_model_capabilities(
            "openai".to_string(),
            vec!["gpt-4".to_string(), "gpt-3.5-turbo".to_string()],
        )
        .unwrap();
        assert!(caps.contains_key("gpt-4"));
        assert!(caps.contains_key("gpt-3.5-turbo"));
    }

    #[test]
    fn list_providers_includes_defaults() {
        let state = test_state();
        let providers = list_providers(unsafe { as_state(&state) }).unwrap();
        assert!(!providers.is_empty());
    }

    #[test]
    fn create_provider_validates_empty_name() {
        let state = test_state();
        let mut input = sample_create_input();
        input.name = "   ".to_string();
        let err = create_provider(input, unsafe { as_state(&state) }).unwrap_err();
        assert_eq!(err.code, "VALIDATION_ERROR");
    }

    #[test]
    fn create_provider_validates_empty_base_url() {
        let state = test_state();
        let mut input = sample_create_input();
        input.base_url = "".to_string();
        let err = create_provider(input, unsafe { as_state(&state) }).unwrap_err();
        assert_eq!(err.code, "VALIDATION_ERROR");
    }

    #[test]
    fn create_provider_validates_empty_default_model() {
        let state = test_state();
        let mut input = sample_create_input();
        input.default_model = "".to_string();
        let err = create_provider(input, unsafe { as_state(&state) }).unwrap_err();
        assert_eq!(err.code, "VALIDATION_ERROR");
    }

    #[test]
    fn create_provider_validates_non_positive_timeout() {
        let state = test_state();
        let mut input = sample_create_input();
        input.timeout_seconds = Some(0);
        let err = create_provider(input, unsafe { as_state(&state) }).unwrap_err();
        assert_eq!(err.code, "VALIDATION_ERROR");
    }

    #[test]
    fn create_provider_persists_and_returns_view() {
        let state = test_state();
        let view = create_provider(sample_create_input(), unsafe { as_state(&state) }).unwrap();
        assert_eq!(view.name, "Test Provider");
        assert_eq!(view.default_model, "gpt-4");

        let fetched = get_provider(view.id.clone(), unsafe { as_state(&state) }).unwrap();
        assert_eq!(fetched.id, view.id);
        assert_eq!(fetched.name, "Test Provider");
    }

    #[test]
    fn get_provider_keys_returns_empty_when_unset() {
        let state = test_state();
        let view = create_provider(sample_create_input(), unsafe { as_state(&state) }).unwrap();
        let keys = get_provider_keys(view.id, unsafe { as_state(&state) }).unwrap();
        assert_eq!(keys, vec!["sk-test"]);
    }

    #[test]
    fn get_provider_keys_parses_json_array() {
        let state = test_state();
        let mut input = sample_create_input();
        input.api_key = Some(r#"["sk-a", "sk-b"]"#.to_string());
        let view = create_provider(input, unsafe { as_state(&state) }).unwrap();
        let keys = get_provider_keys(view.id, unsafe { as_state(&state) }).unwrap();
        assert_eq!(keys, vec!["sk-a", "sk-b"]);
    }

    #[test]
    fn get_provider_returns_not_found_for_missing() {
        let state = test_state();
        let err = get_provider("no-such-id".to_string(), unsafe { as_state(&state) }).unwrap_err();
        assert_eq!(err.code, "NOT_FOUND");
    }

    #[test]
    fn update_provider_changes_name() {
        let state = test_state();
        let view = create_provider(sample_create_input(), unsafe { as_state(&state) }).unwrap();
        let updated = update_provider(
            view.id.clone(),
            UpdateProviderInput {
                name: Some("Renamed".to_string()),
                ..Default::default()
            },
            unsafe { as_state(&state) },
        )
        .unwrap();
        assert_eq!(updated.name, "Renamed");

        let fetched = get_provider(view.id, unsafe { as_state(&state) }).unwrap();
        assert_eq!(fetched.name, "Renamed");
    }

    #[test]
    fn update_provider_validates_empty_name() {
        let state = test_state();
        let view = create_provider(sample_create_input(), unsafe { as_state(&state) }).unwrap();
        let err = update_provider(
            view.id,
            UpdateProviderInput {
                name: Some("".to_string()),
                ..Default::default()
            },
            unsafe { as_state(&state) },
        )
        .unwrap_err();
        assert_eq!(err.code, "VALIDATION_ERROR");
    }

    #[test]
    fn delete_provider_removes_record() {
        let state = test_state();
        let view = create_provider(sample_create_input(), unsafe { as_state(&state) }).unwrap();
        let deleted = delete_provider(view.id.clone(), unsafe { as_state(&state) }).unwrap();
        assert!(deleted);
        let err = get_provider(view.id, unsafe { as_state(&state) }).unwrap_err();
        assert_eq!(err.code, "NOT_FOUND");
    }

    #[test]
    fn autofill_provider_capabilities_fills_missing_models() {
        let state = test_state();
        let view = create_provider(sample_create_input(), unsafe { as_state(&state) }).unwrap();
        let filled =
            autofill_provider_capabilities(view.id.clone(), unsafe { as_state(&state) }).unwrap();
        assert_eq!(filled, 1);

        let filled_again =
            autofill_provider_capabilities(view.id, unsafe { as_state(&state) }).unwrap();
        assert_eq!(filled_again, 0);
    }
}

#[tauri::command]
#[specta::specta]
pub async fn detect_provider_cache(
    id: String,
    state: State<'_, AppState>,
) -> Result<ProviderTestResult, AppError> {
    let provider = {
        let conn = state
            .db
            .get()
            .map_err(|_| AppError::internal("DB lock failed"))?;
        storage::providers::get_by_id(&conn, &id)?
    };

    // Cache probe only works for providers with anthropic_base_url or anthropic type
    let anthropic_url =
        if provider.provider_type == "anthropic" || provider.provider_type == "claude" {
            let base = provider.base_url.trim_end_matches('/');
            Some(crate::providers::adapter::smart_append_path(
                base,
                "/messages",
            ))
        } else if let Some(ref abu) = provider.anthropic_base_url {
            if !abu.is_empty() {
                Some(crate::providers::adapter::smart_append_path(
                    abu,
                    "/messages",
                ))
            } else {
                None
            }
        } else {
            None
        };

    let url = match anthropic_url {
        Some(u) => u,
        None => {
            return Ok(ProviderTestResult {
                success: false,
                status: "skipped".to_string(),
                message:
                    "Cache probe only works for Anthropic or providers with Anthropic endpoint"
                        .to_string(),
                latency_ms: None,
                supports_vision: None,
                diagnostic: None,
            });
        }
    };

    let api_key = match provider.api_key {
        Some(ref k) if !k.is_empty() => {
            // Parse multi-key, use first
            let trimmed = k.trim();
            if trimmed.starts_with('[') {
                serde_json::from_str::<Vec<String>>(trimmed)
                    .ok()
                    .and_then(|v| v.into_iter().find(|s| !s.is_empty()))
                    .unwrap_or_else(|| trimmed.to_string())
            } else {
                trimmed.to_string()
            }
        }
        _ => {
            return Ok(ProviderTestResult {
                success: false,
                status: "failed".to_string(),
                message: "API key is not set".to_string(),
                latency_ms: None,
                supports_vision: None,
                diagnostic: None,
            });
        }
    };

    // Cache 探测每次跑 2 次 HTTP，timeout 硬上限 15s：
    // 上游若不支持 Anthropic 格式（如错配了 anthropic_base_url 的 OpenAI 系 provider），
    // 否则会卡满 provider 默认 timeout（往往 120s+），前端 dialog 永远在转。
    let probe_timeout = std::cmp::min(provider.timeout_seconds as u64, 15);
    let client = crate::gateway::http_client::apply_optional_proxy(
        reqwest::Client::builder().timeout(std::time::Duration::from_secs(probe_timeout)),
        crate::gateway::http_client::proxy_from_db(&state.db),
    )
    .build()
    .map_err(|e| AppError::internal(format!("HTTP client error: {e}")))?;

    // Build a large enough system prompt (>1024 tokens for cache eligibility)
    let long_system = "You are a helpful assistant. ".repeat(100); // ~600 words, >1024 tokens
    let probe_body = serde_json::json!({
        "model": provider.default_model,
        "system": [{"type": "text", "text": long_system, "cache_control": {"type": "ephemeral"}}],
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 1
    });

    let is_anthropic_type =
        provider.provider_type == "anthropic" || provider.provider_type == "claude";

    let build_req = |client: &reqwest::Client| {
        let mut rb = client.post(&url).header("Content-Type", "application/json");
        if is_anthropic_type {
            rb = rb
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01");
        } else {
            rb = rb.header("Authorization", format!("Bearer {api_key}"));
        }
        if let Some(ref eh) = provider.extra_headers {
            if let Ok(headers) =
                serde_json::from_str::<std::collections::HashMap<String, String>>(eh)
            {
                for (k, v) in headers {
                    if let (Ok(name), Ok(val)) = (
                        reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                        reqwest::header::HeaderValue::from_str(&v),
                    ) {
                        rb = rb.header(name, val);
                    }
                }
            }
        }
        rb.json(&probe_body)
    };

    let start = Instant::now();

    // First request: creates cache
    let resp1 = build_req(&client).send().await;
    if let Err(e) = resp1 {
        return Ok(ProviderTestResult {
            success: false,
            status: "failed".to_string(),
            message: format!("Cache probe request 1 failed: {e}"),
            latency_ms: Some(start.elapsed().as_millis() as u64),
            supports_vision: None,
            diagnostic: None,
        });
    }
    let resp1 = resp1.unwrap();
    if !resp1.status().is_success() {
        let body = resp1.text().await.unwrap_or_default();
        let sanitized = body.replace(&api_key, "sk-***REDACTED***");
        return Ok(ProviderTestResult {
            success: false,
            status: "failed".to_string(),
            message: format!(
                "Cache probe request 1 HTTP error: {}",
                &sanitized[..sanitized.len().min(500)]
            ),
            latency_ms: Some(start.elapsed().as_millis() as u64),
            supports_vision: None,
            diagnostic: None,
        });
    }
    // Consume body
    let _ = resp1.text().await;

    // Second request: should hit cache
    let resp2 = build_req(&client).send().await;
    let latency = start.elapsed().as_millis() as u64;

    let supports_cache = match resp2 {
        Ok(resp) if resp.status().is_success() => {
            let body = resp.text().await.unwrap_or_default();
            // Check for cache_read_input_tokens > 0
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                let cache_read = json
                    .get("usage")
                    .and_then(|u| u.get("cache_read_input_tokens"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                cache_read > 0
            } else {
                false
            }
        }
        _ => false,
    };

    // Save result
    {
        let conn = state
            .db
            .get()
            .map_err(|_| AppError::internal("DB lock failed"))?;
        storage::providers::update_supports_cache(&conn, &id, supports_cache)?;
    }

    let message = if supports_cache {
        "Cache supported — cache_read_input_tokens > 0 on second request".to_string()
    } else {
        "Cache not detected — cache_read_input_tokens was 0 on second request".to_string()
    };

    Ok(ProviderTestResult {
        success: true,
        status: "detected".to_string(),
        message,
        latency_ms: Some(latency),
        supports_vision: None, // reuse struct, this field unused here
        diagnostic: None,
    })
}
// ── Provider Health Commands ──────────────────────────────────

#[tauri::command]
#[specta::specta]
pub fn get_provider_health(
    state: State<'_, AppState>,
    provider: String,
) -> Result<crate::storage::request_logs::ProviderHealth, AppError> {
    let conn = state
        .db
        .get()
        .map_err(|_| AppError::internal("DB lock failed"))?;
    crate::storage::request_logs::get_provider_health(&conn, &provider)
}

// ── Tool Connection Test ──────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn test_tool_connection(
    state: State<'_, AppState>,
) -> Result<serde_json::Value, AppError> {
    // Step 1: Check gateway is running
    let (running, host, port) = {
        let runtime = state
            .gateway_runtime
            .lock()
            .map_err(|_| AppError::internal("Runtime lock failed"))?;
        (runtime.running, runtime.host.clone(), runtime.port)
    };

    if !running {
        return Ok(serde_json::json!({
            "config_ok": true,
            "gateway_ok": false,
            "provider_ok": false,
            "error": "Gateway not running",
        }));
    }

    // Step 2: Check gateway health
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| AppError::internal(format!("HTTP client error: {e}")))?;

    let health_url = format!("http://{}:{}/health", host, port);
    let gateway_ok = client
        .get(&health_url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    if !gateway_ok {
        return Ok(serde_json::json!({
            "config_ok": true,
            "gateway_ok": false,
            "provider_ok": false,
            "error": "Gateway health check failed",
        }));
    }

    // Step 3: Test provider with a minimal request
    let test_model = {
        let conn = state
            .db
            .get()
            .map_err(|_| AppError::internal("DB lock failed"))?;
        let route_model =
            storage::route_profiles::get_default_for_protocol(&conn, "openai_chat_completions")?
                .and_then(|profile| profile.active_provider_id)
                .and_then(|provider_id| storage::providers::get_by_id(&conn, &provider_id).ok())
                .map(|provider| provider.default_model);
        route_model
            .or_else(|| {
                storage::providers::list_all(&conn)
                    .ok()
                    .and_then(|providers| {
                        providers
                            .into_iter()
                            .find(|provider| provider.enabled && provider.is_active)
                            .map(|provider| provider.default_model)
                    })
            })
            .unwrap_or_else(|| "test".to_string())
    };
    let token = crate::security::local_token::read_token().unwrap_or_default();
    let test_url = format!("http://{}:{}/v1/chat/completions", host, port);
    let test_body = serde_json::json!({
        "model": test_model,
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 1,
    });

    let resp = client
        .post(&test_url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&test_body)
        .send()
        .await;

    let (provider_ok, error) = match resp {
        Ok(r) => {
            let status = r.status();
            if status.is_success() || status.as_u16() == 200 {
                (true, None)
            } else {
                let body = r.text().await.unwrap_or_default();
                // 400 with model error is ok — means provider is reachable
                if status.as_u16() == 400 || status.as_u16() == 404 {
                    (true, None)
                } else {
                    (
                        false,
                        Some(format!(
                            "Provider error: {} {}",
                            status.as_u16(),
                            body.chars().take(100).collect::<String>()
                        )),
                    )
                }
            }
        }
        Err(e) => (false, Some(format!("Request failed: {e}"))),
    };

    // Step 4: 已接入 AgentGate 的客户端是否有进程在跑。
    // 不能读进程 env 判断「是否热加载了配置」——能做的是：配置已写盘 + 进程是否存活。
    // 进程在跑时提示用户「刚改过配置请重启」；未跑则提示启动客户端。
    let client_processes = probe_active_client_processes(&state);
    // 第四步是「探测完成」而非「进程必须在跑」：未运行只是提示启动，不判失败。
    // 无已接入客户端时返回 null（UI 中性点）；有则 true。
    let client_process_ok = if client_processes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::Bool(true)
    };
    let client_processes_json: Vec<serde_json::Value> = client_processes
        .into_iter()
        .map(|(id, running, count)| {
            serde_json::json!({
                "client_id": id,
                "running": running,
                "count": count,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "config_ok": true,
        "gateway_ok": true,
        "provider_ok": provider_ok,
        "client_process_ok": client_process_ok,
        "client_processes": client_processes_json,
        "test_model": test_body.get("model").and_then(|v| v.as_str()).unwrap_or("test"),
        "error": error,
    }))
}

/// 返回 (client_id, running, process_count)。只列「配置侧已接入 AgentGate」的客户端。
fn probe_active_client_processes(_state: &State<'_, AppState>) -> Vec<(String, bool, usize)> {
    let mut out = Vec::new();
    let pairs: [(&str, &str, bool); 5] = [
        (
            "codex",
            "codex",
            crate::tools::codex::detect().has_agentgate,
        ),
        (
            "claude_code",
            "claude",
            crate::tools::claude_code::detect_env().has_agentgate,
        ),
        (
            "opencode",
            "opencode",
            crate::tools::opencode::detect().has_agentgate,
        ),
        (
            "gemini",
            "gemini",
            crate::tools::gemini_cli::detect().has_agentgate,
        ),
        (
            "atomcode",
            "atomcode",
            crate::tools::atomcode::detect().has_agentgate,
        ),
    ];
    for (client_id, needle, active) in pairs {
        if !active {
            continue;
        }
        let procs = crate::tools::process_detect::find_running(&[needle]);
        out.push((client_id.to_string(), !procs.is_empty(), procs.len()));
    }
    out
}
