use std::collections::HashMap;

use rusqlite::Connection;
use serde::Deserialize;
use serde_json::Value;

use crate::errors::AppError;
use crate::models::provider::Provider;
use crate::models::route_profile::RouteProfileProviderView;
use crate::protocol::openai_responses::ResponsesRequest;
use crate::storage;

/// The result of selecting a provider for a request.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ProviderSelection {
    pub route_profile_id: String,
    pub route_profile_name: String,
    pub mode: String,
    pub provider: Provider,
    pub model: String,
    pub priority: i64,
    pub reason: String,
    /// All candidates in order, for failover iteration
    pub candidates: Vec<ProviderCandidate>,
}

#[derive(Debug, Clone)]
pub struct ProviderCandidate {
    pub provider_id: String,
    pub provider_name: String,
    pub priority: i64,
    pub model: String,
    pub routing_conditions: Option<String>,
    pub in_cooldown: bool,
    pub supports_vision: Option<bool>,
    pub cooldown_seconds: i64,
    pub failover_on_status_codes: Vec<i64>,
    pub failover_on_error_keywords: Vec<String>,
}

/// Request characteristics for condition matching.
#[derive(Debug, Clone)]
pub struct RequestAnalysis {
    pub input_char_count: usize,
    pub has_images: bool,
    pub has_tools: bool,
    #[allow(dead_code)]
    pub tool_count: usize,
    pub system_text: String,
    #[allow(dead_code)]
    pub message_count: usize,
}

/// Analyze a ResponsesRequest to extract routing-relevant characteristics.
pub fn analyze_request(req: &ResponsesRequest) -> RequestAnalysis {
    // 保持 JSON 序列化长度：存量 min/max_input_chars 按这个口径配的。
    let input_char_count = req.input.to_string().len();

    let has_images = crate::gateway::routes::request_contains_images_pub(req);

    let has_tools = req.tools.as_ref().is_some_and(|t| !t.is_empty());
    let tool_count = req.tools.as_ref().map_or(0, |t| t.len());

    let system_text = req
        .instructions
        .clone()
        .or_else(|| req.system.clone())
        .unwrap_or_default();

    let message_count = match &req.input {
        Value::Array(items) => items
            .iter()
            .filter(|i| i.get("type").and_then(|t| t.as_str()) == Some("message"))
            .count(),
        _ => 1,
    };

    RequestAnalysis {
        input_char_count,
        has_images,
        has_tools,
        tool_count,
        system_text,
        message_count,
    }
}

/// Chat Completions (`messages` + `tools`) 请求体分析。
pub fn analyze_chat_value(body: &Value) -> RequestAnalysis {
    let messages = body.get("messages").and_then(|m| m.as_array());
    let mut input_char_count = 0;
    let mut system_text = String::new();
    let message_count = messages.map_or(0, |m| m.len());
    if let Some(msgs) = messages {
        for m in msgs {
            let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("");
            collect_text_content(
                m.get("content"),
                &mut input_char_count,
                role == "system",
                &mut system_text,
            );
        }
    }
    let tools = body.get("tools").and_then(|t| t.as_array());
    RequestAnalysis {
        input_char_count,
        has_images: crate::gateway::routes::chat_request_has_images_value(body),
        has_tools: tools.is_some_and(|t| !t.is_empty()),
        tool_count: tools.map_or(0, |t| t.len()),
        system_text,
        message_count,
    }
}

/// Anthropic Messages (`system` + `messages` + `tools`) 请求体分析。
pub fn analyze_messages_value(body: &Value) -> RequestAnalysis {
    let mut input_char_count = 0;
    let mut system_text = String::new();
    collect_text_content(
        body.get("system"),
        &mut input_char_count,
        true,
        &mut system_text,
    );
    let messages = body.get("messages").and_then(|m| m.as_array());
    let message_count = messages.map_or(0, |m| m.len());
    if let Some(msgs) = messages {
        for m in msgs {
            collect_text_content(
                m.get("content"),
                &mut input_char_count,
                false,
                &mut system_text,
            );
        }
    }
    let tools = body.get("tools").and_then(|t| t.as_array());
    RequestAnalysis {
        input_char_count,
        has_images: crate::gateway::routes::anthropic_request_has_images_value(body),
        has_tools: tools.is_some_and(|t| !t.is_empty()),
        tool_count: tools.map_or(0, |t| t.len()),
        system_text,
        message_count,
    }
}

/// Gemini `generateContent` 请求体分析。
pub fn analyze_gemini_value(body: &Value) -> RequestAnalysis {
    let mut input_char_count = 0;
    let mut system_text = String::new();
    if let Some(sys) = body
        .get("systemInstruction")
        .or_else(|| body.get("system_instruction"))
    {
        collect_gemini_parts(
            sys.get("parts"),
            &mut input_char_count,
            true,
            &mut system_text,
        );
    }
    let contents = body.get("contents").and_then(|c| c.as_array());
    let message_count = contents.map_or(0, |c| c.len());
    let mut has_images = false;
    if let Some(items) = contents {
        for (i, c) in items.iter().enumerate() {
            let is_last_user = i + 1 == items.len()
                && c.get("role")
                    .and_then(|r| r.as_str())
                    .is_none_or(|r| r == "user");
            if let Some(parts) = c.get("parts").and_then(|p| p.as_array()) {
                for part in parts {
                    if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                        input_char_count += t.len();
                    }
                    if is_last_user
                        && (part.get("inline_data").is_some()
                            || part.get("inlineData").is_some()
                            || part.get("file_data").is_some()
                            || part.get("fileData").is_some())
                    {
                        has_images = true;
                    }
                }
            }
        }
    }
    let tools = body.get("tools").and_then(|t| t.as_array());
    RequestAnalysis {
        input_char_count,
        has_images,
        has_tools: tools.is_some_and(|t| !t.is_empty()),
        tool_count: tools.map_or(0, |t| t.len()),
        system_text,
        message_count,
    }
}

fn collect_text_content(
    content: Option<&Value>,
    chars: &mut usize,
    into_system: bool,
    system_text: &mut String,
) {
    match content {
        Some(Value::String(s)) => {
            *chars += s.len();
            if into_system {
                system_text.push_str(s);
            }
        }
        Some(Value::Array(parts)) => {
            for p in parts {
                if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
                    *chars += t.len();
                    if into_system {
                        system_text.push_str(t);
                    }
                } else if let Some(s) = p.as_str() {
                    *chars += s.len();
                    if into_system {
                        system_text.push_str(s);
                    }
                }
            }
        }
        Some(other) => {
            if let Some(t) = other.get("text").and_then(|t| t.as_str()) {
                *chars += t.len();
                if into_system {
                    system_text.push_str(t);
                }
            }
        }
        None => {}
    }
}

fn collect_gemini_parts(
    parts: Option<&Value>,
    chars: &mut usize,
    into_system: bool,
    system_text: &mut String,
) {
    collect_text_content(parts, chars, into_system, system_text);
}

/// Routing conditions that can be attached to a provider in a route profile.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct RoutingConditions {
    pub min_input_chars: Option<usize>,
    pub max_input_chars: Option<usize>,
    pub has_images: Option<bool>,
    pub has_tools: Option<bool>,
    pub system_keywords: Option<Vec<String>>,
    pub model_override: Option<String>,
    /// 客户端请求的 model 名匹配(子串,大小写不敏感)。命中任一即满足。
    /// 仅依赖 model 名 → 三协议(Claude Code / Codex / Gemini)全部生效。
    /// 典型用法:`["haiku"]` 把 Claude Code 的 background 小任务导到便宜 provider。
    pub model_name_match: Option<Vec<String>>,
}

