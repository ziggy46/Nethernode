//! Moving a server to another machine: its archive is sent to the new
//! machine, which sets the server up from it under the same ID and slug.
//! The panel runs the move (see `nn_api::moves`).

use std::pin::Pin;

use nn_store::Server;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use crate::protocol::Request;
use crate::{LocalNode, Node, RemoteNode};

/// Free space kept back when receiving a server.
const DISK_MARGIN: u64 = 1024 * 1024 * 1024;

const NO_DATA: &str = "the data folder isn't available";

impl LocalNode {
    /// Receives archive `name` from `bytes` into the backups folder.
    pub(crate) async fn receive_archive(
        &self,
        name: &str,
        bytes: &mut (dyn AsyncRead + Send + Unpin),
        len: u64,
    ) -> Result<(), String> {
        let (Some(dir), Some(file)) = (self.backups_dir(), self.archive_path(name)) else {
            return Err(NO_DATA.into());
        };
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| format!("couldn't create the backups folder: {e}"))?;
        if let Some(free) = nn_uploads::available_space(&dir)
            && free < len.saturating_add(DISK_MARGIN)
        {
            return Err(format!(
                "there isn't enough free space for the server ({} MB needed)",
                (len + DISK_MARGIN) / (1024 * 1024)
            ));
        }
        let tmp = file.with_extension("zst.tmp");
        let result = async {
            let mut out = tokio::fs::File::create(&tmp).await?;
            let copied = tokio::io::copy(&mut bytes.take(len), &mut out).await?;
            if copied != len {
                return Err(std::io::Error::other("the archive was cut short"));
            }
            out.flush().await?;
            out.sync_all().await?;
            drop(out);
            tokio::fs::rename(&tmp, &file).await
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&tmp).await;
        }
        result.map_err(|e| format!("couldn't receive the server's archive: {e}"))
    }
}

impl RemoteNode {
    /// Sends `len` bytes of archive `name` to the agent.
    async fn send_archive(
        &self,
        name: &str,
        bytes: &mut (dyn AsyncRead + Send + Unpin),
        len: u64,
    ) -> crate::Result<()> {
        let request = Request::ReceiveBackup {
            name: name.to_owned(),
            len,
        };
        self.call_with_body(request, async |stream| {
            let sent = tokio::io::copy(&mut bytes.take(len), stream).await?;
            if sent != len {
                return Err(std::io::Error::other("the archive was cut short"));
            }
            stream.flush().await
        })
        .await
    }
}

impl Node {
    /// Receives archive `name` (`len` bytes) from another machine, for a
    /// server moving here.
    pub async fn receive_archive(
        &self,
        name: &str,
        mut bytes: Pin<Box<dyn AsyncRead + Send>>,
        len: u64,
    ) -> Result<(), String> {
        match self {
            Node::Local(local) => local.receive_archive(name, &mut bytes, len).await,
            Node::Remote(remote) => remote
                .send_archive(name, &mut bytes, len)
                .await
                .map_err(|e| e.to_string()),
        }
    }

    /// Sets up `server`, moving here from another machine, from received
    /// archive `archive`. Returns this machine's record of it, stopped. On
    /// failure, the caller cleans up (see [`Node::release`]).
    pub async fn adopt(
        &self,
        server: &Server,
        archive: &str,
        avoid_ports: &[i64],
        job_id: &str,
    ) -> Result<Server, String> {
        let local = match self {
            Node::Remote(remote) => {
                let request = Request::Adopt {
                    server: Box::new(server.clone()),
                    archive: archive.to_owned(),
                    avoid_ports: avoid_ports.to_vec(),
                    job_id: job_id.to_owned(),
                };
                return remote.call(request).await.map_err(|e| e.to_string());
            }
            Node::Local(local) => local,
        };
        let orchestrator = local.orchestrator().map_err(|e| e.to_string())?;
        let source = local
            .archive_path(archive)
            .ok_or("the server's archive is missing")?;
        let servers_dir = local.servers_dir.as_ref().ok_or(NO_DATA)?;
        let row = orchestrator
            .import(server, avoid_ports)
            .await
            .map_err(|e| e.to_string())?;
        // The restore swaps a folder out for the archive's contents.
        tokio::fs::create_dir_all(servers_dir.join(&row.slug))
            .await
            .map_err(|e| format!("couldn't create the server's folder: {e}"))?;
        local.swap_in(&row, &source, job_id).await?;
        orchestrator
            .finish_import(&row.id)
            .await
            .map_err(|e| e.to_string())
    }

    /// Removes a server's container and folder from this machine after it
    /// moved away (or failed to move here). On this machine its record is
    /// left to the caller; an agent deletes its own.
    pub async fn release(&self, server: &Server) -> Result<(), String> {
        match self {
            Node::Local(local) => local
                .orchestrator()
                .map_err(|e| e.to_string())?
                .release(&server.id, &server.slug)
                .await
                .map_err(|e| e.to_string()),
            Node::Remote(_) => self
                .delete(&server.id, true)
                .await
                .map_err(|e| e.to_string()),
        }
    }
}
