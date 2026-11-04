//! Review Buddy core: diff parsing and layout, syntax highlighting, the GitHub client,
//! git worktrees, the Claude stream-json session, and persistence. No GUI code.

pub mod models;
pub mod diff;
pub mod highlight;
pub mod claude;
pub mod github;
pub mod git;
pub mod review;
pub mod support;
pub mod persistence;

pub use models::*;
