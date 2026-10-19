//! Agent tools with approval gating (README-DK). TODO(M7).
//!
//! v1 ships read-only tools only; anything that changes the system must go
//! through an approval dialog and the `approval.needed` pose.

/// Risk class of a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Risk {
    Safe,
    Risky,
    Danger,
}

/// A tool the LLM may call.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub risk: Risk,
}

/// Built-in read-only tools (README-SP agent tools).
pub const READ_ONLY_TOOLS: &[Tool] = &[
    Tool {
        name: "system_summary",
        description: "CPU, RAM, disk, load, uptime",
        risk: Risk::Safe,
    },
    Tool {
        name: "disk_usage",
        description: "Mounted filesystems and free space",
        risk: Risk::Safe,
    },
    Tool {
        name: "active_window",
        description: "Title and class of the focused window",
        risk: Risk::Safe,
    },
    Tool {
        name: "top_processes",
        description: "Top processes by CPU/RAM",
        risk: Risk::Safe,
    },
    Tool {
        name: "network",
        description: "Connectivity, Wi-Fi strength, VPN",
        risk: Risk::Safe,
    },
    Tool {
        name: "temperatures",
        description: "CPU/GPU temperatures",
        risk: Risk::Safe,
    },
];

/// Whether a tool can run without asking.
pub fn needs_approval(tool: &Tool) -> bool {
    tool.risk > Risk::Safe
}
