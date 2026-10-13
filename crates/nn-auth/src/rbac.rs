//! Role-based access control.

use std::fmt;
use std::str::FromStr;

/// A user's role. Each role includes every permission of the roles below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    /// Read-only access.
    Viewer,
    /// Can manage players and use the console.
    Moderator,
    /// Can manage servers, files, the proxy, and settings.
    Admin,
    /// Full access, including user management and metrics.
    Owner,
}

impl Role {
    /// All roles, lowest privilege first.
    pub const ALL: [Role; 4] = [Role::Viewer, Role::Moderator, Role::Admin, Role::Owner];

    /// The role's stored name.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Moderator => "moderator",
            Role::Admin => "admin",
            Role::Owner => "owner",
        }
    }

    /// Whether this role grants `permission`.
    pub fn can(self, permission: Permission) -> bool {
        self >= permission.minimum_role()
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when parsing an unknown role name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown role: {0}")]
pub struct UnknownRole(pub String);

impl FromStr for Role {
    type Err = UnknownRole;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "viewer" => Ok(Role::Viewer),
            "moderator" => Ok(Role::Moderator),
            "admin" => Ok(Role::Admin),
            "owner" => Ok(Role::Owner),
            other => Err(UnknownRole(other.to_owned())),
        }
    }
}

/// An action gated by role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Permission {
    /// See dashboards, servers, and players.
    View,
    /// Kick, ban, whitelist, and op players; use the console.
    ManagePlayers,
    /// Create, start, stop, and configure servers, files, and mods.
    ManageServers,
    /// Configure proxy listeners and routes.
    ManageProxy,
    /// Change panel settings.
    ManageSettings,
    /// Create, edit, and delete panel users.
    ManageUsers,
    /// Read Prometheus metrics.
    ViewMetrics,
}

impl Permission {
    /// The least-privileged role that holds this permission.
    pub fn minimum_role(self) -> Role {
        match self {
            Permission::View => Role::Viewer,
            Permission::ManagePlayers => Role::Moderator,
            Permission::ManageServers | Permission::ManageProxy | Permission::ManageSettings => {
                Role::Admin
            }
            Permission::ManageUsers | Permission::ViewMetrics => Role::Owner,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_names_round_trip() {
        for role in Role::ALL {
            assert_eq!(role.as_str().parse::<Role>().unwrap(), role);
        }
        assert!("root".parse::<Role>().is_err());
    }

    #[test]
    fn permission_matrix() {
        use Permission::*;
        // (permission, [viewer, moderator, admin, owner])
        let matrix = [
            (View, [true, true, true, true]),
            (ManagePlayers, [false, true, true, true]),
            (ManageServers, [false, false, true, true]),
            (ManageProxy, [false, false, true, true]),
            (ManageSettings, [false, false, true, true]),
            (ManageUsers, [false, false, false, true]),
            (ViewMetrics, [false, false, false, true]),
        ];
        for (permission, expected) in matrix {
            for (role, allowed) in Role::ALL.into_iter().zip(expected) {
                assert_eq!(role.can(permission), allowed, "{role} / {permission:?}");
            }
        }
    }
}
