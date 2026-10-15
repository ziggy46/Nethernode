//! Cleaning up after a backup or restore that NetherNode was stopped in
//! the middle of.

use crate::AppState;

/// Marks interrupted backups failed and removes their partial files, and
/// finishes or undoes interrupted restores.
pub async fn recover(state: &AppState) {
    let unfinished = state
        .auth
        .store()
        .fail_unfinished_backups()
        .await
        .unwrap_or_default();
    state.nodes.local().recover_backups(&unfinished).await;
}
