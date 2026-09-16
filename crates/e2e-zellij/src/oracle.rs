//! Shared client / session / control-plane health checks.
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct Observation {
    pub client_alive: bool,
    pub sessions: BTreeSet<String>,
    pub expected_sessions: BTreeSet<String>,
    pub control_alive: bool,
    pub server_restarted: bool,
    pub lost_connection: bool,
}

#[derive(Debug)]
pub enum Judgement {
    Held,
    Broken {
        hold: &'static str,
        evidence: String,
    },
}

pub fn judge(observation: &Observation) -> Judgement {
    if !observation.client_alive {
        return Judgement::Broken {
            hold: "client_alive",
            evidence: String::new(),
        };
    }
    if !observation.control_alive {
        return Judgement::Broken {
            hold: "control_alive",
            evidence: String::new(),
        };
    }
    if observation.server_restarted {
        return Judgement::Broken {
            hold: "server_stable",
            evidence: String::new(),
        };
    }
    let missing: Vec<_> = observation
        .expected_sessions
        .difference(&observation.sessions)
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Judgement::Broken {
            hold: "session_exists",
            evidence: missing.join(","),
        };
    }
    if observation.lost_connection {
        return Judgement::Broken {
            hold: "lost_connection",
            evidence: "Lost connection".to_owned(),
        };
    }
    Judgement::Held
}