/// model_name_match 评估:只依赖客户端请求的 model 名,与协议无关,三协议都能用。
/// 未配置该条件时恒为 true(不约束)。
fn model_name_matches(conditions: &RoutingConditions, requested_model: Option<&str>) -> bool {
    match &conditions.model_name_match {
        Some(names) if !names.is_empty() => {
            let m = requested_model.unwrap_or("").to_lowercase();
            names.iter().any(|n| m.contains(&n.to_lowercase()))
        }
        _ => true,
    }
}

/// Check if all non-null conditions match the request analysis.
fn matches_conditions(conditions: &RoutingConditions, analysis: &RequestAnalysis) -> bool {
    if let Some(min) = conditions.min_input_chars {
        if analysis.input_char_count < min {
            return false;
        }
    }
    if let Some(max) = conditions.max_input_chars {
        if analysis.input_char_count > max {
            return false;
        }
    }
    if let Some(img) = conditions.has_images {
        if analysis.has_images != img {
            return false;
        }
    }
    if let Some(tools) = conditions.has_tools {
        if analysis.has_tools != tools {
            return false;
        }
    }
    if let Some(ref keywords) = conditions.system_keywords {
        if !keywords.is_empty() {
            let lower = analysis.system_text.to_lowercase();
            if !keywords.iter().any(|kw| lower.contains(&kw.to_lowercase())) {
                return false;
            }
        }
    }
    true
}

/// 一次查出全部 provider 建索引。候选构建 / 选中 provider 都从这里取,
/// 不再每个候选单独 `get_by_id`。
fn load_provider_map(conn: &Connection) -> Result<HashMap<String, Provider>, AppError> {
    Ok(storage::providers::list_all(conn)?
        .into_iter()
        .map(|p| (p.id.clone(), p))
        .collect())
}

fn build_candidates(
    providers: &HashMap<String, Provider>,
    rp_providers: &[RouteProfileProviderView],
    requested_model: Option<&str>,
    analysis: Option<&RequestAnalysis>,
) -> Result<Vec<ProviderCandidate>, AppError> {
    let mut candidates = Vec::new();

    for rpp in rp_providers {
        if !rpp.enabled {
            continue;
        }

        // 路由条件评估。model_name_match 只依赖 model 名;其余条件依赖请求体分析。
        let mut condition_model_override: Option<String> = None;
        if let Some(ref cond_json) = rpp.routing_conditions {
            match serde_json::from_str::<RoutingConditions>(cond_json) {
                Ok(conditions) => {
                    if !model_name_matches(&conditions, requested_model) {
                        continue;
                    }
                    // Chat / Messages / Gemini / Responses 都会传入 analysis。
                    // 解析失败或调用方没给 analysis 时不误杀（只靠 model_name_match）。
                    if let Some(req_analysis) = analysis {
                        if !matches_conditions(&conditions, req_analysis) {
                            continue;
                        }
                    }
                    condition_model_override = conditions.model_override.clone();
                }
                Err(_) => continue,
            }
        }

        // Model resolution: condition_model_override → model_override → model_mapping → supported_models → default_model
        let provider_info = providers.get(&rpp.provider_id);
        if provider_info.as_ref().is_some_and(|p| !p.enabled) {
            continue;
        }

        let model = condition_model_override
            .or_else(|| rpp.model_override.clone())
            .unwrap_or_else(|| {
                if let Some(p) = provider_info {
                    if let Some(req) = requested_model {
                        return p.resolve_model(req);
                    }
                    p.default_model.clone()
                } else {
                    String::new()
                }
            });

        // Capability-aware model promotion: if request demands a capability
        // (e.g. vision) the resolved model lacks but another model on the same
        // provider has, swap to that model. Only fires when model_capabilities
        // matrix is populated; otherwise we leave the resolved model alone.
        let model = if let (Some(p), Some(req_analysis)) = (provider_info, analysis) {
            promote_for_capabilities(p, &model, req_analysis)
        } else {
            model
        };

        let in_cooldown = rpp.cooldown_until.as_ref().is_some_and(|until| {
            chrono::DateTime::parse_from_rfc3339(until)
                .map(|cd| cd > chrono::Utc::now())
                .unwrap_or(false)
        });

        let status_codes: Vec<i64> = rpp
            .failover_on_status_codes
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_else(|| vec![402, 429, 500, 502, 503, 504]);

        let keywords: Vec<String> = rpp
            .failover_on_error_keywords
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        // supports_vision precedence:
        //   1. model_capabilities matrix (any model declares "vision") — MOST ACCURATE,
        //      reflects per-model reality. Wins over the legacy single boolean.
        //   2. explicit per-provider flag (legacy probe result) — only when matrix unset.
        //   3. None — unknown, no opinion.
        //
        // Earlier the order was flipped, which caused MiMo providers where the legacy
        // probe wrote `supports_vision=false` (because mimo-v2.5-pro 404'd on image) to
        // be skipped from image requests entirely — even though the matrix declared
        // mimo-v2.5 / mimo-v2-omni as vision-capable. The promotion step (below) would
        // never run because the provider was filtered out in routes.rs:114 first.
        let supports_vision = provider_info
            .and_then(|p| {
                let caps = p.parse_capabilities();
                if caps.is_empty() {
                    None
                } else {
                    Some(caps.values().any(|c| {
                        c.iter()
                            .any(|x| x == crate::providers::capabilities::CAP_VISION)
                    }))
                }
            })
            .or_else(|| provider_info.and_then(|p| p.supports_vision));

        candidates.push(ProviderCandidate {
            provider_id: rpp.provider_id.clone(),
            provider_name: rpp.provider_name.clone(),
            priority: rpp.priority,
            model,
            routing_conditions: rpp.routing_conditions.clone(),
            in_cooldown,
            supports_vision,
            cooldown_seconds: rpp.cooldown_seconds,
            failover_on_status_codes: status_codes,
            failover_on_error_keywords: keywords,
        });
    }

    Ok(candidates)
}

/// 按 route profile 的 selection_strategy 对 failover 候选稳定排序。
/// "cheapest"：模型单价(input+output)升序；"fastest"：近 24h 平均延迟升序；
/// 其它（含 "priority"）：保持手工顺序不动。查不到价格/延迟的候选排末尾，
/// 平手按原 priority。
/// Re-order candidates by unit price (public for daily budget force_cheapest).
pub fn force_cheapest_order(conn: &Connection, candidates: &mut [ProviderCandidate]) {
    sort_candidates_by_strategy(conn, candidates, "cheapest");
}

