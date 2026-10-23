//! Shared process execution owned by Vapor's core operation machinery.

pub(crate) use vapor_core::managed_process::{
    run_cancellable_with_line_limit, run_with_line_limit, write_to_terminal,
};
