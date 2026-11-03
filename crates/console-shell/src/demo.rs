//! Static demo data for M2: service registry, scope hierarchy and user.
//! M4 replaces this with the gateway registry behind the same accessors.
//! Nothing here is an authorization decision (plan section 5).

/// One entry in the service menu.
pub struct DemoService {
    pub id: &'static str,
    pub title: &'static str,
    pub category: &'static str,
    pub href: &'static str,
    pub description: &'static str,
}

// POAM: AC-3/AC-6 - service list is static and unfiltered; server-side role filtering arrives with the M4 registry.
/// Placeholder tool links route to the shell's NotFound page in M2.
pub const SERVICES: &[DemoService] = &[
    DemoService {
        id: "home",
        title: "Console home",
        category: "Console",
        href: "/",
        description: "Overview of the console",
    },
    DemoService {
        id: "inventory",
        title: "Inventory",
        category: "Operations",
        href: "/t/inventory/",
        description: "Hosts and assets",
    },
    DemoService {
        id: "incidents",
        title: "Incidents",
        category: "Operations",
        href: "/t/incidents/",
        description: "Open and recent incidents",
    },
    DemoService {
        id: "metrics",
        title: "Metrics",
        category: "Monitoring",
        href: "/t/metrics/",
        description: "Dashboards and alarms",
    },
    DemoService {
        id: "logs",
        title: "Logs",
        category: "Monitoring",
        href: "/t/logs/",
        description: "Search application logs",
    },
    DemoService {
        id: "audit",
        title: "Audit trail",
        category: "Security",
        href: "/t/audit/",
        description: "Who did what, when",
    },
];

/// A tenant as offered at one site, with the environments it has there.
pub struct Tenant {
    pub id: &'static str,
    pub name: &'static str,
    pub envs: &'static [&'static str],
}

/// A site and the tenants available at it.
pub struct Site {
    pub id: &'static str,
    pub name: &'static str,
    pub tenants: &'static [Tenant],
}

/// Scope hierarchy: site -> tenant -> environment. `ops` exists at two sites
/// with different environments; `west` has a single tenant (used by tests).
pub const SITES: &[Site] = &[
    Site {
        id: "hq",
        name: "Headquarters",
        tenants: &[
            Tenant {
                id: "ops",
                name: "Operations",
                envs: &["prod", "stage", "dev"],
            },
            Tenant {
                id: "intel",
                name: "Intelligence",
                envs: &["prod"],
            },
        ],
    },
    Site {
        id: "east",
        name: "East DC",
        tenants: &[
            Tenant {
                id: "ops",
                name: "Operations",
                envs: &["prod", "stage"],
            },
            Tenant {
                id: "logistics",
                name: "Logistics",
                envs: &["prod", "dev"],
            },
        ],
    },
    Site {
        id: "west",
        name: "West DC",
        tenants: &[Tenant {
            id: "logistics",
            name: "Logistics",
            envs: &["stage"],
        }],
    },
];

/// Environment display labels.
pub const ENVIRONMENTS: &[(&str, &str)] = &[
    ("prod", "Production"),
    ("stage", "Staging"),
    ("dev", "Development"),
];

/// Static display identity until M3 (labelled "(demo)" in the UI).
pub struct DemoUser {
    pub display_name: &'static str,
}

pub const DEMO_USER: DemoUser = DemoUser {
    display_name: "Demo User",
};

pub fn env_label(id: &str) -> &str {
    ENVIRONMENTS
        .iter()
        .find(|(e, _)| *e == id)
        .map_or(id, |(_, l)| l)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::valid_charset;
    use std::collections::HashSet;

    #[test]
    fn ids_pass_charset_and_are_unique() {
        let mut svc = HashSet::new();
        for s in SERVICES {
            assert!(valid_charset(s.id), "{}", s.id);
            assert!(svc.insert(s.id), "dup service {}", s.id);
            assert!(s.href.starts_with('/') && !s.href.starts_with("//"));
        }
        let mut sites = HashSet::new();
        for s in SITES {
            assert!(valid_charset(s.id), "{}", s.id);
            assert!(sites.insert(s.id), "dup site {}", s.id);
            let mut ts = HashSet::new();
            for t in s.tenants {
                assert!(valid_charset(t.id), "{}", t.id);
                assert!(ts.insert(t.id), "dup tenant {} at {}", t.id, s.id);
                assert!(!t.envs.is_empty());
                for e in t.envs {
                    assert!(ENVIRONMENTS.iter().any(|(id, _)| id == e), "{e}");
                }
            }
        }
        for (e, _) in ENVIRONMENTS {
            assert!(valid_charset(e));
        }
    }

    #[test]
    fn has_cascade_fixtures() {
        let at = |t: &str| {
            SITES
                .iter()
                .filter(|s| s.tenants.iter().any(|x| x.id == t))
                .count()
        };
        assert!(
            SITES
                .iter()
                .any(|s| s.tenants.iter().any(|t| at(t.id) >= 2))
        );
        assert!(SITES.iter().any(|s| s.tenants.len() == 1));
    }
}
