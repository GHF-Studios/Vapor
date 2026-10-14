//! Repair targets and repair command intent.

/// Repair target used by `repair plan` and `repair apply`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairTarget {
    CoreState,
    Toolchain,
    Steam,
    ContentCatalog,
    ContentLibrary,
    ActiveComposition,
    All,
}

/// Repair commands intentionally split planning from mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairCommand {
    /// Inspect repairable areas without proposing mutations.
    Status,
    /// Produce a repair plan without applying it.
    Plan { target: RepairTarget },
    /// Apply repairs for a target after the plan is accepted.
    Apply { target: RepairTarget },
}
