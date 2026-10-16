//! Replacing the container NetherNode runs in with a copy on another image:
//! how an agent installed with Docker updates itself. The copy gets the
//! container's name and settings; the old container is renamed and told not
//! to restart, and the copy removes it once it has stopped.

use std::time::Duration;

use bollard::Docker;
use bollard::models::{
    ContainerCreateBody, ContainerUpdateBody, EndpointSettings, NetworkingConfig, RestartPolicy,
    RestartPolicyNameEnum,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, RemoveContainerOptionsBuilder,
    RenameContainerOptions,
};
use futures_util::StreamExt;

use crate::host_path::container_id;

/// On a replacement container: the ID of the container it replaced.
pub const REPLACES_LABEL: &str = "dev.nethernode.replaces";

/// Where official images come from, when the running one doesn't say.
pub const OFFICIAL_REPOSITORY: &str = "ghcr.io/ziggy46/nethernode";

/// How long the copy must stay up before the old container hands over.
const SETTLE: Duration = Duration::from_secs(5);

/// The container this process runs in, if any.
pub fn own_container() -> Option<String> {
    container_id(&std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default())
}

/// An image reference without its tag or digest; `None` for a bare image ID.
pub fn repository(image: &str) -> Option<&str> {
    let image = image.split('@').next().unwrap_or_default();
    if image.is_empty() || image.starts_with("sha256:") {
        return None;
    }
    Some(match image.rsplit_once(':') {
        Some((repo, tag)) if !tag.contains('/') => repo,
        _ => image,
    })
}

/// What [`replace`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Replaced {
    /// The container already runs that image.
    Current,
    /// A copy on `image` is running; this process should exit now.
    Started {
        /// The image the copy runs.
        image: String,
    },
}

/// Pulls version `version` of the image container `id` runs (same
/// repository), and starts a copy of the container on it. If the copy
/// doesn't start and stay up, everything is put back as it was.
pub async fn replace(docker: &Docker, id: &str, version: &str) -> Result<Replaced, String> {
    let me = docker
        .inspect_container(id, None)
        .await
        .map_err(|e| format!("couldn't inspect this agent's container: {e}"))?;
    let config = me.config.clone().unwrap_or_default();
    let current = config.image.clone().unwrap_or_default();
    let repo = repository(&current)
        .unwrap_or(OFFICIAL_REPOSITORY)
        .to_owned();
    let target = format!("{repo}:{version}");
    let pulled = pull(docker, &repo, version).await;
    let pulled = match (docker.inspect_image(&target).await, pulled) {
        (Ok(image), Ok(())) => image,
        // A copy already here (no registry access, or built locally).
        (Ok(image), Err(e)) => {
            tracing::warn!(image = %target, error = %e, "couldn't download the new agent image; using the copy already here");
            image
        }
        (Err(_), Err(e)) => return Err(e),
        (Err(e), Ok(())) => return Err(format!("couldn't inspect {target}: {e}")),
    };
    if pulled.id.is_some() && pulled.id == me.image {
        return Ok(Replaced::Current);
    }

    let name = me
        .name
        .as_deref()
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_owned();
    if name.is_empty() {
        return Err("this agent's container has no name".into());
    }
    let host_config = me.host_config.clone().unwrap_or_default();
    let mut body: ContainerCreateBody = serde_json::to_value(&config)
        .and_then(serde_json::from_value)
        .map_err(|e| format!("couldn't copy this agent's container settings: {e}"))?;
    body.image = Some(target.clone());
    // The new image brings its own descriptive labels.
    let mut labels = body.labels.take().unwrap_or_default();
    labels.retain(|key, _| !key.starts_with("org.opencontainers.image."));
    labels.insert(REPLACES_LABEL.into(), id.into());
    body.labels = Some(labels);
    let network_mode = host_config.network_mode.clone().unwrap_or_default();
    if network_mode == "host" {
        // Docker refuses a hostname with host networking.
        body.hostname = None;
        body.domainname = None;
    } else if !matches!(network_mode.as_str(), "" | "default" | "bridge" | "none")
        && !network_mode.starts_with("container:")
    {
        let endpoints = me
            .network_settings
            .and_then(|n| n.networks)
            .unwrap_or_default()
            .into_keys()
            .map(|network| (network, EndpointSettings::default()))
            .collect();
        body.networking_config = Some(NetworkingConfig {
            endpoints_config: Some(endpoints),
        });
    }
    body.host_config = Some(host_config.clone());

    // Out of the way, and not restarted once this process exits.
    let aside = format!("{name}-replaced-{}", &id[..12.min(id.len())]);
    docker
        .rename_container(id, RenameContainerOptions { name: aside })
        .await
        .map_err(|e| format!("couldn't rename this agent's container: {e}"))?;
    let restart = |policy: Option<RestartPolicy>| ContainerUpdateBody {
        restart_policy: policy,
        ..Default::default()
    };
    let no_restart = Some(RestartPolicy {
        name: Some(RestartPolicyNameEnum::NO),
        maximum_retry_count: None,
    });
    let undo = async |copy: Option<&str>| {
        if let Some(copy) = copy {
            let force = RemoveContainerOptionsBuilder::default().force(true).build();
            let _ = docker.remove_container(copy, Some(force)).await;
        }
        let _ = docker
            .rename_container(id, RenameContainerOptions { name: name.clone() })
            .await;
        let _ = docker
            .update_container(id, restart(host_config.restart_policy.clone()))
            .await;
    };
    if let Err(e) = docker.update_container(id, restart(no_restart)).await {
        undo(None).await;
        return Err(format!("couldn't change this agent's restart policy: {e}"));
    }
    let options = CreateContainerOptionsBuilder::default().name(&name).build();
    let copy = match docker.create_container(Some(options), body).await {
        Ok(created) => created.id,
        Err(e) => {
            undo(None).await;
            return Err(format!("couldn't create the new agent container: {e}"));
        }
    };
    if let Err(e) = docker.start_container(&copy, None).await {
        undo(Some(&copy)).await;
        return Err(format!("couldn't start the new agent container: {e}"));
    }
    tokio::time::sleep(SETTLE).await;
    let running = docker
        .inspect_container(&copy, None)
        .await
        .ok()
        .and_then(|c| c.state)
        .and_then(|s| s.running)
        .unwrap_or(false);
    if !running {
        undo(Some(&copy)).await;
        return Err(format!(
            "{target} stopped right after starting; staying on this version"
        ));
    }
    Ok(Replaced::Started { image: target })
}