fn sort_candidates_by_strategy(
    conn: &Connection,
    candidates: &mut [ProviderCandidate],
    strategy: &str,
) {
    match strategy {
        "cheapest" => {
            // 价格键先算好再排序:之前在比较器里查库,n 个候选要 O(n log n) 次 SQL。
            let mut keyed: Vec<(f64, ProviderCandidate)> = candidates
                .iter()
                .map(|c| (candidate_unit_cost(conn, c), c.clone()))
                .collect();
            keyed.sort_by(|(ca, a), (cb, b)| {
                ca.partial_cmp(cb)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.priority.cmp(&b.priority))
            });
            for (slot, (_, c)) in candidates.iter_mut().zip(keyed) {
                *slot = c;
            }
        }
        "fastest" => {
            let lat = recent_latency_by_provider(conn);
            // 冷启动/闲置 provider 没有请求延迟,用主动探测值兜底(默认关,
            // 见 probe_latency 模块说明),都没有才排末尾。
            let probes = crate::gateway::probe_latency::snapshot(
                crate::gateway::probe_latency::PROBE_STALE_MS,
            );
            candidates.sort_by(|a, b| {
                let la =
                    crate::gateway::probe_latency::resolve_latency(&lat, &probes, &a.provider_name);
                let lb =
                    crate::gateway::probe_latency::resolve_latency(&lat, &probes, &b.provider_name);
                la.partial_cmp(&lb)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.priority.cmp(&b.priority))
            });
        }
        _ => {}
    }
}

/// fastest 排序用的近 24h 平均延迟缓存有效期。聚合 request_logs 是全表范围扫描,
/// 每个请求都跑一遍太贵;30s 的滞后对延迟排序没有实际影响。
const LATENCY_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

type LatencyCache = Option<(std::time::Instant, HashMap<String, f64>)>;
static LATENCY_CACHE: std::sync::Mutex<LatencyCache> = std::sync::Mutex::new(None);

fn recent_latency_by_provider(conn: &Connection) -> HashMap<String, f64> {
    {
        let guard = LATENCY_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, map)) = guard.as_ref() {
            if at.elapsed() < LATENCY_CACHE_TTL {
                return map.clone();
            }
        }
    }
    let fresh = match storage::request_logs::avg_latency_by_provider(conn, 24) {
        Ok(map) => map,
        Err(e) => {
            // 查不到历史延迟时退化为只用探测值排序(与之前一致),但要留痕。
            tracing::warn!(error = %e, "avg_latency_by_provider failed; fastest falls back to probes");
            return HashMap::new();
        }
    };
    *LATENCY_CACHE.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((std::time::Instant::now(), fresh.clone()));
    fresh
}

/// 候选模型单价排序键：input+output 单价之和($/1M)。查不到价时返回 MAX 排末尾。
/// provider 用实例名，与成本计算/日志写入一致。
fn candidate_unit_cost(conn: &Connection, c: &ProviderCandidate) -> f64 {
    storage::pricing::get_price(conn, &c.provider_name, &c.model)
        .map(|(input, output)| input + output)
        .unwrap_or(f64::MAX)
}

fn select_global_fallback(
    conn: &Connection,
    requested_model: Option<&str>,
    analysis: Option<&RequestAnalysis>,
) -> Result<ProviderSelection, AppError> {
    let settings = storage::gateway_settings::get(conn)?;
    let provider_id = settings.active_provider_id.ok_or_else(|| {
        AppError::new(
            crate::errors::codes::ACTIVE_PROVIDER_NOT_FOUND,
            "No active provider configured",
        )
        .with_suggestion("Set an active provider in the Providers page")
    })?;

    let provider = storage::providers::get_by_id(conn, &provider_id)?;
    let model = match requested_model {
        Some(req) => provider.resolve_model(req),
        None => provider.default_model.clone(),
    };
    // Capability-aware promotion in fallback path too — mirrors the route_profile path.
    let model = if let Some(req_analysis) = analysis {
        promote_for_capabilities(&provider, &model, req_analysis)
    } else {
        model
    };

    Ok(ProviderSelection {
        route_profile_id: String::new(),
        route_profile_name: "Global Fallback".to_string(),
        mode: "manual".to_string(),
        provider,
        model,
        priority: 0,
        reason: "No route profile found, using global active provider".to_string(),
        candidates: vec![],
    })
}

/// Select provider for failover mode. Returns the ordered list of providers to try.
/// If `analysis` is provided, body-dependent routing conditions are evaluated.
pub fn select_for_failover(
    db: &crate::storage::db::DbPool,
    input_protocol: &str,
    requested_model: Option<&str>,
    analysis: Option<&RequestAnalysis>,
) -> Result<ProviderSelection, AppError> {
    let conn = db.get().map_err(|_| AppError::internal("DB lock failed"))?;
    select_for_failover_with_conn(&conn, input_protocol, requested_model, analysis)
}

/// 同 [`select_for_failover`],但复用调用方的连接——网关热路径在 blocking
/// 线程里一次借连接,把预算闸 / 选路 / 强制最便宜放进同一个任务。
pub fn select_for_failover_with_conn(
    conn: &Connection,
    input_protocol: &str,
    requested_model: Option<&str>,
    analysis: Option<&RequestAnalysis>,
) -> Result<ProviderSelection, AppError> {
    let profile = storage::route_profiles::get_default_for_protocol(conn, input_protocol)?;

    if let Some(profile) = profile {
        let rp_providers = storage::route_profiles::list_providers(conn, &profile.id)?;
        if rp_providers.is_empty() {
            return Err(AppError::new(
                crate::errors::codes::ROUTE_PROFILE_EMPTY,
                "Route profile has no providers",
            ));
        }

        let providers = load_provider_map(conn)?;
        let mut candidates =
            build_candidates(&providers, &rp_providers, requested_model, analysis)?;
        if candidates.is_empty() {
            return Err(AppError::new(
                crate::errors::codes::NO_PROVIDER_CANDIDATE,
                "No available provider candidate",
            ));
        }

        // Failover 模式按 selection_strategy 重排候选（cheapest/fastest）；
        // manual 模式按 active_id 选、priority 维持原序，都不需要重排。
        if profile.mode != "manual" && profile.selection_strategy != "priority" {
            sort_candidates_by_strategy(conn, &mut candidates, &profile.selection_strategy);
        }

        // Manual mode: use active_provider_id; Failover mode: first non-cooldown
        let (selected, reason) = if profile.mode == "manual" {
            if let Some(ref active_id) = profile.active_provider_id {
                if let Some(c) = candidates.iter().find(|c| c.provider_id == *active_id) {
                    (c, "Manual: active provider")
                } else {
                    (
                        &candidates[0],
                        "Manual: active not in candidates, using first",
                    )
                }
            } else {
                (&candidates[0], "Manual: no active set, using first")
            }
        } else {
            let c = candidates
                .iter()
                .find(|c| !c.in_cooldown)
                .unwrap_or(&candidates[0]);
            (c, "Failover: first available")
        };

        let provider = providers
            .get(&selected.provider_id)
            .cloned()
            .ok_or_else(|| AppError::not_found("Provider", &selected.provider_id))?;

        Ok(ProviderSelection {
            route_profile_id: profile.id,
            route_profile_name: profile.name,
            mode: profile.mode,
            provider,
            model: selected.model.clone(),
            priority: selected.priority,
            reason: reason.to_string(),
            candidates,
        })
    } else {
        select_global_fallback(conn, requested_model, analysis)
    }
}

