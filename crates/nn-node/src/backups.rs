//! Backup archives on a node: `<data>/backups/<name>.tar.zst`.
//!
//! The panel keeps the records (what exists, retention, schedules) and
//! names the archives; the node makes, restores, serves, and deletes them.
//! Making and restoring hold the server's lifecycle lock throughout, so
//! nothing starts, stops, or wakes the server meanwhile.

use std::path::{Path, PathBuf};

use nn_files::backup::{BackupReport, REGENERATED, restore_into};
use nn_files::{FileError, Limits, ServerFiles};
use nn_store::Server;

use crate::jobs::Progress;
use crate::{FileReader, LocalNode, Node};

/// Why a server on another machine has no backups (yet).
const NOT_YET: &str = "Backups of servers on other machines aren't available yet.";

/// A finished archive: its size on disk, and what's in it.
#[derive(Debug, Clone, Copy)]
pub struct Archive {
    /// Bytes on disk.
    pub size: u64,
    /// What was archived.
    pub report: BackupReport,
}

/// How a restore went.
#[derive(Debug)]
pub struct Restored {
    /// The copy of what was there before.
    pub pre_restore: Result<Archive, String>,
    /// Files restored, or why the restore didn't happen.
    pub files: Result<u64, String>,
}

/// An archive name the panel gave: a plain `*.tar.zst` file name.
fn checked(name: &str) -> Option<&str> {
    (!name.contains(['/', '\\']) && !name.starts_with('.') && name.ends_with(".tar.zst"))
        .then_some(name)
}

impl LocalNode {
    fn backups_dir(&self) -> Option<PathBuf> {
        Some(self.servers_dir.as_ref()?.parent()?.join("backups"))
    }

    fn archive_path(&self, name: &str) -> Option<PathBuf> {
        Some(self.backups_dir()?.join(checked(name)?))
    }

    /// Runs `work` under the server's lifecycle lock (when Docker is
    /// there; without it, nothing can start the server anyway).
    async fn locked<T>(
        &self,
        server: &Server,
        work: impl AsyncFnOnce(Server) -> Result<T, String>,
    ) -> Result<T, String> {
        match &self.orchestrator {
            Some(orchestrator) => orchestrator
                .locked(&server.id, work)
                .await
                .map_err(|e| e.to_string())?,
            None => work(server.clone()).await,
        }
    }

    /// Archives the server's folder (lock already held), pausing saving
    /// while a running server is read.
    async fn make(&self, server: &Server, name: &str, job_id: &str) -> Result<Archive, String> {
        let file = self
            .archive_path(name)
            .ok_or("the data folder isn't available, so backups can't be made")?;
        let running = server.status == "running";
        let orchestrator = self.orchestrator.clone().filter(|_| running);
        if let Some(o) = &orchestrator {
            o.command(&server.id, "save-off")
                .await
                .map_err(|e| format!("couldn't pause saving: {e}"))?;
            if let Err(e) = o.command(&server.id, "save-all flush").await {
                let _ = o.command(&server.id, "save-on").await;
                return Err(format!("couldn't save the world first: {e}"));
            }
        }
        let result = self.write_archive(server, &file, job_id).await;
        if let Some(o) = &orchestrator {
            // Always turn saving back on, whatever happened.
            if let Err(e) = o.command(&server.id, "save-on").await {
                tracing::warn!(server = %server.id, error = %e, "couldn't turn saving back on after a backup");
            }
        }
        if result.is_err() {
            let _ = tokio::fs::remove_file(&file).await;
        }
        result
    }