async fn pull(docker: &Docker, repo: &str, tag: &str) -> Result<(), String> {
    tracing::info!(image = %format!("{repo}:{tag}"), "pulling the new agent image");
    let options = CreateImageOptionsBuilder::default()
        .from_image(repo)
        .tag(tag)
        .build();
    let mut stream = docker.create_image(Some(options), None, None);
    while let Some(item) = stream.next().await {
        item.map_err(|e| format!("couldn't download {repo}:{tag}: {e}"))?;
    }
    Ok(())
}

/// If this container replaced another, waits for that one to stop (stopping
/// it after a minute), then removes it. Call before opening the data
/// folder, so two agents never use it at once.
pub async fn finish_replacing(socket: &str) {
    let Some(id) = own_container() else {
        return;
    };
    let Ok(docker) = Docker::connect_with_socket(socket, 120, bollard::API_DEFAULT_VERSION) else {
        return;
    };
    let Ok(me) = docker.inspect_container(&id, None).await else {
        return;
    };
    let Some(old) = me
        .config
        .and_then(|c| c.labels)
        .and_then(|labels| labels.get(REPLACES_LABEL).cloned())
    else {
        return;
    };
    for _ in 0..60 {
        match docker.inspect_container(&old, None).await {
            Ok(c) if c.state.as_ref().and_then(|s| s.running) == Some(true) => {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Ok(_) => break,
            // Already gone.
            Err(_) => return,
        }
    }
    let force = RemoveContainerOptionsBuilder::default().force(true).build();
    match docker.remove_container(&old, Some(force)).await {
        Ok(()) => tracing::info!(replaced = %old, "removed the agent container this one replaced"),
        Err(e) => {
            tracing::warn!(replaced = %old, error = %e, "couldn't remove the agent container this one replaced")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::repository;

    #[test]
    fn repositories_drop_tags_and_digests() {
        assert_eq!(
            repository("ghcr.io/ziggy46/nethernode:0.2"),
            Some("ghcr.io/ziggy46/nethernode")
        );
        assert_eq!(
            repository("localhost:5000/nn:dev"),
            Some("localhost:5000/nn")
        );
        assert_eq!(repository("localhost:5000/nn"), Some("localhost:5000/nn"));
        assert_eq!(repository("nethernode@sha256:abc"), Some("nethernode"));
        assert_eq!(repository("sha256:0123abcd"), None);
    }
}
