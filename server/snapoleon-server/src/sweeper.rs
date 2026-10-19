//! Background retention: removes expired items and per-workspace retention
//! overflow, then prunes bookkeeping rows.

use std::time::Duration;

use snap_core::now_ms;

use crate::{error::AppError, state::AppState, viewer};

const INTERVAL: Duration = Duration::from_secs(600);

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        let mut tick = tokio::time::interval(INTERVAL);
        loop {
            tick.tick().await;
            match run_once(&state).await {
                Ok(0) => {}
                Ok(n) => tracing::info!("retention sweep removed {n} item(s)"),
                Err(e) => tracing::warn!("retention sweep failed: {}", e.message()),
            }
        }
    });
}

/// One sweep; returns how many items were removed.
pub async fn run_once(state: &AppState) -> Result<usize, AppError> {
    let mut removed = 0;
    loop {
        let lapsed = state.db.lapsed_items(now_ms()).await?;
        if lapsed.is_empty() {
            break;
        }
        for item in &lapsed {
            viewer::remove_item(state, item).await?;
            removed += 1;
        }
    }
    state.db.prune(now_ms()).await?;
    Ok(removed)
}