/// Promote the resolved model to a sibling that has the capability the
/// request needs, if the current pick lacks it. Currently checks vision
/// (image input). Returns the original model when:
///   - the provider has no model_capabilities matrix configured,
///   - the current model already satisfies the request, or
///   - no sibling model satisfies the demanded capability.
fn promote_for_capabilities(
    provider: &Provider,
    current_model: &str,
    analysis: &RequestAnalysis,
) -> String {
    let caps_map = provider.parse_capabilities();
    if caps_map.is_empty() {
        return current_model.to_string();
    }

    // Strip any qualifier ([1m] etc.) before looking up in the matrix.
    let base = strip_qualifier(current_model);
    let current_caps: Vec<String> = caps_map.get(base).cloned().unwrap_or_default();
    let has = |c: &str| current_caps.iter().any(|x| x == c);

    if analysis.has_images && !has(crate::providers::capabilities::CAP_VISION) {
        if let Some(picked) = pick_best_substitute(
            provider,
            crate::providers::capabilities::CAP_VISION,
            &current_caps,
            &caps_map,
        ) {
            return picked;
        }
    }
    // Future: extend with audio_in / tts / etc. as more clients send them.

    current_model.to_string()
}

/// Pick the best vision-capable (or whatever-capable) substitute, preferring
/// the model that preserves the most of the original model's other capabilities.
/// Ties broken by `supported_models` order (first wins).
///
/// Example: original = mimo-v2.5-pro [reasoning, tools, web_search]; needed = vision.
/// Candidates: [mimo-v2-omni (text+vision+tools), mimo-v2.5 (text+vision+reasoning+tools+web_search)].
/// Original keeps 3 caps in v2.5 (tools+reasoning+web_search) vs 1 in omni (tools).
/// → pick v2.5 even though omni came first in the supported_models list.
fn pick_best_substitute(
    provider: &Provider,
    required: &str,
    original_caps: &[String],
    caps_map: &std::collections::HashMap<String, Vec<String>>,
) -> Option<String> {
    let candidates = provider.models_with_capability(required);
    if candidates.is_empty() {
        return None;
    }
    let original: std::collections::HashSet<&str> =
        original_caps.iter().map(|s| s.as_str()).collect();

    // Iterate in supported_models order so ties favor the user's listed priority.
    let mut best: Option<(String, usize)> = None;
    for model in candidates {
        let model_caps: std::collections::HashSet<&str> = caps_map
            .get(model.as_str())
            .map(|caps| caps.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default();
        let overlap = original.iter().filter(|c| model_caps.contains(*c)).count();
        // Strict > so the first model with this score wins (stable tie-break).
        if best.as_ref().is_none_or(|(_, score)| overlap > *score) {
            best = Some((model, overlap));
        }
    }
    best.map(|(m, _)| m)
}

fn strip_qualifier(model: &str) -> &str {
    if let Some(stripped) = model.strip_suffix(']') {
        if let Some(open) = stripped.rfind('[') {
            return &stripped[..open];
        }
    }
    model
}

/// Walk the per-provider `model_degradation_chain` for the given requested
/// model. Returns the *full ordered chain including the requested model
/// itself* (head = primary), so the failover loop can iterate without
/// having to track "did I try the original yet?" separately. Returns
/// just `[requested]` when the provider has no degradation chain configured
/// or the requested model has no fallbacks listed.
///
/// Example:
///   provider.model_degradation_chain = {"gpt-5-codex": ["gpt-5-mini","gpt-4o"]}
///   degradation_chain_for_model(provider, "gpt-5-codex")
///       → ["gpt-5-codex", "gpt-5-mini", "gpt-4o"]
///
/// Models not present in the chain (e.g. user requested "claude-sonnet-4"
/// against a provider with no entry for it) return a single-element vec
/// — the failover loop falls back to provider-level failover at that point.
pub fn degradation_chain_for_model(provider: &Provider, requested_model: &str) -> Vec<String> {
    let chain = provider.parse_degradation_chain();
    let mut result = vec![requested_model.to_string()];
    if let Some(fallbacks) = chain.get(requested_model) {
        for m in fallbacks {
            if m != requested_model && !result.contains(m) {
                result.push(m.clone());
            }
        }
    }
    result
}

pub fn route_decision_trace(selection: &ProviderSelection) -> Value {
    let selected_conditions = selection
        .candidates
        .iter()
        .find(|c| c.provider_id == selection.provider.id)
        .and_then(|c| c.routing_conditions.as_ref())
        .and_then(|s| serde_json::from_str::<Value>(s).ok());

    serde_json::json!({
        "route_decision": {
            "profile_id": selection.route_profile_id,
            "profile_name": selection.route_profile_name,
            "mode": selection.mode,
            "reason": selection.reason,
            "selected_provider_id": selection.provider.id,
            "selected_provider_name": selection.provider.name,
            "selected_model": selection.model,
            "selected_priority": selection.priority,
            "matched_conditions": selected_conditions,
            "candidates": selection.candidates.iter().map(|c| {
                serde_json::json!({
                    "provider_id": c.provider_id,
                    "provider_name": c.provider_name,
                    "priority": c.priority,
                    "model": c.model,
                    "in_cooldown": c.in_cooldown,
                    "supports_vision": c.supports_vision,
                    "has_conditions": c.routing_conditions.is_some(),
                })
            }).collect::<Vec<_>>(),
        }
    })
}

/// Check if we should failover based on error status/message and the candidate's config.
pub fn should_failover(
    status_code: Option<u16>,
    error_msg: &str,
    candidate: &ProviderCandidate,
) -> bool {
    // Check status code
    if let Some(code) = status_code {
        if candidate.failover_on_status_codes.contains(&(code as i64)) {
            return true;
        }
    }

    // Check error keywords
    let lower = error_msg.to_lowercase();
    for kw in &candidate.failover_on_error_keywords {
        if lower.contains(&kw.to_lowercase()) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate_with_defaults() -> ProviderCandidate {
        ProviderCandidate {
            provider_id: "p1".to_string(),
            provider_name: "Test".to_string(),
            priority: 0,
            model: "gpt-4".to_string(),
            routing_conditions: None,
            in_cooldown: false,
            supports_vision: None,
            cooldown_seconds: 60,
            failover_on_status_codes: vec![402, 429, 500, 502, 503, 504],
            failover_on_error_keywords: vec!["rate limit".to_string(), "timeout".to_string()],
        }
    }

    #[test]
    fn route_decision_trace_includes_selected_provider_and_candidates() {
        let mut selection = ProviderSelection {
            route_profile_id: "rp1".to_string(),
            route_profile_name: "Codex Default".to_string(),
            mode: "failover".to_string(),
            provider: mimo_provider_with_matrix("mimo-v2.5-pro"),
            model: "mimo-v2.5-pro".to_string(),
            priority: 1,
            reason: "Failover mode selected first available provider".to_string(),
            candidates: vec![candidate_with_defaults()],
        };
        selection.provider.id = "p1".to_string();
        selection.provider.name = "MiMo".to_string();

        let trace = route_decision_trace(&selection);

        assert_eq!(trace["route_decision"]["profile_id"], "rp1");
        assert_eq!(trace["route_decision"]["profile_name"], "Codex Default");
        assert_eq!(trace["route_decision"]["selected_provider_id"], "p1");
        assert_eq!(trace["route_decision"]["selected_provider_name"], "MiMo");
        assert_eq!(trace["route_decision"]["selected_model"], "mimo-v2.5-pro");
        assert_eq!(
            trace["route_decision"]["candidates"][0]["provider_name"],
            "Test"
        );
    }

    #[test]
    fn sort_cheapest_orders_by_unit_price() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE model_pricing (id TEXT PRIMARY KEY, provider TEXT, model_pattern TEXT,
                input_price REAL, output_price REAL, is_custom INTEGER, updated_at TEXT);
             INSERT INTO model_pricing VALUES ('1','cheap','m', 1.0, 1.0, 0, '');
             INSERT INTO model_pricing VALUES ('2','pricey','m', 50.0, 50.0, 0, '');",
        )
        .unwrap();

        let mk = |name: &str, priority: i64| {
            let mut c = candidate_with_defaults();
            c.provider_id = name.to_string();
            c.provider_name = name.to_string();
            c.model = "m".to_string();
            c.priority = priority;
            c
        };

        // 手工顺序贵的在前；cheapest 把便宜的排到前面。
        let mut cands = vec![mk("pricey", 1), mk("cheap", 2)];
        sort_candidates_by_strategy(&conn, &mut cands, "cheapest");
        assert_eq!(cands[0].provider_name, "cheap");
        assert_eq!(cands[1].provider_name, "pricey");

        // priority 策略不动，保持手工顺序。
        let mut cands2 = vec![mk("pricey", 1), mk("cheap", 2)];
        sort_candidates_by_strategy(&conn, &mut cands2, "priority");
        assert_eq!(cands2[0].provider_name, "pricey");

        // 查不到价的候选排到末尾。
        let mut cands3 = vec![mk("cheap", 1), mk("unknown", 2)];
        sort_candidates_by_strategy(&conn, &mut cands3, "cheapest");
        assert_eq!(cands3[0].provider_name, "cheap");
        assert_eq!(cands3[1].provider_name, "unknown");
    }

    #[test]
    fn cheapest_sort_breaks_price_ties_by_priority_and_keeps_unknown_last() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE model_pricing (id TEXT PRIMARY KEY, provider TEXT, model_pattern TEXT,
                input_price REAL, output_price REAL, is_custom INTEGER, updated_at TEXT);
             INSERT INTO model_pricing VALUES ('1','a','m', 2.0, 2.0, 0, '');
             INSERT INTO model_pricing VALUES ('2','b','m', 2.0, 2.0, 0, '');
             INSERT INTO model_pricing VALUES ('3','c','m', 1.0, 1.0, 0, '');",
        )
        .unwrap();
        let mk = |name: &str, priority: i64| {
            let mut c = candidate_with_defaults();
            c.provider_id = name.to_string();
            c.provider_name = name.to_string();
            c.model = "m".to_string();
            c.priority = priority;
            c
        };
        // get_price 会按 model 名跨 provider 兜底查价,未知候选必须用没有定价的 model。
        let mut unknown = mk("x", 0);
        unknown.model = "no-price-model".to_string();
        let mut cands = vec![unknown, mk("b", 3), mk("a", 2), mk("c", 5)];
        sort_candidates_by_strategy(&conn, &mut cands, "cheapest");
        let order: Vec<&str> = cands.iter().map(|c| c.provider_name.as_str()).collect();
        // c 最便宜;a/b 同价按 priority;查不到价的 x 排最后
        assert_eq!(order, vec!["c", "a", "b", "x"]);
    }

    #[test]
    fn test_should_failover_on_status_code() {
        let c = candidate_with_defaults();
        assert!(should_failover(Some(429), "ok", &c));
        assert!(should_failover(Some(500), "ok", &c));
        assert!(should_failover(Some(503), "ok", &c));
    }

    #[test]
    fn test_should_not_failover_on_success_code() {
        let c = candidate_with_defaults();
        assert!(!should_failover(Some(200), "ok", &c));
        assert!(!should_failover(Some(400), "bad request", &c));
        assert!(!should_failover(Some(404), "not found", &c));
    }

    #[test]
    fn test_should_failover_on_keyword() {
        let c = candidate_with_defaults();
        assert!(should_failover(None, "Rate limit exceeded", &c));
        assert!(should_failover(None, "Connection timeout", &c));
        assert!(should_failover(None, "request timeout error", &c));
    }

    #[test]
    fn test_should_failover_on_status_and_keyword() {
        let c = candidate_with_defaults();
        assert!(should_failover(Some(429), "Rate limit exceeded", &c));
    }

    #[test]
    fn test_should_not_failover_no_match() {
        let c = candidate_with_defaults();
        assert!(!should_failover(None, "everything is fine", &c));
        assert!(!should_failover(Some(200), "success", &c));
    }

    #[test]
    fn test_should_failover_custom_status_codes() {
        let mut c = candidate_with_defaults();
        c.failover_on_status_codes = vec![418];
        assert!(should_failover(Some(418), "I'm a teapot", &c));
        assert!(!should_failover(Some(500), "error", &c));
    }

    #[test]
    fn test_should_failover_custom_keywords() {
        let mut c = candidate_with_defaults();
        c.failover_on_error_keywords = vec!["insufficient_quota".to_string()];
        assert!(should_failover(None, "insufficient_quota", &c));
        assert!(!should_failover(None, "rate limit", &c));
    }

    #[test]
    fn test_should_failover_empty_lists() {
        let mut c = candidate_with_defaults();
        c.failover_on_status_codes = vec![];
        c.failover_on_error_keywords = vec![];
        assert!(!should_failover(Some(500), "error", &c));
        assert!(!should_failover(None, "error", &c));
    }

    // ── Routing conditions tests ──

    fn test_analysis(chars: usize, images: bool, tools: bool, system: &str) -> RequestAnalysis {
        RequestAnalysis {
            input_char_count: chars,
            has_images: images,
            has_tools: tools,
            tool_count: 0,
            system_text: system.to_string(),
            message_count: 1,
        }
    }

    // ── Scenario routing condition tests ──

    #[test]
    fn model_name_match_hits_substring_case_insensitive() {
        let cond = RoutingConditions {
            model_name_match: Some(vec!["haiku".into()]),
            ..Default::default()
        };
        // Claude Code background 任务发 claude-3-5-haiku → 命中
        assert!(model_name_matches(&cond, Some("claude-3-5-haiku-20241022")));
        assert!(model_name_matches(&cond, Some("Claude-HAIKU")));
        // 主对话发 opus → 不命中
        assert!(!model_name_matches(&cond, Some("claude-opus-4")));
        // 无 model 名 → 不命中
        assert!(!model_name_matches(&cond, None));
    }

    #[test]
    fn model_name_match_unset_is_unconstrained() {
        let cond = RoutingConditions::default();
        assert!(model_name_matches(&cond, Some("anything")));
        assert!(model_name_matches(&cond, None));
    }

    #[test]
    fn analyze_chat_value_reads_tools_images_and_system() {
        let body = serde_json::json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "you are a coder"},
                {"role": "user", "content": [
                    {"type": "text", "text": "look"},
                    {"type": "image_url", "image_url": {"url": "https://x/a.png"}}
                ]}
            ],
            "tools": [{"type": "function", "function": {"name": "rg"}}]
        });
        let a = analyze_chat_value(&body);
        assert!(a.has_images);
        assert!(a.has_tools);
        assert_eq!(a.tool_count, 1);
        assert!(a.system_text.contains("coder"));
        assert!(a.input_char_count >= "you are a coder".len() + "look".len());
        let cond = RoutingConditions {
            has_images: Some(true),
            has_tools: Some(true),
            system_keywords: Some(vec!["coder".into()]),
            ..Default::default()
        };
        assert!(matches_conditions(&cond, &a));
    }

    #[test]
    fn analyze_messages_value_reads_anthropic_image_and_system() {
        let body = serde_json::json!({
            "model": "claude-sonnet-4",
            "system": "be brief",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "see"},
                    {"type": "image", "source": {"type": "url", "url": "https://x/a.png"}}
                ]
            }],
            "tools": [{"name": "Bash"}]
        });
        let a = analyze_messages_value(&body);
        assert!(a.has_images);
        assert!(a.has_tools);
        assert_eq!(a.system_text, "be brief");
        assert!(matches_conditions(
            &RoutingConditions {
                has_images: Some(true),
                min_input_chars: Some(3),
                ..Default::default()
            },
            &a
        ));
    }

    #[test]
    fn analyze_gemini_value_reads_inline_image_and_system() {
        let body = serde_json::json!({
            "systemInstruction": {"parts": [{"text": "sys-hint"}]},
            "contents": [{
                "role": "user",
                "parts": [
                    {"text": "photo"},
                    {"inline_data": {"mime_type": "image/png", "data": "xxxx"}}
                ]
            }],
            "tools": [{"functionDeclarations": [{"name": "search"}]}]
        });
        let a = analyze_gemini_value(&body);
        assert!(a.has_images);
        assert!(a.has_tools);
        assert_eq!(a.system_text, "sys-hint");
        assert!(a.input_char_count >= "sys-hint".len() + "photo".len());
    }

    #[test]
    fn has_images_condition_selects_on_chat_messages_and_responses() {
        use crate::models::provider::CreateProviderInput;
        use crate::models::route_profile::{AddProviderToRouteInput, CreateRouteProfileInput};
        use crate::storage::db::DbPool;
        use crate::storage::{migrations, providers, route_profiles};
        use r2d2::Pool;
        use r2d2_sqlite::SqliteConnectionManager;

        for protocol in [
            "openai_chat_completions",
            "anthropic_messages",
            "openai_responses",
        ] {
            let manager = SqliteConnectionManager::memory();
            let pool: DbPool = Pool::builder().max_size(1).build(manager).unwrap();
            let vision_id;
            let text_id;
            {
                let conn = pool.get().unwrap();
                migrations::run_migrations(&conn).unwrap();
                let mk = |name: &str| {
                    providers::create(
                        &conn,
                        CreateProviderInput {
                            name: name.into(),
                            provider_type: "openai".into(),
                            base_url: format!("https://api.{name}.example"),
                            api_key: Some("sk-test".into()),
                            default_model: "gpt-4o".into(),
                            protocol: r#"["openai_chat_completions","openai_responses","anthropic_messages"]"#
                                .into(),
                            timeout_seconds: Some(120),
                            enabled: Some(true),
                            ..Default::default()
                        },
                    )
                    .unwrap()
                    .id
                };
                vision_id = mk("vision");
                text_id = mk("text");
                let profile = route_profiles::create(
                    &conn,
                    CreateRouteProfileInput {
                        name: format!("{protocol} vision-lock"),
                        input_protocol: protocol.into(),
                        mode: Some("failover".into()),
                    },
                )
                .unwrap();
                route_profiles::set_default(&conn, &profile.id).unwrap();
                route_profiles::add_provider(
                    &conn,
                    &profile.id,
                    &vision_id,
                    AddProviderToRouteInput {
                        priority: Some(1),
                        model_override: None,
                        cooldown_seconds: None,
                        failover_on_status_codes: None,
                        failover_on_error_keywords: None,
                        routing_conditions: Some(r#"{"has_images":true}"#.into()),
                    },
                )
                .unwrap();
                route_profiles::add_provider(
                    &conn,
                    &profile.id,
                    &text_id,
                    AddProviderToRouteInput {
                        priority: Some(2),
                        model_override: None,
                        cooldown_seconds: None,
                        failover_on_status_codes: None,
                        failover_on_error_keywords: None,
                        routing_conditions: None,
                    },
                )
                .unwrap();
            }

            let image = select_for_failover(
                &pool,
                protocol,
                Some("gpt-4o"),
                Some(&test_analysis(10, true, false, "")),
            )
            .unwrap();
            assert_eq!(
                image.provider.id, vision_id,
                "{protocol} 带图应命中 has_images 条件"
            );
            let text = select_for_failover(
                &pool,
                protocol,
                Some("gpt-4o"),
                Some(&test_analysis(10, false, false, "")),
            )
            .unwrap();
            assert_eq!(
                text.provider.id, text_id,
                "{protocol} 无图应跳过 has_images 条件"
            );
        }
    }

    // ── Capability promotion tests ──

    fn mimo_provider_with_matrix(default: &str) -> Provider {
        Provider {
            id: "p".into(),
            name: "MiMo".into(),
            provider_type: "mimo".into(),
            base_url: "https://api.xiaomimimo.com/v1".into(),
            api_key: Some("sk-x".into()),
            default_model: default.into(),
            reasoning_model: None,
            supported_models: Some(
                r#"["mimo-v2.5-pro","mimo-v2.5","mimo-v2-omni","mimo-v2-flash"]"#.into(),
            ),
            model_mapping: None,
            extra_headers: None,
            anthropic_base_url: None,
            responses_base_url: None,
            protocol: "openai_chat_completions".into(),
            timeout_seconds: 120,
            status: "ok".into(),
            supports_vision: None,
            auto_cache_control: None,
            supports_cache: None,
            model_capabilities: Some(
                r#"{
                "mimo-v2.5-pro":["text","reasoning","tools","web_search"],
                "mimo-v2.5":["text","vision","reasoning","tools","web_search"],
                "mimo-v2-omni":["text","vision","audio_in","video_in","tools"],
                "mimo-v2-flash":["text","reasoning","tools","web_search"]
            }"#
                .into(),
            ),
            provider_quirks: None,
            body_filter_enabled: None,
            thinking_rectifier_enabled: None,
            error_mapper_enabled: None,
            model_degradation_chain: None,
            model_context_windows: None,
            enabled: true,
            is_active: true,
            created_at: "2024-01-01".into(),
            updated_at: "2024-01-01".into(),
        }
    }

    #[test]
    fn promote_swaps_to_vision_model_when_request_has_image() {
        // supported_models order: pro, v2.5, v2-omni, flash. Both v2.5 and v2-omni
        // have vision. v2.5 preserves more of the original (reasoning + web_search)
        // than omni (which has neither), so v2.5 should win even though omni is
        // not listed first.
        let p = mimo_provider_with_matrix("mimo-v2.5-pro");
        let analysis = test_analysis(100, /* images */ true, false, "");
        let promoted = promote_for_capabilities(&p, "mimo-v2.5-pro", &analysis);
        assert_eq!(
            promoted, "mimo-v2.5",
            "should pick v2.5 — preserves reasoning + web_search of original"
        );
    }

    #[test]
    fn promote_prefers_capability_overlap_over_list_order() {
        // Sanity check: even if v2-omni is listed FIRST in supported_models,
        // the ranking should still pick v2.5 because of higher overlap.
        let mut p = mimo_provider_with_matrix("mimo-v2.5-pro");
        p.supported_models =
            Some(r#"["mimo-v2-omni","mimo-v2.5","mimo-v2.5-pro","mimo-v2-flash"]"#.into());
        let analysis = test_analysis(100, true, false, "");
        let promoted = promote_for_capabilities(&p, "mimo-v2.5-pro", &analysis);
        assert_eq!(
            promoted, "mimo-v2.5",
            "list order doesn't override overlap score"
        );
    }

    #[test]
    fn promote_falls_back_to_list_order_on_tied_overlap() {
        // If two vision models have identical overlap, supported_models order breaks the tie.
        // Build a matrix where two models have identical caps.
        let mut p = mimo_provider_with_matrix("mimo-v2.5-pro");
        p.supported_models = Some(r#"["mimo-v2-omni","mimo-v2.5","mimo-v2.5-pro"]"#.into());
        p.model_capabilities = Some(
            r#"{
            "mimo-v2.5-pro":["text","reasoning"],
            "mimo-v2.5":["text","vision","reasoning"],
            "mimo-v2-omni":["text","vision","reasoning"]
        }"#
            .into(),
        );
        let analysis = test_analysis(100, true, false, "");
        let promoted = promote_for_capabilities(&p, "mimo-v2.5-pro", &analysis);
        assert_eq!(promoted, "mimo-v2-omni", "ties → first in supported_models");
    }

    #[test]
    fn promote_keeps_model_when_already_vision_capable() {
        let p = mimo_provider_with_matrix("mimo-v2.5");
        let analysis = test_analysis(100, true, false, "");
        let promoted = promote_for_capabilities(&p, "mimo-v2.5", &analysis);
        assert_eq!(promoted, "mimo-v2.5");
    }

    #[test]
    fn promote_keeps_model_when_no_image_in_request() {
        let p = mimo_provider_with_matrix("mimo-v2.5-pro");
        let analysis = test_analysis(100, /* images */ false, false, "");
        let promoted = promote_for_capabilities(&p, "mimo-v2.5-pro", &analysis);
        assert_eq!(promoted, "mimo-v2.5-pro");
    }

    #[test]
    fn promote_noop_when_matrix_missing() {
        let mut p = mimo_provider_with_matrix("mimo-v2.5-pro");
        p.model_capabilities = None;
        let analysis = test_analysis(100, true, false, "");
        let promoted = promote_for_capabilities(&p, "mimo-v2.5-pro", &analysis);
        assert_eq!(promoted, "mimo-v2.5-pro", "no matrix → no promotion");
    }

    #[test]
    fn promote_handles_1m_qualifier() {
        let p = mimo_provider_with_matrix("mimo-v2.5-pro[1m]");
        let analysis = test_analysis(100, true, false, "");
        let promoted = promote_for_capabilities(&p, "mimo-v2.5-pro[1m]", &analysis);
        assert_eq!(
            promoted, "mimo-v2.5",
            "[1m] qualifier should be stripped before lookup"
        );
    }

    // Verify the supports_vision derivation precedence: matrix must WIN
    // over the legacy single-boolean flag, otherwise a stale `false` from
    // the per-provider probe (run against a non-vision default model)
    // would mask a perfectly capable sibling model in the matrix.
    #[test]
    fn supports_vision_derivation_matrix_overrides_legacy_false() {
        let p = mimo_provider_with_matrix("mimo-v2.5-pro");
        // simulate the old buggy probe that wrote false
        let mut p = p;
        p.supports_vision = Some(false);

        let caps = p.parse_capabilities();
        let from_matrix = caps.values().any(|c| c.iter().any(|x| x == "vision"));
        // The matrix says: yes, *some* model has vision.
        assert!(
            from_matrix,
            "matrix should report vision-capable model present"
        );
        // After the fix, the derived flag for the candidate must trust the matrix.
        // (This mirrors the production code's chain of .and_then().or_else())
        let derived = if !caps.is_empty() {
            Some(from_matrix)
        } else {
            p.supports_vision
        };
        assert_eq!(derived, Some(true));
    }

    #[test]
    fn promote_no_swap_when_no_vision_model_exists() {
        let mut p = mimo_provider_with_matrix("deepseek-v4-pro");
        p.provider_type = "deepseek".into();
        p.supported_models = Some(r#"["deepseek-v4-pro","deepseek-flash"]"#.into());
        p.model_capabilities = Some(
            r#"{
            "deepseek-v4-pro":["text","reasoning","tools","web_search"],
            "deepseek-flash":["text","tools"]
        }"#
            .into(),
        );
        let analysis = test_analysis(100, true, false, "");
        let promoted = promote_for_capabilities(&p, "deepseek-v4-pro", &analysis);
        assert_eq!(
            promoted, "deepseek-v4-pro",
            "no vision model → leave alone, let upstream surface error"
        );
    }

    #[test]
    fn test_matches_conditions_empty() {
        let cond = RoutingConditions::default();
        let analysis = test_analysis(100, false, false, "");
        assert!(matches_conditions(&cond, &analysis));
    }

    #[test]
    fn test_matches_conditions_min_chars() {
        let cond = RoutingConditions {
            min_input_chars: Some(1000),
            ..Default::default()
        };
        assert!(!matches_conditions(
            &cond,
            &test_analysis(500, false, false, "")
        ));
        assert!(matches_conditions(
            &cond,
            &test_analysis(1000, false, false, "")
        ));
        assert!(matches_conditions(
            &cond,
            &test_analysis(5000, false, false, "")
        ));
    }

    #[test]
    fn test_matches_conditions_max_chars() {
        let cond = RoutingConditions {
            max_input_chars: Some(1000),
            ..Default::default()
        };
        assert!(matches_conditions(
            &cond,
            &test_analysis(500, false, false, "")
        ));
        assert!(matches_conditions(
            &cond,
            &test_analysis(1000, false, false, "")
        ));
        assert!(!matches_conditions(
            &cond,
            &test_analysis(5000, false, false, "")
        ));
    }

    #[test]
    fn test_matches_conditions_has_images() {
        let cond = RoutingConditions {
            has_images: Some(true),
            ..Default::default()
        };
        assert!(!matches_conditions(
            &cond,
            &test_analysis(100, false, false, "")
        ));
        assert!(matches_conditions(
            &cond,
            &test_analysis(100, true, false, "")
        ));
    }

    #[test]
    fn test_matches_conditions_has_tools() {
        let cond = RoutingConditions {
            has_tools: Some(true),
            ..Default::default()
        };
        assert!(!matches_conditions(
            &cond,
            &test_analysis(100, false, false, "")
        ));
        assert!(matches_conditions(
            &cond,
            &test_analysis(100, false, true, "")
        ));
    }

    #[test]
    fn test_matches_conditions_system_keywords() {
        let cond = RoutingConditions {
            system_keywords: Some(vec!["background".to_string(), "subagent".to_string()]),
            ..Default::default()
        };
        assert!(!matches_conditions(
            &cond,
            &test_analysis(100, false, false, "You are a helpful assistant")
        ));
        assert!(matches_conditions(
            &cond,
            &test_analysis(100, false, false, "Run this in background mode")
        ));
        assert!(matches_conditions(
            &cond,
            &test_analysis(100, false, false, "This is a SUBAGENT task")
        ));
    }

    #[test]
    fn test_matches_conditions_combined() {
        let cond = RoutingConditions {
            min_input_chars: Some(1000),
            has_images: Some(true),
            ..Default::default()
        };
        assert!(!matches_conditions(
            &cond,
            &test_analysis(500, true, false, "")
        )); // chars too low
        assert!(!matches_conditions(
            &cond,
            &test_analysis(2000, false, false, "")
        )); // no images
        assert!(matches_conditions(
            &cond,
            &test_analysis(2000, true, false, "")
        )); // both match
    }

    #[test]
    fn test_matches_conditions_parse_json() {
        let json = r#"{"has_images": true, "system_keywords": ["background"]}"#;
        let cond: RoutingConditions = serde_json::from_str(json).unwrap();
        assert!(matches_conditions(
            &cond,
            &test_analysis(100, true, false, "background task")
        ));
        assert!(!matches_conditions(
            &cond,
            &test_analysis(100, false, false, "background task")
        ));
    }

    #[test]
    fn build_candidates_skips_invalid_routing_conditions() {
        let provider = RouteProfileProviderView {
            id: "rpp1".to_string(),
            provider_id: "p1".to_string(),
            provider_name: "BrokenConditions".to_string(),
            provider_type: "openai".to_string(),
            provider_protocol: "openai_responses".to_string(),
            has_anthropic_url: false,
            supports_vision: None,
            model_capabilities: None,
            priority: 1,
            enabled: true,
            model_override: Some("gpt-4".to_string()),
            cooldown_seconds: 600,
            failover_on_status_codes: None,
            failover_on_error_keywords: None,
            routing_conditions: Some("{bad-json".to_string()),
            runtime_available: true,
            cooldown_until: None,
            consecutive_failures: 0,
        };
        let analysis = test_analysis(100, false, false, "");

        let candidates =
            build_candidates(&HashMap::new(), &[provider], None, Some(&analysis)).unwrap();

        assert!(
            candidates.is_empty(),
            "invalid routing_conditions must not widen the provider match"
        );
    }

    #[test]
    fn build_candidates_skips_disabled_provider() {
        let temp = std::env::temp_dir().join(format!(
            "agentgate_provider_selector_{}",
            uuid::Uuid::new_v4()
        ));
        let pool = crate::storage::db::init_database(&temp).unwrap();
        let conn = pool.get().unwrap();
        let provider = crate::storage::providers::create(
            &conn,
            crate::models::provider::CreateProviderInput {
                name: "DisabledProvider".to_string(),
                provider_type: "openai".to_string(),
                base_url: "https://api.openai.com".to_string(),
                api_key: Some("sk-test".to_string()),
                default_model: "gpt-4".to_string(),
                reasoning_model: None,
                supported_models: None,
                model_mapping: None,
                extra_headers: None,
                anthropic_base_url: None,
                responses_base_url: None,
                protocol: "openai_chat_completions".to_string(),
                timeout_seconds: Some(120),
                auto_cache_control: None,
                model_capabilities: None,
                provider_quirks: None,
                body_filter_enabled: None,
                thinking_rectifier_enabled: None,
                error_mapper_enabled: None,
                model_degradation_chain: None,
                model_context_windows: None,
                enabled: Some(false),
            },
        )
        .unwrap();
        let route_provider = RouteProfileProviderView {
            id: "rpp1".to_string(),
            provider_id: provider.id,
            provider_name: "DisabledProvider".to_string(),
            provider_type: "openai".to_string(),
            provider_protocol: "openai_chat_completions".to_string(),
            has_anthropic_url: false,
            supports_vision: None,
            model_capabilities: None,
            priority: 1,
            enabled: true,
            model_override: None,
            cooldown_seconds: 600,
            failover_on_status_codes: None,
            failover_on_error_keywords: None,
            routing_conditions: None,
            runtime_available: true,
            cooldown_until: None,
            consecutive_failures: 0,
        };

        let providers = load_provider_map(&conn).unwrap();
        let candidates =
            build_candidates(&providers, &[route_provider], Some("agentgate"), None).unwrap();

        assert!(
            candidates.is_empty(),
            "disabled providers must not remain routable through route profiles"
        );
        drop(conn);
        drop(pool);
        let _ = std::fs::remove_dir_all(temp);
    }

    #[test]
    fn test_should_failover_keyword_case_insensitive() {
        let c = candidate_with_defaults();
        assert!(should_failover(None, "RATE LIMIT", &c));
        assert!(should_failover(None, "Timeout", &c));
        assert!(should_failover(None, "TIMEOUT", &c));
    }

    // ── Degradation chain tests ──

    fn provider_with_chain(chain_json: Option<&str>) -> Provider {
        Provider {
            id: "p".into(),
            name: "P".into(),
            provider_type: "openai".into(),
            base_url: "https://api.openai.com".into(),
            api_key: Some("sk-x".into()),
            default_model: "gpt-5-codex".into(),
            reasoning_model: None,
            supported_models: None,
            model_mapping: None,
            extra_headers: None,
            anthropic_base_url: None,
            responses_base_url: None,
            protocol: "openai_responses".into(),
            timeout_seconds: 120,
            status: "ok".into(),
            supports_vision: None,
            auto_cache_control: None,
            supports_cache: None,
            model_capabilities: None,
            provider_quirks: None,
            body_filter_enabled: None,
            thinking_rectifier_enabled: None,
            error_mapper_enabled: None,
            model_degradation_chain: chain_json.map(|s| s.to_string()),
            model_context_windows: None,
            enabled: true,
            is_active: true,
            created_at: "2024".into(),
            updated_at: "2024".into(),
        }
    }

    #[test]
    fn degradation_chain_returns_requested_then_fallbacks() {
        let p = provider_with_chain(Some(r#"{"gpt-5-codex":["gpt-5-mini","gpt-4o"]}"#));
        assert_eq!(
            degradation_chain_for_model(&p, "gpt-5-codex"),
            vec!["gpt-5-codex", "gpt-5-mini", "gpt-4o"]
        );
    }

    #[test]
    fn degradation_chain_returns_just_requested_when_no_entry() {
        let p = provider_with_chain(Some(r#"{"gpt-5-codex":["gpt-5-mini"]}"#));
        assert_eq!(
            degradation_chain_for_model(&p, "claude-sonnet-4"),
            vec!["claude-sonnet-4"]
        );
    }

    #[test]
    fn degradation_chain_handles_missing_config() {
        let p = provider_with_chain(None);
        assert_eq!(
            degradation_chain_for_model(&p, "gpt-5-codex"),
            vec!["gpt-5-codex"]
        );
    }

    #[test]
    fn degradation_chain_dedupes_and_skips_self_reference() {
        // Pathological config: chain includes the requested model and a dup.
        // The walker should not loop or revisit a model.
        let p = provider_with_chain(Some(
            r#"{"gpt-5-codex":["gpt-5-codex","gpt-5-mini","gpt-5-mini","gpt-4o"]}"#,
        ));
        assert_eq!(
            degradation_chain_for_model(&p, "gpt-5-codex"),
            vec!["gpt-5-codex", "gpt-5-mini", "gpt-4o"]
        );
    }

    #[test]
    fn degradation_chain_handles_invalid_json() {
        let p = provider_with_chain(Some("not-json-at-all"));
        assert_eq!(
            degradation_chain_for_model(&p, "anything"),
            vec!["anything"]
        );
    }
}