    async fn write_archive(
        &self,
        server: &Server,
        file: &Path,
        job_id: &str,
    ) -> Result<Archive, String> {
        let files = self.open_files(server).await.map_err(|e| e.to_string())?;
        let tmp = file.with_extension("zst.tmp");
        let (file, tmp2) = (file.to_owned(), tmp.clone());
        let mut progress = Progress::for_job(&self.events, &server.id, job_id, "backup");
        let result = tokio::task::spawn_blocking(move || -> Result<Archive, FileError> {
            let out = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp2)?;
            let (out, report) = files.backup_to(
                std::io::BufWriter::with_capacity(1 << 20, out),
                &mut |done, total| progress.report(done, total),
            )?;
            let out = out
                .into_inner()
                .map_err(|e| FileError::Io(e.into_error()))?;
            out.sync_all()?;
            let size = out.metadata()?.len();
            drop(out);
            std::fs::rename(&tmp2, &file)?;
            Ok(Archive { size, report })
        })
        .await
        .map_err(|e| e.to_string())?;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&tmp).await;
        }
        result.map_err(|e| e.to_string())
    }

    /// Extracts archive `source` beside the server's folder and swaps it
    /// in, carrying over the folders backups leave out so they aren't
    /// downloaded again. Returns the files restored.
    async fn swap_in(&self, server: &Server, source: &Path, job_id: &str) -> Result<u64, String> {
        let servers_dir = self
            .servers_dir
            .clone()
            .ok_or("the data folder isn't available")?;
        let owner = self
            .open_files(server)
            .await
            .map_err(|e| e.to_string())?
            .owner();
        let stamp = jiff::Timestamp::now().as_second();
        let live = servers_dir.join(&server.slug);
        let incoming_name = format!("{}.restoring-{stamp}", server.slug);
        let replaced = servers_dir.join(format!("{}.replaced-{stamp}", server.slug));
        let (source, incoming_dir) = (source.to_owned(), servers_dir.clone());
        let mut progress = Progress::for_job(&self.events, &server.id, job_id, "restore");
        let name2 = incoming_name.clone();
        let report = tokio::task::spawn_blocking(move || {
            let size = std::fs::metadata(&source)?.len();
            let file = std::fs::File::open(&source)?;
            restore_into(
                std::io::BufReader::with_capacity(1 << 20, file),
                size,
                &incoming_dir,
                &name2,
                owner,
                Limits::default(),
                &mut |done, total| progress.report(done, total),
            )
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("couldn't read the backup: {e}"))?;
        let incoming = servers_dir.join(&incoming_name);

        let (live2, incoming2, replaced2) = (live.clone(), incoming.clone(), replaced.clone());
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            for name in REGENERATED {
                let (from, to) = (live2.join(name), incoming2.join(name));
                if from.is_dir() && !to.exists() {
                    std::fs::rename(&from, &to)?;
                }
            }
            std::fs::rename(&live2, &replaced2)?;
            if let Err(e) = std::fs::rename(&incoming2, &live2) {
                // Put the original back.
                let _ = std::fs::rename(&replaced2, &live2);
                return Err(e);
            }
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| {
            let _ = std::fs::remove_dir_all(&incoming);
            format!("couldn't swap the folders: {e}")
        })?;
        // The old folder is in the pre-restore backup; remove it.
        if let Ok(Err(e)) =
            tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&replaced)).await
        {
            tracing::warn!(error = %e, "couldn't remove the replaced server folder");
        }
        Ok(report.files)
    }

    async fn open_files(
        &self,
        server: &Server,
    ) -> Result<std::sync::Arc<ServerFiles>, crate::NodeError> {
        self.open_folder(server)
            .await?
            .into_local_files()
            .ok_or(crate::NodeError::NotYet(NOT_YET))
    }
}

