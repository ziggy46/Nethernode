//! Turning a complete upload into a file in the server's folder.

use nn_events::Event;
use nn_files::{Adopted, OnConflict, RelPath};
use nn_store::uploads::status;
use nn_store::{NewAuditEntry, Upload};
use serde_json::json;

use crate::AppState;
use crate::files::{open, start_extract};

/// Verifies and places a fully received upload in the background (it can
/// take a while for a large file with a checksum, longer than a proxy lets
/// a request run).
pub(crate) fn spawn(state: AppState, upload: Upload) {
    tokio::spawn(async move {
        let id = upload.id.clone();
        if let Err(message) = finish(&state, &upload).await {
            tracing::warn!(upload = %id, %message, "upload couldn't be placed");
            fail(&state, &upload, &message).await;
        }
    });
}

async fn finish(state: &AppState, upload: &Upload) -> Result<(), String> {
    let temp = state
        .uploads
        .temp()
        .ok_or("uploads aren't available")?
        .clone();
    let src = temp.path(&upload.id).map_err(|e| e.to_string())?;
    let size = u64::try_from(upload.total_size).unwrap_or(0);

    if let Some(want) = upload.sha256.clone() {
        let path = src.clone();
        let got = tokio::task::spawn_blocking(move || nn_uploads::sha256_file(&path, size))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("couldn't read the upload back: {e}"))?;
        if hex::encode(got) != want {
            return Err(format!(
                "{} didn't arrive intact (its SHA-256 doesn't match); upload it again",
                upload.filename
            ));
        }
    }

    let dir = RelPath::parse(&upload.dest_path).map_err(|e| e.to_string())?;
    let on_conflict = OnConflict::parse(&upload.on_conflict).unwrap_or(OnConflict::Rename);
    let (server, files) = open(state, &upload.server_id)
        .await
        .map_err(|e| e.message.unwrap_or_default())?;
    let (placed_files, name, placed_dir) = (files.clone(), upload.filename.clone(), dir.clone());
    let adopted = tokio::task::spawn_blocking(move || {
        placed_files.adopt(&src, &placed_dir, &name, on_conflict)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    let (path, message) = match &adopted {
        Adopted::Placed(path) => (path.clone(), format!("Uploaded {path}.")),
        Adopted::Skipped(path) => (
            path.clone(),
            format!("Skipped {}: {path} already exists.", upload.filename),
        ),
    };
    let store = state.auth.store();
    if let Err(e) = store
        .transition_upload(&upload.id, status::FINISHING, status::DONE, Some(&message))
        .await
    {
        tracing::warn!(upload = %upload.id, error = %e, "couldn't record a finished upload");
    }
    let entry = NewAuditEntry {
        user_id: Some(&upload.user_id),
        action: "file.upload",
        target_type: Some("server"),
        target_id: Some(&server.id),
        details: Some(json!({
            "path": path.as_str(),
            "size": upload.total_size,
            "on_conflict": upload.on_conflict,
            "skipped": matches!(adopted, Adopted::Skipped(_)),
        })),
    };
    if let Err(e) = store.insert_audit(entry).await {
        tracing::warn!(error = %e, "failed to write audit log entry");
    }
    tracing::info!(upload = %upload.id, path = %path, "upload finished");
    state.events.publish(Event::UploadFinished {
        server_id: server.id.clone(),
        upload_id: upload.id.clone(),
        path: path.to_string(),
        message,
        skipped: matches!(adopted, Adopted::Skipped(_)),
        failed: false,
    });

    if upload.extract
        && let Adopted::Placed(archive) = adopted
        && archive.as_str().to_ascii_lowercase().ends_with(".zip")
    {
        start_extract(
            state,
            &server.id,
            files,
            archive.clone(),
            dir.clone(),
            false,
        );
        let entry = NewAuditEntry {
            user_id: Some(&upload.user_id),
            action: "file.extract",
            target_type: Some("server"),
            target_id: Some(&server.id),
            details: Some(json!({ "archive": archive.as_str(), "destination": dir.as_str() })),
        };
        if let Err(e) = store.insert_audit(entry).await {
            tracing::warn!(error = %e, "failed to write audit log entry");
        }
    }
    Ok(())
}

async fn fail(state: &AppState, upload: &Upload, message: &str) {
    if let Err(e) = state
        .auth
        .store()
        .transition_upload(&upload.id, status::FINISHING, status::FAILED, Some(message))
        .await
    {
        tracing::warn!(upload = %upload.id, error = %e, "couldn't record a failed upload");
    }
    if let Some(temp) = state.uploads.temp()
        && let Err(e) = temp.remove(&upload.id)
    {
        tracing::warn!(upload = %upload.id, error = %e, "couldn't delete a failed upload");
    }
    state.events.publish(Event::UploadFinished {
        server_id: upload.server_id.clone(),
        upload_id: upload.id.clone(),
        path: String::new(),
        message: message.to_owned(),
        skipped: false,
        failed: true,
    });
}
