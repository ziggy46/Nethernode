//! Where NetherNode's data folder is on the Docker host.
//!
//! Server containers get their folders as bind mounts, and Docker resolves
//! bind-mount paths on the host. When NetherNode itself runs in a container,
//! its data folder (say `/data`) is usually a different path on the host
//! (say `/srv/nethernode/data`, or a named volume), so the path NetherNode
//! sees would mount the wrong folder, or none. NetherNode finds its own
//! container, reads its mounts, and translates.

use std::path::{Path, PathBuf};

use bollard::Docker;

/// How the host path was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostPath {
    /// Not running in a container: paths are the same.
    Same,
    /// Running in a container; this is the host path.
    Translated(PathBuf),
    /// Running in a container, but the host path couldn't be found. Paths
    /// are used as-is, which only works if the host path is the same.
    Unknown(String),
}

/// The 64-hex-digit ID of the container this process runs in, from
/// `/proc/self/mountinfo`. Docker bind-mounts `/etc/hostname`,
/// `/etc/hosts`, and `/etc/resolv.conf` from `.../containers/<id>/`, even
/// with host networking (where the hostname isn't the container ID).
pub fn container_id(mountinfo: &str) -> Option<String> {
    mountinfo.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        // Fields: id parent major:minor root mount-point …
        let root = parts.nth(3)?;
        let mount_point = parts.next()?;
        if !matches!(
            mount_point,
            "/etc/hostname" | "/etc/hosts" | "/etc/resolv.conf"
        ) {
            return None;
        }
        let rest = root.split("/containers/").nth(1)?;
        let id = rest.split('/').next()?;
        (id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())).then(|| id.to_owned())
    })
}

/// `path` (inside the container) as a host path, given the container's
/// mounts as (host source, container destination). The deepest mount
/// containing `path` wins.
pub fn translate(path: &Path, mounts: &[(PathBuf, PathBuf)]) -> Option<PathBuf> {
    mounts
        .iter()
        .filter_map(|(source, destination)| {
            let rest = path.strip_prefix(destination).ok()?;
            Some((destination.components().count(), source.join(rest)))
        })
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, host)| host)
}

/// Finds the host path of `path`.
pub async fn detect(docker: &Docker, path: &Path) -> HostPath {
    let in_container = Path::new("/.dockerenv").exists();
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let Some(id) = container_id(&mountinfo) else {
        return if in_container {
            HostPath::Unknown("couldn't find this container's ID".into())
        } else {
            HostPath::Same
        };
    };
    let container = match docker.inspect_container(&id, None).await {
        Ok(c) => c,
        Err(e) => return HostPath::Unknown(format!("couldn't inspect this container: {e}")),
    };
    let mounts: Vec<(PathBuf, PathBuf)> = container
        .mounts
        .unwrap_or_default()
        .into_iter()
        .filter_map(|m| Some((PathBuf::from(m.source?), PathBuf::from(m.destination?))))
        .collect();
    match translate(path, &mounts) {
        Some(host) => HostPath::Translated(host),
        None => HostPath::Unknown(format!(
            "{} isn't on a volume or bind mount, so server folders would be lost with the container",
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "4f2c9d0e1b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d";

    #[test]
    fn finds_the_container_id() {
        let mountinfo = format!(
            "\
741 600 0:59 / / rw,relatime master:221 - overlay overlay rw,lowerdir=/var/lib/docker/overlay2/l/X
755 741 259:2 /var/lib/docker/containers/{ID}/resolv.conf /etc/resolv.conf rw,relatime - ext4 /dev/nvme0n1p2 rw
756 741 259:2 /var/lib/docker/containers/{ID}/hostname /etc/hostname rw,relatime - ext4 /dev/nvme0n1p2 rw
760 741 259:2 /srv/nethernode/data /data rw,relatime - ext4 /dev/nvme0n1p2 rw
"
        );
        assert_eq!(container_id(&mountinfo).as_deref(), Some(ID));
        // A host process has no such mounts.
        assert_eq!(
            container_id("22 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw\n"),
            None
        );
        // Something else under a "containers" folder doesn't count.
        let other =
            format!("9 1 259:2 /var/lib/docker/containers/{ID}/x /data rw - ext4 /dev/x rw\n");
        assert_eq!(container_id(&other), None);
    }

    #[test]
    fn translates_through_the_deepest_mount() {
        let mounts = vec![
            (
                PathBuf::from("/srv/nethernode/data"),
                PathBuf::from("/data"),
            ),
            (
                PathBuf::from("/var/lib/docker/volumes/nn_servers/_data"),
                PathBuf::from("/data/servers"),
            ),
            (
                PathBuf::from("/var/run/docker.sock"),
                PathBuf::from("/var/run/docker.sock"),
            ),
        ];
        assert_eq!(
            translate(Path::new("/data/backups"), &mounts),
            Some(PathBuf::from("/srv/nethernode/data/backups"))
        );
        assert_eq!(
            translate(Path::new("/data/servers"), &mounts),
            Some(PathBuf::from("/var/lib/docker/volumes/nn_servers/_data"))
        );
        // Prefixes match whole path components, not strings.
        assert_eq!(translate(Path::new("/database"), &mounts), None);
        assert_eq!(translate(Path::new("/opt/x"), &mounts), None);
    }
}