impl Node {
    /// Free space where backups go, making the folder if needed. `None` if
    /// the system can't say.
    pub async fn backups_free_space(&self) -> Result<Option<u64>, String> {
        let dir = self
            .as_local(NOT_YET)
            .map_err(|e| e.to_string())?
            .backups_dir()
            .ok_or("the data folder isn't available, so backups can't be made")?;
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| format!("couldn't create the backups folder: {e}"))?;
        Ok(nn_uploads::available_space(&dir))
    }

    /// Archives `server` as `name`, reporting progress on job `job_id`.
    pub async fn backup(
        &self,
        server: &Server,
        name: &str,
        job_id: &str,
    ) -> Result<Archive, String> {
        let local = self.as_local(NOT_YET).map_err(|e| e.to_string())?;
        local
            .locked(server, async |current| {
                local.make(&current, name, job_id).await
            })
            .await
    }

    /// Restores archive `source` over `server`, which must be stopped,
    /// after archiving what's there as `pre_restore`.
    pub async fn restore(
        &self,
        server: &Server,
        source: &str,
        pre_restore: &str,
        job_id: &str,
    ) -> Result<Restored, String> {
        let local = self.as_local(NOT_YET).map_err(|e| e.to_string())?;
        let source = local
            .archive_path(source)
            .ok_or("the backup's file is missing")?;
        local
            .locked(server, async |current| {
                if !matches!(
                    current.status.as_str(),
                    "stopped" | "crashed" | "sleeping" | "installing"
                ) {
                    return Err(
                        "the server started again before the restore could begin; try again"
                            .to_owned(),
                    );
                }
                let pre = local.make(&current, pre_restore, job_id).await;
                if let Err(e) = &pre {
                    return Ok(Restored {
                        files: Err(e.clone()),
                        pre_restore: pre,
                    });
                }
                Ok(Restored {
                    files: local.swap_in(&current, &source, job_id).await,
                    pre_restore: pre,
                })
            })
            .await
    }

    /// Whether archive `name` is on this node.
    pub async fn has_backup(&self, name: &str) -> bool {
        let Ok(local) = self.as_local(NOT_YET) else {
            return false;
        };
        match local.archive_path(name) {
            Some(path) => tokio::fs::try_exists(&path).await.unwrap_or(false),
            None => false,
        }
    }

    /// Archive `name`, to download.
    pub async fn open_backup(&self, name: &str) -> Option<FileReader> {
        let path = self.as_local(NOT_YET).ok()?.archive_path(name)?;
        tokio::task::spawn_blocking(move || {
            let file = std::fs::File::open(&path)?;
            let size = file.metadata()?.len();
            Ok::<_, std::io::Error>(FileReader::local(file, size))
        })
        .await
        .ok()?
        .ok()
    }

    /// Deletes archive `name`. Gone already counts as deleted.
    pub async fn delete_backup(&self, name: &str) -> Result<(), String> {
        let local = self.as_local(NOT_YET).map_err(|e| e.to_string())?;
        let Some(path) = local.archive_path(name) else {
            return Ok(());
        };
        match tokio::fs::remove_file(&path).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(format!("couldn't delete the backup: {e}"))
            }
            _ => Ok(()),
        }
    }

    /// Cleans up after backups and restores NetherNode was stopped in the
    /// middle of: removes `unfinished` archives and partial files, and
    /// finishes or undoes half-swapped restores.
    pub async fn recover_backups(&self, unfinished: &[String]) {
        let Ok(local) = self.as_local(NOT_YET) else {
            return;
        };
        if let Some(dir) = local.backups_dir() {
            for name in unfinished.iter().filter_map(|n| checked(n)) {
                let _ = tokio::fs::remove_file(dir.join(name)).await;
                let _ = tokio::fs::remove_file(dir.join(format!("{name}.tmp"))).await;
                let _ = tokio::fs::remove_file(dir.join(name.replace(".tar.zst", ".tar.zst.tmp")))
                    .await;
            }
            if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    if entry.file_name().to_string_lossy().ends_with(".tmp") {
                        let _ = tokio::fs::remove_file(entry.path()).await;
                    }
                }
            }
        }
        let Some(servers_dir) = local.servers_dir.clone() else {
            return;
        };
        let _ = tokio::task::spawn_blocking(move || {
            let Ok(entries) = std::fs::read_dir(&servers_dir) else {
                return;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let path = entry.path();
                if let Some((slug, _)) = name.split_once(".restoring-") {
                    // Never swapped in: the live folder is untouched.
                    tracing::info!(server = %slug, "removing an interrupted restore");
                    let _ = std::fs::remove_dir_all(&path);
                } else if let Some((slug, _)) = name.split_once(".replaced-") {
                    let live = servers_dir.join(slug);
                    if live.exists() {
                        // The restore finished; this is the old copy.
                        let _ = std::fs::remove_dir_all(&path);
                    } else {
                        // Stopped between the two renames: put the original back.
                        tracing::warn!(server = %slug, "undoing an interrupted restore");
                        let _ = std::fs::rename(&path, &live);
                    }
                }
            }
        })
        .await;
    }
}
