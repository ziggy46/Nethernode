//! Upload housekeeping: finishing uploads a restart interrupted, and
//! discarding abandoned ones.

use std::collections::HashSet;
use std::time::Duration;

use nn_store::uploads::status;

use crate::AppState;

/// How often abandoned uploads are looked for.
const JANITOR_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// How long records of ended uploads are kept.
const KEEP_RECORDS_MS: i64 = 7 * 24 * 3600 * 1000;

/// Starts the background work.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        resume_finishing(&state).await;
        let mut tick = tokio::time::interval(JANITOR_INTERVAL);
        loop {
            tick.tick().await;
            janitor(&state).await;
        }
    });
}

/// Uploads whose bytes were all in when NetherNode stopped get placed now.
async fn resume_finishing(state: &AppState) {
    match state.auth.store().finishing_uploads().await {
        Ok(uploads) => {
            for upload in uploads {
                tracing::info!(upload = %upload.id, "finishing an upload interrupted by a restart");
                super::finish::spawn(state.clone(), upload);
            }
        }
        Err(e) => tracing::warn!(error = %e, "couldn't load unfinished uploads"),
    }
}

/// Discards expired uploads and partial files nothing refers to, and prunes
/// old records.
pub async fn janitor(state: &AppState) {
    let Some(temp) = state.uploads.temp() else {
        return;
    };
    let store = state.auth.store();
    match store.expired_uploads().await {
        Ok(expired) => {
            for upload in expired {
                // Skip one being written right now; it'll be fresh again.
                let Some(_guard) = state.uploads.try_lock(&upload.id) else {
                    continue;
                };
                match store
                    .transition_upload(
                        &upload.id,
                        status::UPLOADING,
                        status::EXPIRED,
                        Some("Discarded after no progress."),
                    )
                    .await
                {
                    Ok(true) => {
                        tracing::info!(upload = %upload.id, file = %upload.filename, "discarded an abandoned upload");
                        if let Err(e) = temp.remove(&upload.id) {
                            tracing::warn!(upload = %upload.id, error = %e, "couldn't delete an expired upload");
                        }
                        state.uploads.forget(&upload.id);
                    }
                    Ok(false) => {}
                    Err(e) => tracing::warn!(error = %e, "couldn't expire an upload"),
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "couldn't look for expired uploads"),
    }

    // Partial files without a live upload: left by a crash, or by a server
    // deleted mid-upload. Files are listed first: a new upload's row is
    // written before its file, so it can't be mistaken for an orphan.
    let on_disk = temp.ids();
    match (store.unfinished_upload_ids().await, on_disk) {
        (Ok(live), Ok(on_disk)) => {
            let live: HashSet<String> = live.into_iter().collect();
            for id in on_disk.into_iter().filter(|id| !live.contains(id)) {
                if let Err(e) = temp.remove(&id) {
                    tracing::warn!(upload = %id, error = %e, "couldn't delete an orphaned upload file");
                }
            }
        }
        (Err(e), _) => tracing::warn!(error = %e, "couldn't list unfinished uploads"),
        (_, Err(e)) => tracing::warn!(error = %e, "couldn't list upload files"),
    }

    if let Err(e) = store
        .prune_uploads(nn_store::now_ms() - KEEP_RECORDS_MS)
        .await
    {
        tracing::warn!(error = %e, "couldn't prune upload records");
    }
}
