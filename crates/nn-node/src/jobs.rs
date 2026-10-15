//! Long file jobs (extract, compress, install) that report progress as
//! `file_job_progress` and `file_job_finished` events.

use std::future::Future;

use nn_events::{Event, EventBus};
use nn_files::FileError;

/// Reports a job's progress, publishing an event when the percentage moves.
pub struct Progress {
    events: EventBus,
    server_id: String,
    job_id: String,
    kind: &'static str,
    last: Option<u8>,
}

impl Progress {
    fn new(events: EventBus, server_id: String, job_id: String, kind: &'static str) -> Self {
        Self {
            events,
            server_id,
            job_id,
            kind,
            last: None,
        }
    }

    /// Reports progress for job `job_id`, which someone else finishes.
    pub fn for_job(events: &EventBus, server_id: &str, job_id: &str, kind: &'static str) -> Self {
        Self::new(
            events.clone(),
            server_id.to_owned(),
            job_id.to_owned(),
            kind,
        )
    }

    /// `done` of `total` (bytes, files, whatever the job counts).
    pub fn report(&mut self, done: u64, total: u64) {
        let percent = (done.min(total) * 100)
            .checked_div(total)
            .map_or(0, |p| u8::try_from(p).unwrap_or(100));
        if self.last != Some(percent) {
            self.last = Some(percent);
            self.events.publish(Event::FileJobProgress {
                server_id: self.server_id.clone(),
                job_id: self.job_id.clone(),
                kind: self.kind.to_owned(),
                percent,
            });
        }
    }

    fn finish(self, result: Result<String, FileError>) {
        let (message, failed) = match result {
            Ok(message) => (message, false),
            Err(e) => {
                tracing::warn!(server = %self.server_id, kind = self.kind, error = %e, "file job failed");
                (e.to_string(), true)
            }
        };
        self.events.publish(Event::FileJobFinished {
            server_id: self.server_id,
            job_id: self.job_id,
            kind: self.kind.to_owned(),
            message,
            failed,
        });
    }
}

/// Runs blocking `work` as a job. Returns the job ID at once.
pub fn spawn_blocking<F>(events: &EventBus, server_id: &str, kind: &'static str, work: F) -> String
where
    F: FnOnce(&mut dyn FnMut(u64, u64)) -> Result<String, FileError> + Send + 'static,
{
    let job_id = nn_store::new_id();
    let mut progress = Progress::new(events.clone(), server_id.to_owned(), job_id.clone(), kind);
    tokio::task::spawn_blocking(move || {
        let result = work(&mut |done, total| progress.report(done, total));
        progress.finish(result);
    });
    job_id
}

/// Runs async `work` as a job. Returns the job ID at once.
pub fn spawn<F, Fut>(events: &EventBus, server_id: &str, kind: &'static str, work: F) -> String
where
    F: FnOnce(Progress) -> Fut + Send + 'static,
    Fut: Future<Output = (Progress, Result<String, FileError>)> + Send + 'static,
{
    let job_id = nn_store::new_id();
    let progress = Progress::new(events.clone(), server_id.to_owned(), job_id.clone(), kind);
    tokio::spawn(async move {
        let (progress, result) = work(progress).await;
        progress.finish(result);
    });
    job_id
}
