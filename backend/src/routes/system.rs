use axum::{Json, extract::State};

use crate::models::{AppState, DomainResponse, HealthResponse};

pub(super) async fn healthz(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        domains: state.config.domains,
        service: "kooixmail-rust-backend",
    })
}

pub(super) async fn list_domains(State(state): State<AppState>) -> Json<Vec<DomainResponse>> {
    let domains = state
        .config
        .domains
        .iter()
        .map(|domain| DomainResponse {
            id: domain.clone(),
            domain: domain.clone(),
            is_verified: true,
        })
        .collect();

    Json(domains)
}
