//! The Cloudflare Tunnel connector: a `cloudflared` container NetherNode
//! runs for the panel when Cloudflare is connected (see `nn_api::cloudflare`).
//!
//! It uses the host's network, so the tunnel's routes can reach the panel at
//! `127.0.0.1:<its port>` whether the panel is a plain binary or a container
//! with a published port. It's labelled as the tunnel, never as a server or
//! a module, so neither's reconciliation touches it, and Docker restarts it
//! (`unless-stopped`) so it outlives a panel restart.

use std::collections::HashMap;

use bollard::models::{ContainerCreateBody, HostConfig, RestartPolicy, RestartPolicyNameEnum};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, LogsOptionsBuilder,
    RemoveContainerOptionsBuilder, StartContainerOptions,
};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::lifecycle::{Orchestrator, is_not_found};
use crate::modules::ModuleState;
use crate::spec::ContainerSpec;

/// The connector's container name.
pub const CONTAINER: &str = "nethernode-cloudflared";
/// Label marking the connector.
pub const LABEL_TUNNEL: &str = "nethernode.tunnel";
const LABEL_HASH: &str = "nethernode.tunnel_hash";
/// cloudflared 2026.9.3, pinned (amd64 and arm64).
pub const IMAGE: &str = "cloudflare/cloudflared@sha256:072c067d25ccbe61d46e18f0d0723255f2bb5304f7317caa95b27031520ff92c";

/// The connector's container definition, for the tunnel with run token
/// `token`.
pub fn spec(token: &str) -> ContainerSpec {
    let host_config = HostConfig {
        network_mode: Some("host".into()),
        restart_policy: Some(RestartPolicy {
            name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
            maximum_retry_count: None,
        }),
        security_opt: Some(vec!["no-new-privileges".into()]),
        memory: Some(256 * 1024 * 1024),
        ..Default::default()
    };
    let mut labels = HashMap::from([(LABEL_TUNNEL.to_owned(), "true".to_owned())]);
    let mut body = ContainerCreateBody {
        image: Some(IMAGE.into()),
        cmd: Some(vec![
            "tunnel".into(),
            "--no-autoupdate".into(),
            "run".into(),
        ]),
        // The token in the environment keeps it out of `ps` listings.
        env: Some(vec![format!("TUNNEL_TOKEN={token}")]),
        host_config: Some(host_config),
        labels: Some(labels.clone()),
        ..Default::default()
    };
    let hash = hex::encode(Sha256::digest(
        serde_json::to_vec(&body).unwrap_or_default(),
    ));
    labels.insert(LABEL_HASH.to_owned(), hash.clone());
    body.labels = Some(labels);
    ContainerSpec {
        name: CONTAINER.into(),
        body,
        hash,
    }
}

impl Orchestrator {
    /// Runs the connector for the tunnel with run token `token`, creating
    /// or replacing its container as needed. Leaves a running one with the
    /// same token alone.
    pub async fn run_tunnel(&self, token: &str) -> Result<()> {
        let spec = spec(token);
        let create = match self.docker.inspect_container(CONTAINER, None).await {
            Ok(existing) => {
                let hash = existing
                    .config
                    .and_then(|c| c.labels)
                    .and_then(|l| l.get(LABEL_HASH).cloned());
                let running = existing.state.and_then(|s| s.running).unwrap_or(false);
                if hash.as_deref() == Some(spec.hash.as_str()) {
                    if running {
                        return Ok(());
                    }
                    false
                } else {
                    self.stop_tunnel().await?;
                    true
                }
            }
            Err(e) if is_not_found(&e) => true,
            Err(e) => return Err(e.into()),
        };
        if create {
            if self.docker.inspect_image(IMAGE).await.is_err() {
                let (repo, digest) = IMAGE.split_once('@').unwrap_or((IMAGE, "latest"));
                let options = CreateImageOptionsBuilder::default()
                    .from_image(repo)
                    .tag(digest)
                    .build();
                let mut pull = self.docker.create_image(Some(options), None, None);
                while let Some(item) = pull.next().await {
                    item?;
                }
            }
            let options = CreateContainerOptionsBuilder::default()
                .name(CONTAINER)
                .build();
            self.docker
                .create_container(Some(options), spec.body)
                .await?;
        }
        self.docker
            .start_container(CONTAINER, None::<StartContainerOptions>)
            .await?;
        tracing::info!("Cloudflare Tunnel connector started");
        Ok(())
    }

    /// Removes the connector.
    pub async fn stop_tunnel(&self) -> Result<()> {
        let options = RemoveContainerOptionsBuilder::default().force(true).build();
        match self.docker.remove_container(CONTAINER, Some(options)).await {
            Ok(()) => Ok(()),
            Err(e) if is_not_found(&e) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// What the connector is doing (`running`, `stopped`, `failed` or
    /// `absent`), with the end of its log when it isn't running.
    pub async fn tunnel_state(&self) -> ModuleState {
        let state = match self.docker.inspect_container(CONTAINER, None).await {
            Ok(c) => c.state.unwrap_or_default(),
            Err(e) if is_not_found(&e) => {
                return ModuleState {
                    status: "absent".into(),
                    message: None,
                };
            }
            Err(e) => {
                return ModuleState {
                    status: "failed".into(),
                    message: Some(format!("Docker: {e}")),
                };
            }
        };
        if state.running.unwrap_or(false) {
            return ModuleState {
                status: "running".into(),
                message: None,
            };
        }
        let options = LogsOptionsBuilder::default()
            .stdout(true)
            .stderr(true)
            .tail("5")
            .build();
        let mut logs = self.docker.logs(CONTAINER, Some(options));
        let mut tail = String::new();
        while let Some(Ok(line)) = logs.next().await {
            tail.push_str(&line.to_string());
        }
        ModuleState {
            status: if state.exit_code.unwrap_or(0) == 0 {
                "stopped".into()
            } else {
                "failed".into()
            },
            message: Some(tail.trim().to_owned()).filter(|t| !t.is_empty()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_connector_is_kept_apart_and_follows_its_token() {
        let a = spec("token-a");
        assert_eq!(a.name, CONTAINER);
        let labels = a.body.labels.as_ref().unwrap();
        assert_eq!(labels[LABEL_TUNNEL], "true");
        assert!(!labels.contains_key(crate::spec::LABEL_MANAGED));
        assert!(!labels.contains_key(crate::modules::LABEL_MODULE));
        let host = a.body.host_config.as_ref().unwrap();
        assert_eq!(host.network_mode.as_deref(), Some("host"));
        assert_eq!(
            a.body.env.as_deref(),
            Some(&["TUNNEL_TOKEN=token-a".to_owned()][..])
        );
        assert!(
            !a.body
                .cmd
                .as_ref()
                .unwrap()
                .iter()
                .any(|c| c.contains("token-a")),
            "the token isn't in the command line"
        );
        assert!(IMAGE.contains("@sha256:"));
        assert_ne!(a.hash, spec("token-b").hash);
    }
}
